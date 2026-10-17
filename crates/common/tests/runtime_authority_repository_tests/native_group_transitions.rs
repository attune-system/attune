//! Actual D group -> A lifecycle proof -> B canonical next/publication consumer.
use super::native_cache_checkpoint::Case;
use super::*;
use attune_common::{
    models::{Execution, WorkflowExecution},
    repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        declared_input::DeclaredInputRepository,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::{InheritedInputRepository as Inputs, InputSitePurpose},
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        native_iteration_materialization::NativeIterationMaterializationRepository as Iteration,
        native_transition_materialization::{
            NativeTransitionMaterializationRepository as Transitions, NativeTransitionRenderFrame,
        },
        workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
        Update,
    },
    test_database::TestDatabase,
};
use std::sync::Arc;

#[tokio::test]
async fn nonempty_group_next_uses_real_condition_and_ordered_pending_overwrites() {
    super::run_native_cache_fixture(Some(Case::GroupNext)).await;
}
#[tokio::test]
async fn failed_sibling_with_successful_last_delivery_selects_group_failed_next() {
    super::run_native_cache_fixture(Some(Case::GroupFailedNext)).await;
}
#[tokio::test]
async fn group_member_change_after_publication_denies_held_frame_effects() {
    super::run_native_cache_fixture(Some(Case::GroupChanged)).await;
}

pub(super) async fn check(
    case: Case,
    db: &TestDatabase,
    root: &Execution,
    workflow: &WorkflowExecution,
    key: &str,
) {
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut initial = Iteration::prepare(&mut tx, root, "cached", None, key)
        .await
        .unwrap();
    let created = initial.apply(&mut tx, key).await.unwrap();
    assert_eq!(created.requested.len(), 2);
    initial.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    initial.ensure_fresh().unwrap();
    let mut tx = db.begin().await.unwrap();
    for (ordinal, child) in created.requested.iter().enumerate() {
        ExecutionRepository::update(
            &mut *tx,
            child.id,
            UpdateExecutionInput {
                status: Some(if case == Case::GroupFailedNext && ordinal == 0 {
                    ExecutionStatus::Failed
                } else {
                    ExecutionStatus::Completed
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let d = Arc::new(
        Iteration::prepare_completion(&mut tx, root.id, "cached", key)
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(d.ordinal_count(), 2);
    let frame = Transitions::prepare_iteration_successors(&mut tx, d.clone(), key)
        .await
        .unwrap()
        .unwrap();
    let name = if case == Case::GroupFailedNext {
        "after_failed"
    } else {
        "after"
    };
    assert_eq!(frame.selected_targets(), &[name.to_owned()]);
    let evidence = Snapshots::storage_evidence(&mut tx, frame.group().snapshot())
        .await
        .unwrap();
    assert_eq!(
        evidence.metadata().source_kind(),
        "ordinary_iteration_outcome"
    );
    assert!(evidence.iteration_outcome().is_some());
    let path = format!("{}/with_items", frame.target_path(name).unwrap());
    let binding = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap()
        .into_iter()
        .find(|binding| binding.source_path == path)
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &binding.declaration_revision,
        &path,
        &binding.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    frame
        .bind(&mut tx, &mut session, &binding, key)
        .await
        .unwrap();
    let mut list = Inputs::evaluate_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    frame.attach_rendered_control(&mut list).unwrap();
    assert_eq!(
        list.value(),
        &json!([{"token":if case == Case::GroupFailedNext { "failed" } else { "original" }}])
    );
    let mut literal = Inputs::literal(
        &mut tx,
        root.id,
        &binding.declaration_revision,
        &format!("{}/params/token/token", frame.target_path(name).unwrap()),
    )
    .await
    .unwrap();
    frame.attach_literal(&mut literal).unwrap();
    assert!(!literal.selections().is_empty());
    assert!(frame.require_published(&mut tx).await.is_err());
    let actual = Workflows::find_by_id_for_update(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    let recorded = frame.record(&mut tx, &actual, key).await.unwrap();
    let expected_chosen = if case == Case::GroupFailedNext {
        "failed"
    } else {
        "overwrite"
    };
    assert_eq!(
        recorded.variables["chosen"],
        attune_common::secret_values::redaction_marker()
    );
    Secrets::replace_entity_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
        root.id,
        &recorded.secrets,
    )
    .await
    .unwrap();
    let stored = Secrets::find_stored_by_entity(
        &mut *tx,
        attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
        root.id,
    )
    .await
    .unwrap();
    let restored = attune_common::secret_values::restore_secret_values(
        recorded.variables.clone(),
        &stored,
        key,
    )
    .unwrap();
    assert_eq!(restored["chosen"], json!([{"token":expected_chosen}]));
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(recorded.variables),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        d.require_source_current(&mut tx).await.is_err(),
        "full original buffers are intentionally pre-publication only"
    );
    frame.require_members_current(&mut tx).await.unwrap();
    frame.require_published(&mut tx).await.unwrap();
    assert!(frame.record(&mut tx, &actual, key).await.is_err());
    if case == Case::GroupChanged {
        ExecutionRepository::update(
            &mut *tx,
            created.requested[1].id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Failed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(frame.require_members_current(&mut tx).await.is_err());
        assert!(frame.require_published(&mut tx).await.is_err());
        tx.rollback().await.unwrap();
    } else {
        frame.ensure_fresh_on(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        frame.ensure_fresh().unwrap();
    }
    // D owns turning the prepared list into actual successor children. This test
    // claims B's real source/condition/publication consumer, not that future driver.
    assert_eq!(
        ExecutionRepository::find_by_parent(db.pool(), root.id)
            .await
            .unwrap()
            .len(),
        2
    );
}
