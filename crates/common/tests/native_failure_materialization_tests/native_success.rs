//! Real persisted admission/source preparation. No success receipt API is faked
//! and these are not final executor/reader acceptance cases.
use super::*;
use attune_common::{
    repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        inherited_input::InputSitePurpose,
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
        FindById,
    },
    value_schema::CompiledContract,
};

#[tokio::test]
async fn native_success_default_generated_state_has_no_authored_status_or_output_witness() {
    let (db, files, parent, workflow, child) =
        original_success_source(NativeResultCase::DefaultGeneratedSuccess).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(plan.output().is_none());
    assert_eq!(plan.prepared_output_leaves().count(), 0);
    let prepared =
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .unwrap()
            .unwrap();
    assert!(std::ptr::eq(prepared.output_plan(), &plan));
    assert_eq!(prepared.projected_status(), ExecutionStatus::Completed);
    assert_eq!(prepared.projected_completed_tasks(), &["next"]);
    assert!(prepared.projected_failed_tasks().is_empty());
    assert!(prepared.decisions().is_empty());
    assert_eq!(
        prepared.outcomes().next().unwrap().value("succeeded"),
        Some(json!(true))
    );
    assert_eq!(prepared.source_executions().next().unwrap().id, child.id);
    assert_eq!(
        prepared.original_task_path(child.id).unwrap(),
        "/workflow/tasks/0"
    );
    let original =
        CompiledContract::new(prepared.output_contract().output.as_ref().unwrap()).unwrap();
    assert!(
        original.declares_secrets(),
        "the valid original protected contract is retained, not replaced by a caller schema"
    );
    assert!(parent
        .executable_snapshot
        .as_ref()
        .unwrap()
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition
        .get("output_map")
        .is_none());
    prepared.require_original_rows(&parent, &workflow).unwrap();
    assert!(prepared.require_projected_workflow(&workflow).is_err());
    let mut comparison_only = workflow.clone();
    comparison_only.completed_tasks = prepared.projected_completed_tasks().to_vec();
    comparison_only.current_tasks.clear();
    prepared
        .require_projected_workflow(&comparison_only)
        .unwrap();
    prepared
        .require_original_child(
            &child,
            prepared.original_contract(child.id).unwrap(),
            prepared.original_result_fields(child.id).unwrap(),
        )
        .unwrap();
    prepared.require_current_on(&mut tx).await.unwrap();
    prepared.ensure_fresh_on(&mut tx).await.unwrap();
    assert_eq!(
        ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_completion_release WHERE execution=$1",
    )
    .bind(parent.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        releases, 0,
        "a source state cannot record a generated success recipe"
    );
    tx.commit().await.unwrap();
    prepared.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn native_success_retains_actual_invalid_output_leaves_and_original_validation_contract() {
    let (db, files, parent, workflow, child) =
        original_success_source(NativeResultCase::GeneratedWarningCandidate).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    let prepared =
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .unwrap()
            .unwrap();
    let leaves = prepared
        .output_plan()
        .prepared_output_leaves()
        .collect::<Vec<_>>();
    assert_eq!(leaves.len(), 1);
    assert_eq!(leaves[0].0, "/value");
    assert_eq!(
        leaves[0].1.value(),
        &json!("{{ keystore['foreign.never_parse'] }}")
    );
    let actual = prepared.output_plan().output().unwrap();
    assert!(actual.get("succeeded").is_none());
    assert!(actual.get("output_validation_warning").is_none());
    assert!(
        CompiledContract::new(prepared.output_contract().output.as_ref().unwrap())
            .unwrap()
            .validate(actual)
            .is_err()
    );
    // A will derive the real diagnostic from these actual opaque leaves and the
    // retained contract. H accepts/provides no diagnostic or replacement JSON.
    prepared.require_current_on(&mut tx).await.unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn native_success_keeps_real_false_decisions_as_controls_with_no_output_value() {
    let (db, files, parent, workflow, child) =
        original_success_source(NativeResultCase::FalseDecisionGeneratedSuccess).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(!plan.fires(0).unwrap());
    let prepared =
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .unwrap()
            .unwrap();
    let controls = prepared.decisions();
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0].value(), &json!(false));
    assert_eq!(controls[0].site().purpose, InputSitePurpose::Condition);
    assert_eq!(controls[0].site().path, "/workflow/tasks/0/next/0/when");
    assert_eq!(prepared.output_plan().prepared_output_leaves().count(), 0);
    prepared.ensure_fresh_on(&mut tx).await.unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn native_success_rejects_wrong_ids_and_changed_original_source_before_targets() {
    let (db, files, parent, workflow, child) =
        original_success_source(NativeResultCase::DefaultGeneratedSuccess).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(
        Failure::prepare_native_success(&mut tx, child.id, workflow.id, child.id, &plan)
            .await
            .is_err()
    );
    assert!(
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, parent.id, &plan)
            .await
            .is_err()
    );
    let prepared =
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .unwrap()
            .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            result: Some(json!({"data":{"literal":"changed after source"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(prepared.require_original_sources_on(&mut tx).await.is_err());
    assert!(
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn native_success_missing_original_schema_never_uses_the_current_publisher() {
    let (db, files, parent, workflow, child) =
        original_success_source(NativeResultCase::DefaultGeneratedSuccess).await;
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut tx)
        .await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "DELETE FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1",
    )
    .bind(child.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .is_err()
    );
    assert_eq!(
        ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn native_success_does_not_reissue_for_an_already_advanced_source() {
    use attune_common::repositories::workflow::UpdateWorkflowExecutionInput;
    let (db, files, parent, workflow, child) =
        original_success_source(NativeResultCase::DefaultGeneratedSuccess).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            completed_tasks: Some(vec!["next".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn native_success_cannot_be_issued_after_the_actual_plan_entered_target_phase() {
    let (db, files, parent, workflow, child) =
        original_success_source(NativeResultCase::DefaultGeneratedSuccess).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    plan.record(&mut tx, &workflow, KEY).await.unwrap();
    assert!(
        Failure::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}
