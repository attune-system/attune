//! Prospective page custody, separate from the shared A/J repository test root.
use super::*;
use attune_common::repositories::{
    execution_runtime_admission::ExecutionAdmission, native_input_snapshot::NativeStorageParent,
    workflow_cache_iteration::UpdateWorkflowCacheIterationProgressInput,
};

#[tokio::test]
async fn initial_and_linked_next_file_pins_keep_actual_frontier_epoch_and_physical_entries_until_settlement(
) {
    super::native_storage_custody_case(true).await;
}

pub(super) async fn capture_linked_cache(
    tx: &mut sqlx::PgConnection,
    renderer: Id,
    workflow: Id,
    task: &ExecutionAdmission,
) -> (ReceiptContext, Id) {
    let path = "/workflow/tasks/0";
    assert!(
        WorkflowCacheIterationRepository::find_by_workflow_task(&mut *tx, workflow, "cached")
            .await
            .unwrap()
            .is_none()
    );
    let initial =
        Snapshots::prepare_cache_initial(tx, renderer, path, task, workflow, 65536, &[], KEY)
            .await
            .unwrap();
    let next = Snapshots::prepare_cache_next(tx, &initial, 65536, KEY)
        .await
        .unwrap();
    let eof = Snapshots::prepare_cache_next(tx, &next, 65536, KEY)
        .await
        .unwrap();
    assert_eq!(initial.count(), 2);
    assert_eq!(next.count(), 1);
    assert!(eof.value().is_none());
    assert!(
        WorkflowCacheIterationRepository::find_by_workflow_task(&mut *tx, workflow, "cached")
            .await
            .unwrap()
            .is_none(),
        "all page sources must be prepared before an actual iteration ID exists"
    );
    let declarations = DeclaredInputRepository::bindings(tx, renderer)
        .await
        .unwrap();
    let declaration = declarations
        .iter()
        .find(|d| d.source_path.starts_with(&format!("{path}/params/")))
        .unwrap();
    let mut session = Inputs::begin_render(
        tx,
        renderer,
        &declaration.declaration_revision,
        &declaration.source_path,
        &declaration.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_cache_input(&mut session, &next).unwrap();
    let rendered = Inputs::evaluate(tx, session).await.unwrap();
    let destination = InputSite {
        execution: renderer,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/native_linked_cache".into(),
    };
    let prepared = Inputs::prepare_destination(tx, &rendered, &destination, KEY)
        .await
        .unwrap();
    // Actual cache/iteration target acquisition starts only after all sources and
    // the final output are prepared. No IDs, cursor or ordinal are invented.
    CacheEntryRepository::protect_transaction(tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let iteration =
        WorkflowCacheIterationRepository::create(&mut *tx, initial.initial_create_input().unwrap())
            .await
            .unwrap();
    assert!(Inputs::record_prepared(
        tx,
        &rendered,
        &prepared,
        rendered.value(),
        rendered.selections(),
        KEY
    )
    .await
    .is_err());
    assert!(Snapshots::bind_cache_frontier(tx, &next, iteration.id)
        .await
        .is_err());
    Snapshots::bind_cache_frontier(tx, &initial, iteration.id)
        .await
        .unwrap();
    WorkflowCacheIterationRepository::update_scan_progress(
        &mut *tx,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: initial.after_external_id().unwrap().into(),
            next_batch_index: initial.index() + 1,
            scanned_count: initial.count() as i64,
            dispatched_count: 1,
        },
    )
    .await
    .unwrap()
    .unwrap();
    Snapshots::bind_cache_frontier(tx, &next, iteration.id)
        .await
        .unwrap();
    let receipt = Inputs::record_prepared(
        tx,
        &rendered,
        &prepared,
        rendered.value(),
        rendered.selections(),
        KEY,
    )
    .await
    .unwrap();
    WorkflowCacheIterationRepository::update_scan_progress(
        &mut *tx,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: next.after_external_id().unwrap().into(),
            next_batch_index: next.index() + 1,
            scanned_count: (initial.count() + next.count()) as i64,
            dispatched_count: 2,
        },
    )
    .await
    .unwrap()
    .unwrap();
    Snapshots::bind_cache_frontier(tx, &eof, iteration.id)
        .await
        .unwrap();
    initial.ensure_fresh().unwrap();
    next.ensure_fresh().unwrap();
    eof.ensure_fresh().unwrap();
    receipt.ensure_fresh().unwrap();
    (receipt, iteration.id)
}

pub(super) async fn assert_pinned_after_raw_iteration_gc(
    db: &TestDatabase,
    version: Id,
    iteration: Id,
    snapshot: Id,
) {
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    let original_epoch: String = sqlx::query_scalar(
        "SELECT creation_transaction FROM runtime_cache_iteration_epoch WHERE iteration=$1",
    )
    .bind(iteration)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let storage = Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    assert_eq!(storage.cache().unwrap().iteration(), iteration);
    let metadata = storage.metadata().clone();
    let original_origins = storage.origins().to_vec();
    let predecessor = storage
        .physical_parents()
        .iter()
        .find_map(|parent| match parent {
            NativeStorageParent::NativeSnapshot { snapshot } => Some(*snapshot),
            _ => None,
        })
        .unwrap();
    assert!(
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", predecessor)
            .await
            .unwrap()
    );
    assert!(
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
            .await
            .unwrap()
    );
    let all:Vec<(Id,Option<Id>)>=sqlx::query_as("SELECT snapshot,predecessor FROM runtime_native_cache_frontier WHERE bound_iteration=$1 ORDER BY ordinal")
        .bind(iteration).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0], (predecessor, None));
    assert_eq!(all[1], (snapshot, Some(predecessor)));
    assert_eq!(all[2].1, Some(snapshot));
    let first_bytes = Snapshots::storage_evidence(&mut tx, predecessor)
        .await
        .unwrap()
        .retained_bytes();
    let eof_bytes = Snapshots::storage_evidence(&mut tx, all[2].0)
        .await
        .unwrap()
        .retained_bytes();
    assert!(first_bytes < storage.retained_bytes() && storage.retained_bytes() < eof_bytes);
    for id in [predecessor, snapshot] {
        assert_eq!(
            sqlx::query("DELETE FROM runtime_native_cache_frontier WHERE snapshot=$1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .unwrap()
                .rows_affected(),
            0,
            "an owned physical frontier association cannot be removed separately"
        );
    }
    assert_eq!(
        sqlx::query("DELETE FROM runtime_cache_iteration_epoch WHERE iteration=$1")
            .bind(iteration)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query("DELETE FROM workflow_cache_iteration WHERE id=$1")
            .bind(iteration)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected(),
        1,
        "raw iteration GC is independent of frozen frontier and creation history"
    );
    let epoch: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_cache_iteration_epoch WHERE iteration=$1)",
    )
    .bind(iteration)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(epoch);
    let retained_epoch: String = sqlx::query_scalar(
        "SELECT creation_transaction FROM runtime_cache_iteration_epoch WHERE iteration=$1",
    )
    .bind(iteration)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(retained_epoch, original_epoch);
    assert_eq!(Evidence::collect_cache_epochs(&mut tx, 1).await.unwrap(), 0);
    let retained = Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    assert_eq!(retained.cache().unwrap().iteration(), iteration);
    assert_eq!(retained.metadata(), &metadata);
    assert_eq!(
        retained.origins(),
        original_origins.as_slice(),
        "binding and raw GC never rebase the original audit origin"
    );
    tx.commit().await.unwrap();
    sqlx::query("UPDATE file_evidence_owner SET evidence_state='pending_audit' WHERE version=$1")
        .bind(version)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        Evidence::readmit(db.pool(), version).await.unwrap(),
        "keeper inventory must remain keyless and positive after raw iteration GC"
    );
}

pub(super) async fn assert_released(db: &TestDatabase, iteration: Id) {
    let frontiers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_cache_frontier WHERE bound_iteration=$1",
    )
    .bind(iteration)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let epochs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM runtime_cache_iteration_epoch WHERE iteration=$1")
            .bind(iteration)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        frontiers, 0,
        "unowned EOF and successors must collect before their predecessors"
    );
    assert_eq!(
        epochs, 0,
        "creation history releases only after all physical/frontier owners settle"
    );
}
