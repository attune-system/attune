//! Actual ordinary all-done caller using the genuine A/B/H success owners.
use super::*;
use fixture::NativeResultCase;

#[tokio::test]
async fn ordinary_generated_success_caller_records_zero_authored_output_leaves() {
    complete_success(NativeResultCase::DefaultGeneratedSuccess).await;
}
#[tokio::test]
async fn ordinary_success_caller_records_canonical_original_contract_warning() {
    complete_success(NativeResultCase::GeneratedWarningCandidate).await;
}
#[tokio::test]
async fn ordinary_success_caller_preserves_the_actual_authored_false_status() {
    complete_success(NativeResultCase::AuthoredFalseStatus).await;
}

#[tokio::test]
async fn ordinary_success_caller_preserves_authored_false_and_both_control_decisions() {
    complete_success(NativeResultCase::AuthoredFalseWithControls).await;
}

async fn complete_success(case: NativeResultCase) {
    let (db, files, parent, workflow, child) =
        fixture::original_executor_native_result(case, canonical_task_graph).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    let output = Failure::prepare_success_output(
        &mut tx,
        parent.id,
        workflow.id,
        child.id,
        &plan,
        fixture::KEY,
    )
    .await
    .unwrap()
    .unwrap();
    let expected = output.value().clone();
    let graph = &parent
        .executable_snapshot
        .as_ref()
        .unwrap()
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition;
    let leaves = plan.prepared_output_leaves().count();
    match case {
        NativeResultCase::DefaultGeneratedSuccess => {
            assert!(graph.get("output_map").is_none());
            assert_eq!(leaves, 0);
            assert_eq!(expected, serde_json::json!({"succeeded":true}));
        }
        NativeResultCase::GeneratedWarningCandidate => {
            assert!(graph["output_map"].get("succeeded").is_none());
            assert!(graph["output_map"]
                .get("output_validation_warning")
                .is_none());
            assert_eq!(leaves, 1);
            assert_eq!(
                expected["value"],
                serde_json::json!("{{ keystore['foreign.never_parse'] }}")
            );
            let mut candidate = expected.clone();
            candidate
                .as_object_mut()
                .unwrap()
                .remove("output_validation_warning");
            let contract = plan.prepared_output_contract().unwrap();
            let diagnostic = attune_common::value_schema::CompiledContract::new(contract)
                .unwrap()
                .validate(&candidate)
                .unwrap_err();
            assert_eq!(
                expected["output_validation_warning"],
                serde_json::json!({"code":diagnostic.code,"path":diagnostic.path})
            );
        }
        NativeResultCase::AuthoredFalseStatus | NativeResultCase::AuthoredFalseWithControls => {
            assert_eq!(
                graph["output_map"]["succeeded"],
                serde_json::json!("{{ false }}")
            );
            assert_eq!(leaves, 2);
            assert_eq!(expected["succeeded"], serde_json::json!(false));
            if matches!(case, NativeResultCase::AuthoredFalseWithControls) {
                assert_eq!(plan.completion_decisions().len(), 2);
                assert!(!plan.fires(0).unwrap());
                assert!(plan.fires(1).unwrap());
            }
        }
        _ => unreachable!("finite source recipe cases only"),
    }
    // Match production's guard before logging and its repeated pure comparison
    // at advance entry. A's own one-shot marker must not be minted twice.
    output.require_original_rows(&parent, &workflow).unwrap();
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY))
        .await
        .unwrap();
    let context = ExecutionScheduler::prepare_native_transition_context(
        &mut tx,
        &plan,
        &sources,
        Some(fixture::KEY),
    )
    .await
    .unwrap();
    let outcome = ExecutionScheduler::advance_workflow_prepared(
        &mut tx,
        &AtomicUsize::new(0),
        Some(fixture::KEY),
        &child,
        &SchedulerMetadataCaches::new(),
        Some(&plan),
        Some(&sources),
        Some(&context),
        None,
        None,
        Some(&output),
    )
    .await
    .unwrap();
    assert!(outcome.execution_requests.is_empty());
    let actual = ExecutionRepository::find_by_id(&mut *tx, parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, ExecutionStatus::Completed);
    assert!(attune_common::secret_values::is_redaction_marker(
        actual.result.as_ref().unwrap()
    ));
    let fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_RESULT,
        parent.id,
    )
    .await
    .unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(
        attune_common::secret_values::restore_secret_values(
            actual.result.unwrap(),
            &fields,
            fixture::KEY
        )
        .unwrap(),
        expected
    );
    let state = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ExecutionStatus::Completed);
    assert_eq!(state.completed_tasks, vec!["next"]);
    let release: (i64, i64, bool) = sqlx::query_as("SELECT workflow,completed_execution,release_transaction=pg_current_xact_id()::TEXT FROM runtime_native_success_metadata_release WHERE execution=$1").bind(parent.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(release, (workflow.id, child.id, true));
    let proof: JsonValue = sqlx::query_scalar("SELECT proof FROM runtime_input_receipt WHERE execution=$1 AND entity_kind='execution_result' AND destination_purpose='value' AND destination_path=''").bind(parent.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        proof["native"]["recipe"],
        serde_json::json!("ordinary_success")
    );
    let recorded_leaves: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_input_receipt WHERE execution=$1 AND entity_kind='execution_result' AND destination_purpose='value' AND destination_path<>''").bind(parent.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        recorded_leaves, leaves as i64,
        "A writes each actual B leaf once; the legacy B output recorder is not also called"
    );
    output.ensure_fresh_on(&mut tx).await.unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    sources.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    output.ensure_fresh().unwrap();
    plan.ensure_fresh().unwrap();
    sources.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_success_caller_rejects_changed_original_target_and_rolls_back() {
    let (db, files, parent, workflow, child) = fixture::original_executor_native_result(
        NativeResultCase::DefaultGeneratedSuccess,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    let output = Failure::prepare_success_output(
        &mut tx,
        parent.id,
        workflow.id,
        child.id,
        &plan,
        fixture::KEY,
    )
    .await
    .unwrap()
    .unwrap();
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY))
        .await
        .unwrap();
    let context = ExecutionScheduler::prepare_native_transition_context(
        &mut tx,
        &plan,
        &sources,
        Some(fixture::KEY),
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        parent.id,
        UpdateExecutionInput {
            result: Some(serde_json::json!({"changed":"unrecorded target"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(ExecutionScheduler::advance_workflow_prepared(
        &mut tx,
        &AtomicUsize::new(0),
        Some(fixture::KEY),
        &child,
        &SchedulerMetadataCaches::new(),
        Some(&plan),
        Some(&sources),
        Some(&context),
        None,
        None,
        Some(&output),
    )
    .await
    .is_err());
    assert_no_release(&mut tx, parent.id).await;
    tx.rollback().await.unwrap();
    assert_original_target(db.pool(), &parent, &workflow).await;
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_success_caller_rejects_a_different_genuine_plan_publication() {
    let (db, files, parent, workflow, child) = fixture::original_executor_native_result(
        NativeResultCase::DefaultGeneratedSuccess,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let original = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    let output = Failure::prepare_success_output(
        &mut tx,
        parent.id,
        workflow.id,
        child.id,
        &original,
        fixture::KEY,
    )
    .await
    .unwrap()
    .unwrap();
    let other = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY))
        .await
        .unwrap();
    let context = ExecutionScheduler::prepare_native_transition_context(
        &mut tx,
        &other,
        &sources,
        Some(fixture::KEY),
    )
    .await
    .unwrap();
    // Both are genuine owning plans for the actual child. Publishing one is not
    // the same as publishing the exact plan borrowed by H/A's source token.
    assert!(ExecutionScheduler::advance_workflow_prepared(
        &mut tx,
        &AtomicUsize::new(0),
        Some(fixture::KEY),
        &child,
        &SchedulerMetadataCaches::new(),
        Some(&other),
        Some(&sources),
        Some(&context),
        None,
        None,
        Some(&output),
    )
    .await
    .is_err());
    let projected = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected.status, ExecutionStatus::Running);
    assert_eq!(
        projected.completed_tasks,
        vec!["next"],
        "the wrong-plan rejection must occur after the actual B publication/task-list writes"
    );
    assert_no_release(&mut tx, parent.id).await;
    tx.rollback().await.unwrap();
    assert_original_target(db.pool(), &parent, &workflow).await;
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_success_false_control_expiring_at_the_real_target_wait_rolls_back() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository,
        inherited_input::InheritedInputRepository as Inputs,
    };
    let (db, files, parent, workflow, child) =
        fixture::original_executor_success_with_approval(canonical_task_graph).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let binding = DeclaredInputRepository::bindings(&mut tx, parent.id)
        .await
        .unwrap()
        .into_iter()
        .find(|binding| binding.source_path == "/workflow/tasks/0/next/0/when")
        .unwrap();
    let deadline = Utc::now() + chrono::Duration::seconds(15);
    Inputs::approve_external_input(
        &mut tx, parent.id, &binding.declaration_revision, &binding.source_path, &binding.expression, parent.executor.unwrap(),
        serde_json::from_value(serde_json::json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.native_failure_decision"]}}])).unwrap(),
        Some(deadline),
    ).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(!plan.fires(0).unwrap());
    assert_eq!(plan.completion_decisions().len(), 1);
    let output = Failure::prepare_success_output(
        &mut tx,
        parent.id,
        workflow.id,
        child.id,
        &plan,
        fixture::KEY,
    )
    .await
    .unwrap()
    .unwrap();
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY))
        .await
        .unwrap();
    let context = ExecutionScheduler::prepare_native_transition_context(
        &mut tx,
        &plan,
        &sources,
        Some(fixture::KEY),
    )
    .await
    .unwrap();
    output.ensure_fresh_on(&mut tx).await.unwrap();
    let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut blocker = db.begin().await.unwrap();
    WorkflowExecutionRepository::find_by_id_for_update(&mut *blocker, workflow.id)
        .await
        .unwrap()
        .unwrap();
    let release = tokio::spawn(async move {
        let limit = tokio::time::Instant::now() + Duration::from_secs(40);
        let mut observed = false;
        loop {
            observed |= sqlx::query_scalar::<_, bool>("SELECT cardinality(pg_blocking_pids($1))>0")
                .bind(waiter)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let expired: bool = sqlx::query_scalar("SELECT $1::TIMESTAMPTZ <= clock_timestamp()")
                .bind(deadline)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            if observed && expired {
                blocker.commit().await.unwrap();
                break;
            }
            assert!(tokio::time::Instant::now() < limit, "must observe actual workflow-row blocking through the original false-control expiry");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let outcome = tokio::time::timeout(
        Duration::from_secs(45),
        ExecutionScheduler::advance_workflow_prepared(
            &mut tx,
            &AtomicUsize::new(0),
            Some(fixture::KEY),
            &child,
            &SchedulerMetadataCaches::new(),
            Some(&plan),
            Some(&sources),
            Some(&context),
            None,
            None,
            Some(&output),
        ),
    )
    .await;
    release.await.unwrap();
    assert!(outcome.unwrap().is_err());
    assert!(output.ensure_fresh_on(&mut tx).await.is_err());
    assert!(output.ensure_fresh().is_err());
    assert_no_release(&mut tx, parent.id).await;
    tx.rollback().await.unwrap();
    assert_original_target(db.pool(), &parent, &workflow).await;
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

async fn assert_no_release(conn: &mut PgConnection, root: i64) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_success_metadata_release WHERE execution=$1",
    )
    .bind(root)
    .fetch_one(conn)
    .await
    .unwrap();
    assert_eq!(count, 0);
}
async fn assert_original_target(
    pool: &PgPool,
    parent: &Execution,
    workflow: &attune_common::models::WorkflowExecution,
) {
    let actual = ExecutionRepository::find_by_id(pool, parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.result, parent.result);
    assert_eq!(actual.status, parent.status);
    let actual = WorkflowExecutionRepository::find_by_id(pool, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, workflow.status);
    assert_eq!(actual.completed_tasks, workflow.completed_tasks);
    assert_eq!(actual.variables, workflow.variables);
    assert!(ExecutionSecretValueRepository::find_stored_by_entity(
        pool,
        ENTITY_EXECUTION_RESULT,
        parent.id
    )
    .await
    .unwrap()
    .is_empty());
}
