//! Exercise the dedicated terminal constructor, not ordinary successor staging.
use super::*;

#[tokio::test]
async fn native_ordinary_iteration_empty_root_inventory_counts_unselected_original_fields() {
    for leaves in [152, 157] {
        let (db, root, workflow, _, _) = setup_graph_contract(
            json!([]),
            None,
            false,
            0,
            false,
            Some((false, false)),
            false,
            EmptyProgram::TerminalInventory(leaves),
        )
        .await;
        let mut tx = db.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        let fields = Secrets::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, root.id)
            .await
            .unwrap();
        assert_eq!(
            fields.len(),
            100,
            "one hundred actual encrypted config rows, not projected recipe JSON"
        );
        for field in &fields {
            assert!(field.json_path.starts_with("/unselected_"));
            assert!(
                attune_common::crypto::decrypt_json(&field.encrypted_value, KEY)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .starts_with("recorded input ")
            );
        }
        let (ordinary, original_receipts) =
            attune_common::repositories::immutable_input_origin::prepare_input_receipts(
                &mut tx,
                ENTITY_EXECUTION_CONFIG,
                root.id,
                &fields,
                KEY,
            )
            .await
            .unwrap();
        assert!(ordinary.is_empty());
        assert_eq!(original_receipts.len(), 100);
        Materialize::prepare_execution_input_receipts(&mut tx, root.id, KEY)
            .await
            .unwrap();
        assert!(
            Secrets::find_stored_by_entity(&mut *tx, "workflow_variables", root.id)
                .await
                .unwrap()
                .is_empty()
        );
        let mut target_lock = db.begin().await.unwrap();
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .execute(&mut *target_lock)
            .await
            .unwrap();
        // Bound lock waits rather than CPU/DB work for hundreds of genuine
        // original fields. The test binary's 600-second deadline is unchanged.
        sqlx::query("SET LOCAL lock_timeout='1s'")
            .execute(&mut *tx)
            .await
            .unwrap();
        let prepared = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY).await;
        target_lock.rollback().await.unwrap();
        if leaves == 152 {
            let mut prepared =
                prepared.expect("a recipe within both independent inventory and graph bounds");
            let applied = prepared.apply(&mut tx, KEY).await.unwrap();
            assert!(applied.requested.is_empty());
            assert_eq!(
                applied.completed_parent.unwrap().status,
                ExecutionStatus::Completed
            );
            tx.commit().await.unwrap();
            drop(prepared);
            drop(original_receipts);
            let mut cold = db.begin().await.unwrap();
            let replay = Iteration::prepare_empty_root_replay(&mut cold, root.id, KEY)
                .await
                .unwrap();
            replay.ensure_fresh_on(&mut cold).await.unwrap();
            let recipe: Value = sqlx::query_scalar(
                "SELECT recipe FROM runtime_native_empty_root_release WHERE execution=$1",
            )
            .bind(root.id)
            .fetch_one(&mut *cold)
            .await
            .unwrap();
            assert_eq!(recipe["config_fields"].as_array().unwrap().len(), 100);
            assert_eq!(recipe["leaves"].as_array().unwrap().len(), 152);
            cold.commit().await.unwrap();
        } else {
            if let Err(error) = &prepared {
                eprintln!("independent recipe inventory admission: {error}");
            }
            let denied = matches!(&prepared, Err(attune_common::Error::PermissionDenied(message))
                if message == "Empty root recipe inventory exceeds bound");
            drop(prepared);
            // Commit the denial so rollback cannot conceal partial publication.
            tx.commit().await.unwrap();
            let current = ExecutionRepository::find_by_id(&*db, root.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(current.status, ExecutionStatus::Running);
            assert!(current.result.is_none());
            let current_workflow = Workflows::find_by_id(&*db, workflow.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::to_value(&current_workflow).unwrap(),
                serde_json::to_value(&workflow).unwrap()
            );
            assert!(
                Secrets::find_stored_by_entity(&*db, "execution_result", root.id)
                    .await
                    .unwrap()
                    .is_empty()
            );
            let releases: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM runtime_native_empty_root_release WHERE execution=$1",
            )
            .bind(root.id)
            .fetch_one(&*db)
            .await
            .unwrap();
            assert_eq!(releases, 0);
            assert!(ExecutionRepository::find_by_parent(&*db, root.id)
                .await
                .unwrap()
                .is_empty());
            db.cleanup().await.unwrap();
            assert!(denied, "dedicated root constructor did not enforce the independent recipe inventory for 100 original config fields plus 157 output leaves");
            continue;
        }
        db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn native_ordinary_iteration_empty_root_budget_rejects_cumulative_nodes() {
    let (db, root, _, _, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        EmptyProgram::TerminalBudget(255, 1),
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let result = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY).await;
    assert!(
        matches!(result, Err(attune_common::Error::PermissionDenied(_))),
        "new leaf receipts and retained ancestry must share one node limit"
    );
    tx.commit().await.unwrap();
    assert_eq!(
        ExecutionRepository::find_by_id(&*db, root.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    assert!(ExecutionRepository::find_by_parent(&*db, root.id)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_empty_root_budget_mixed_rejects_before_publication() {
    use attune_common::repositories::{
        native_failure_materialization::NativeFailureMaterializationRepository as Success,
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
    };
    let (db, root, workflow, _, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        EmptyProgram::OrdinaryTerminalBudget(12, 16 * 1024),
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let mut activation = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    let children = activation.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(children.len(), 1);
    let child = ExecutionRepository::update(
        &mut *tx,
        children[0].id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(json!({"ordinary":"real"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(child.workflow_task.as_ref().unwrap().task_index.is_none());
    tx.commit().await.unwrap();
    drop(activation);
    let mut tx = db.begin().await.unwrap();
    let before = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    let rejected =
        Success::prepare_success_output(&mut tx, root.id, workflow.id, child.id, &plan, KEY).await;
    let denied = matches!(rejected, Err(attune_common::Error::PermissionDenied(_)));
    if let Err(error) = &rejected {
        eprintln!("mixed root source admission: {error}");
    }
    drop(rejected);
    drop(plan);
    tx.commit().await.unwrap();
    let current = ExecutionRepository::find_by_id(&*db, root.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.status, ExecutionStatus::Running);
    assert!(current.result.is_none());
    let after = Workflows::find_by_id(&*db, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    assert_eq!(
        ExecutionRepository::find_by_parent(&*db, root.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let releases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_success_metadata_release WHERE execution=$1",
    )
    .bind(root.id)
    .fetch_one(&*db)
    .await
    .unwrap();
    assert_eq!(releases, 0);
    db.cleanup().await.unwrap();
    assert!(denied, "mixed root preparation accepted an archive whose final leaves and generated ciphertext exceed the reader budget");
}

#[tokio::test]
async fn native_ordinary_iteration_empty_root_budget_near_boundary_cold_replay_and_custody() {
    let (db, root, _, human, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        EmptyProgram::TerminalInheritedBudget(9, 31 * 1024),
    )
    .await;
    let writer = empty_root_custody::writer(&db, &root, human).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let applied = plan.apply(&mut tx, KEY).await.unwrap();
    assert!(applied.requested.is_empty());
    assert_eq!(
        applied.completed_parent.unwrap().status,
        ExecutionStatus::Completed
    );
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    drop(plan);
    let mut cold = db.begin().await.unwrap();
    let replay = Iteration::prepare_empty_root_replay(&mut cold, root.id, KEY)
        .await
        .unwrap();
    replay.ensure_fresh_on(&mut cold).await.unwrap();
    let fields = Secrets::find_stored_by_entity(&mut *cold, "execution_result", root.id)
        .await
        .unwrap();
    let (ordinary, receipts) =
        attune_common::repositories::immutable_input_origin::prepare_input_receipts(
            &mut cold,
            "execution_result",
            root.id,
            &fields,
            KEY,
        )
        .await
        .unwrap();
    assert!(ordinary.is_empty());
    assert_eq!(receipts.len(), 1);
    let value = attune_common::crypto::decrypt_json(&fields[0].encrypted_value, KEY).unwrap();
    assert_eq!(value["field_00"].as_str().unwrap().len(), 31 * 1024);
    assert!(ExecutionRepository::find_by_parent(&mut *cold, root.id)
        .await
        .unwrap()
        .is_empty());
    // This lower bound deliberately excludes all ancestry: the accepted case
    // really is near the byte boundary, not a tiny-input smoke test.
    let (snapshot, recipe): (Id, Value) = sqlx::query_as(
        "SELECT snapshot,recipe FROM runtime_native_empty_root_release WHERE execution=$1",
    )
    .bind(root.id)
    .fetch_one(&mut *cold)
    .await
    .unwrap();
    let stored: Vec<(Value, Value)> = sqlx::query_as("SELECT proof,encrypted_value FROM runtime_input_receipt WHERE execution=$1 AND entity_kind='execution_result'")
        .bind(root.id).fetch_all(&mut *cold).await.unwrap();
    let bytes = stored
        .iter()
        .map(|(proof, value)| {
            serde_json::to_vec(proof).unwrap().len() + serde_json::to_vec(value).unwrap().len()
        })
        .sum::<usize>()
        + serde_json::to_vec(&recipe).unwrap().len();
    eprintln!("empty root publication lower bound: {bytes} bytes before deduplicated CONTROL/input ancestry");
    assert!(bytes > 800 * 1024);
    assert!(bytes < 1024 * 1024);
    let (shared, uses): (Id, i64) = sqlx::query_as("SELECT edge.source_receipt,count(*) FROM runtime_input_receipt_source edge JOIN runtime_input_receipt leaf ON leaf.id=edge.receipt WHERE leaf.execution=$1 AND leaf.entity_kind='execution_result' AND leaf.destination_path<>'' GROUP BY edge.source_receipt ORDER BY count(*) DESC LIMIT 1")
        .bind(root.id).fetch_one(&mut *cold).await.unwrap();
    assert_eq!(
        uses, 9,
        "all real leaf receipts retain the same original workflow variable ancestor"
    );
    let (proof, encrypted): (Value, Value) =
        sqlx::query_as("SELECT proof,encrypted_value FROM runtime_input_receipt WHERE id=$1")
            .bind(shared)
            .fetch_one(&mut *cold)
            .await
            .unwrap();
    let shared_bytes =
        serde_json::to_vec(&proof).unwrap().len() + serde_json::to_vec(&encrypted).unwrap().len();
    assert!(bytes + shared_bytes * uses as usize > 1024 * 1024,
        "the positive must require ancestry deduplication, not merely fit an overcounted reservation");
    cold.commit().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store =
        attune_common::blob_store::FilesystemBlobStore::new(directory.path().join("objects"))
            .unwrap();
    let held = super::super::file_native_retention_cases::hold_wait_output(
        &db,
        &writer,
        human,
        &root
            .executable_snapshot
            .as_ref()
            .unwrap()
            .executable
            .action
            .pack_ref,
        KEY,
        &receipts[0],
        directory.path(),
        &store,
    )
    .await;
    empty_root_custody::settle(&db, held, directory.path(), &store, snapshot).await;
    directory.close().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_empty_root_budget_rejects_aggregate_before_publication() {
    let (db, root, workflow, _, _) = setup_graph_contract(
        json!([]),
        None,
        false,
        0,
        false,
        Some((false, false)),
        false,
        EmptyProgram::TerminalBudget(12, 48 * 1024),
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut target_lock = db.begin().await.unwrap();
    // Root source preparation legitimately takes a retained-admission SHARE
    // lock. Hold the distinct workflow publication target, not that source lock.
    sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
        .bind(workflow.id)
        .execute(&mut *target_lock)
        .await
        .unwrap();
    let rejected = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Iteration::prepare(&mut tx, &root, "ordinary", None, KEY),
    )
    .await
    .expect("overbudget admission must finish before acquiring the terminal workflow target lock");
    let denied = matches!(rejected, Err(attune_common::Error::PermissionDenied(_)));
    // Commit even a denied preparation, so rollback cannot hide partial root
    // publication. Source-phase private CONTROL staging may remain in this tx.
    tx.commit().await.unwrap();
    target_lock.rollback().await.unwrap();
    let current = ExecutionRepository::find_by_id(&*db, root.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.status, ExecutionStatus::Running);
    assert!(current.result.is_none());
    assert_eq!(
        Workflows::find_by_id(&*db, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    assert!(ExecutionRepository::find_by_parent(&*db, root.id)
        .await
        .unwrap()
        .is_empty());
    assert!(
        Secrets::find_stored_by_entity(&*db, "execution_result", root.id)
            .await
            .unwrap()
            .is_empty()
    );
    let releases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runtime_native_empty_root_release WHERE execution=$1",
    )
    .bind(root.id)
    .fetch_one(&*db)
    .await
    .unwrap();
    assert_eq!(releases, 0);
    db.cleanup().await.unwrap();
    assert!(denied, "dedicated empty-root source preparation accepted twelve 48KiB leaves whose root and leaf ciphertexts alone exceed the cold reader budget");
}
