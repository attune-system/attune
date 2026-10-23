//! Reuse H's actual admitted executions, not caller-constructed success evidence.
#[path = "native_failure_materialization_tests.rs"]
// H also exposes executor-only fixture cases that this repository target does not call.
#[allow(dead_code)]
mod source;

use attune_common::{
    models::ExecutionStatus,
    repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        inherited_input::InheritedInputRepository as Inputs,
        native_failure_materialization::NativeFailureMaterializationRepository as State,
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
        workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
        FindById, Update,
    },
};

async fn recipe(case: source::NativeResultCase, warning: bool) {
    let (db, files, parent, workflow, child) = source::original_success_source(case).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, source::KEY)
        .await
        .unwrap()
        .unwrap();
    let state = State::prepare_native_success(&mut tx, parent.id, workflow.id, child.id, &plan)
        .await
        .unwrap()
        .unwrap();
    let prepared = Inputs::prepare_native_success_metadata(&mut tx, &state, source::KEY)
        .await
        .unwrap();
    let unguarded = Inputs::prepare_native_success_metadata(&mut tx, &state, source::KEY)
        .await
        .unwrap();
    assert_eq!(prepared.value()["succeeded"], true);
    assert_eq!(
        prepared.value().get("output_validation_warning").is_some(),
        warning
    );
    // A genuinely different original row cannot unlock publication.
    let mut wrong = workflow.clone();
    wrong.current_tasks.clear();
    wrong.status = ExecutionStatus::Failed;
    assert!(prepared.require_original_rows(&parent, &wrong).is_err());
    prepared.require_original_rows(&parent, &workflow).unwrap();
    plan.record(&mut tx, &workflow, source::KEY).await.unwrap();
    let projected = Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            completed_tasks: Some(state.projected_completed_tasks().to_vec()),
            current_tasks: Some(Vec::new()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let value = prepared.value().clone();
    assert!(Inputs::record_native_success_metadata(
        &mut tx,
        &state,
        &unguarded,
        &projected,
        &value,
        source::KEY
    )
    .await
    .is_err());
    let mut invented = value.clone();
    invented["succeeded"] = serde_json::json!(false);
    assert!(Inputs::record_native_success_metadata(
        &mut tx,
        &state,
        &prepared,
        &projected,
        &invented,
        source::KEY
    )
    .await
    .is_err());
    let receipt = Inputs::record_native_success_metadata(
        &mut tx,
        &state,
        &prepared,
        &projected,
        &value,
        source::KEY,
    )
    .await
    .unwrap();
    assert_eq!(receipt.site().execution, parent.id);
    assert!(receipt.site().path.is_empty());
    let metadata = receipt
        .native_source_metadata()
        .iter()
        .find(|metadata| metadata.source_kind() == "ordinary_success_metadata")
        .unwrap();
    let storage = Snapshots::storage_evidence(&mut tx, metadata.snapshot())
        .await
        .unwrap();
    let evidence = storage.success_metadata().unwrap();
    assert_eq!(evidence.completed_execution(), child.id);
    assert_eq!(evidence.children().len(), 1);
    assert_eq!(evidence.output_leaves().len(), usize::from(warning));
    if matches!(
        case,
        source::NativeResultCase::FalseDecisionGeneratedSuccess
    ) {
        assert!(!evidence.controls().is_empty());
        assert!(evidence.controls().iter().all(|(_, site)| site.purpose
            != attune_common::repositories::inherited_input::InputSitePurpose::Value));
    }
    let replay = Snapshots::source_evidence(&mut tx, metadata, source::KEY)
        .await
        .unwrap();
    assert_eq!(
        replay.success_metadata().unwrap().original_contract(),
        state.output_contract()
    );
    assert_eq!(prepared.secret_path_sources(&receipt).unwrap().len(), 1);
    assert_eq!(
        Workflows::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    assert!(Inputs::record_native_success_metadata(
        &mut tx,
        &state,
        &prepared,
        &projected,
        &value,
        source::KEY
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}
#[tokio::test]
async fn success_recipe_records_generated_default_without_an_authored_status_leaf() {
    recipe(source::NativeResultCase::DefaultGeneratedSuccess, false).await;
}
#[tokio::test]
async fn success_recipe_records_canonical_original_schema_warning_without_a_warning_dto() {
    recipe(source::NativeResultCase::GeneratedWarningCandidate, true).await;
}
#[tokio::test]
async fn success_recipe_keeps_genuine_false_conditions_as_controls() {
    recipe(
        source::NativeResultCase::FalseDecisionGeneratedSuccess,
        false,
    )
    .await;
}
