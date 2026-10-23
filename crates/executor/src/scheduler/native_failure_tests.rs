//! Actual prepared-advance caller, with the same canonical admitted H fixture.
use super::*;
#[path = "../../../common/tests/native_failure_materialization_tests.rs"]
mod fixture;
mod prospective_result;
mod receipt_deadline;
mod success_recipe;
use attune_common::repositories::{
    cache::{CacheEntryRepository, CacheTransactionMode},
    native_failure_materialization::NativeFailureMaterializationRepository as Failure,
    native_input_origin::NativeInputOriginRepository as Native,
    native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
};

fn canonical_task_graph(value: &serde_json::Value) -> serde_json::Value {
    let definition =
        attune_common::workflow::parse_workflow_yaml(&serde_yaml_ng::to_string(value).unwrap())
            .unwrap();
    serde_json::to_value(TaskGraph::from_workflow(&definition).unwrap()).unwrap()
}

async fn complete(status: ExecutionStatus) {
    let (db, files, parent, workflow, child) =
        fixture::original_executor_failure(status, false, canonical_task_graph).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let state = Failure::prepare_for_completion(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    let transitions = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap();
    assert!(
        transitions.is_none(),
        "an empty task with no output has no fabricated transition plan"
    );
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY))
        .await
        .unwrap();
    let output = Failure::prepare_output(&mut tx, &state, &[], fixture::KEY)
        .await
        .unwrap();
    let advance = ExecutionScheduler::advance_workflow_prepared(
        &mut tx,
        &AtomicUsize::new(0),
        Some(fixture::KEY),
        &child,
        &SchedulerMetadataCaches::new(),
        transitions.as_ref(),
        Some(&sources),
        None,
        Some(&state),
        Some(&output),
        None,
    )
    .await
    .unwrap();
    assert!(advance.execution_requests.is_empty());
    let actual = ExecutionRepository::find_by_id(&mut *tx, parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, ExecutionStatus::Failed);
    let redacted = actual.result.unwrap();
    assert!(attune_common::secret_values::is_redaction_marker(&redacted));
    let fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_RESULT,
        parent.id,
    )
    .await
    .unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].json_path, "");
    // Ciphertext byte check, not a current-reader/GC acceptance claim.
    assert_eq!(
        attune_common::secret_values::restore_secret_values(redacted, &fields, fixture::KEY)
            .unwrap(),
        state.metadata().clone()
    );
    let final_workflow = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_workflow.status, ExecutionStatus::Failed);
    assert_eq!(final_workflow.failed_tasks, vec!["next"]);
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_completion_release WHERE execution=$1",
    )
    .bind(parent.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        releases, 1,
        "actual all_done must publish the genuine completion release"
    );
    output.ensure_fresh_on(&mut tx).await.unwrap();
    state.ensure_fresh_on(&mut tx).await.unwrap();
    sources.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    output.ensure_fresh().unwrap();
    state.ensure_fresh().unwrap();
    sources.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_failed_all_done_records_a_genuine_protected_recipe_before_parent_completion() {
    complete(ExecutionStatus::Failed).await;
}

#[tokio::test]
async fn ordinary_timeout_all_done_records_the_actual_timeout_failure_recipe() {
    complete(ExecutionStatus::Timeout).await;
}

#[tokio::test]
async fn ordinary_failure_recording_without_the_original_target_guard_is_denied() {
    let (db, files, parent, workflow, child) =
        fixture::original_executor_failure(ExecutionStatus::Failed, false, canonical_task_graph)
            .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let state = Failure::prepare_for_completion(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    let output = Failure::prepare_output(&mut tx, &state, &[], fixture::KEY)
        .await
        .unwrap();
    assert!(output
        .record(&mut tx, state.failed_tasks(), fixture::KEY)
        .await
        .is_err());
    assert_eq!(
        ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        parent.status
    );
    assert_eq!(
        WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .failed_tasks,
        workflow.failed_tasks
    );
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_completion_release WHERE execution=$1",
    )
    .bind(parent.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(releases, 0);
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_failure_false_control_expiring_at_the_actual_workflow_target_rolls_back() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository,
        inherited_input::InheritedInputRepository as Inputs,
    };
    let (db, files, parent, workflow, child) =
        fixture::original_executor_failure(ExecutionStatus::Failed, true, canonical_task_graph)
            .await;
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
    let deadline = Utc::now() + chrono::Duration::seconds(10);
    Inputs::approve_external_input(
        &mut tx, parent.id, &binding.declaration_revision, &binding.source_path, &binding.expression,
        parent.executor.unwrap(),
        serde_json::from_value(serde_json::json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.native_failure_decision"]}}])).unwrap(),
        Some(deadline),
    ).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let state = Failure::prepare_for_completion(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    let transitions = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(!transitions.fires(0).unwrap());
    assert_eq!(transitions.completion_decisions().len(), 1);
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY))
        .await
        .unwrap();
    let context = ExecutionScheduler::prepare_native_transition_context(
        &mut tx,
        &transitions,
        &sources,
        Some(fixture::KEY),
    )
    .await
    .unwrap();
    let output = Failure::prepare_output(
        &mut tx,
        &state,
        &transitions.completion_decisions(),
        fixture::KEY,
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
        let limit = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
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
            assert!(
                tokio::time::Instant::now() < limit,
                "must observe the actual workflow-row wait through expiry"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        ExecutionScheduler::advance_workflow_prepared(
            &mut tx,
            &AtomicUsize::new(0),
            Some(fixture::KEY),
            &child,
            &SchedulerMetadataCaches::new(),
            Some(&transitions),
            Some(&sources),
            Some(&context),
            Some(&state),
            Some(&output),
            None,
        ),
    )
    .await;
    release.await.unwrap();
    assert!(result.unwrap().is_err());
    assert!(output.ensure_fresh_on(&mut tx).await.is_err());
    assert!(output.ensure_fresh().is_err());
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_completion_release WHERE execution=$1",
    )
    .bind(parent.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(releases, 0);
    tx.rollback().await.unwrap();
    let actual = ExecutionRepository::find_by_id(db.pool(), parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, parent.status);
    assert_eq!(actual.result, parent.result);
    assert_eq!(
        WorkflowExecutionRepository::find_by_id(db.pool(), workflow.id)
            .await
            .unwrap()
            .unwrap()
            .failed_tasks,
        workflow.failed_tasks
    );
    assert!(ExecutionSecretValueRepository::find_stored_by_entity(
        db.pool(),
        ENTITY_EXECUTION_RESULT,
        parent.id
    )
    .await
    .unwrap()
    .is_empty());
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

async fn complete_native_result(case: fixture::NativeResultCase) {
    use fixture::NativeResultCase;
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
    let canonical = plan.native_result_view(child.id).unwrap().clone();
    let success = Failure::prepare_success_output(
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
    if matches!(case, NativeResultCase::ConditionalWholeResult) {
        assert!(!plan.fires(0).unwrap());
        assert!(plan.fires(1).unwrap());
        assert_eq!(plan.completion_decisions().len(), 2);
    }
    if matches!(case, NativeResultCase::LegacyWholeResult) {
        assert!(plan.fires(0).unwrap());
        assert!(
            plan.completion_decisions().is_empty(),
            "derived legacy succeeded gate is not a fabricated authored Condition"
        );
    }
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
        Some(&success),
    )
    .await
    .unwrap();
    assert!(outcome.execution_requests.is_empty());
    let actual = ExecutionRepository::find_by_id(&mut *tx, parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, ExecutionStatus::Completed);
    let fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_RESULT,
        parent.id,
    )
    .await
    .unwrap();
    let output = attune_common::secret_values::restore_secret_values(
        actual.result.unwrap(),
        &fields,
        fixture::KEY,
    )
    .unwrap();
    assert_eq!(output["succeeded"], serde_json::json!(true));
    assert!(output.get("output_validation_warning").is_none());
    let literal = serde_json::json!("{{ keystore['foreign.never_parse'] }}");
    match case {
        NativeResultCase::StdoutOutput => {
            assert_eq!(output["value"], literal);
            assert_eq!(canonical["token"], literal);
            assert_eq!(
                canonical["stdout"],
                child.result.as_ref().unwrap()["stdout"]
            );
        }
        NativeResultCase::DataOutput => {
            assert_eq!(output["value"], literal);
            assert_eq!(output["collision"], serde_json::json!("envelope_wins"));
            assert_eq!(
                canonical["data"]["token"],
                serde_json::json!("data_must_not_overwrite_envelope")
            );
        }
        NativeResultCase::WholeResult
        | NativeResultCase::ConditionalWholeResult
        | NativeResultCase::LegacyWholeResult
        | NativeResultCase::ExplicitNextOverridesLegacy => {
            assert_eq!(output["echo"], literal);
            let actual_workflow = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap();
            let variable_fields = ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *tx,
                attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
                parent.id,
            )
            .await
            .unwrap();
            let variables = attune_common::secret_values::restore_secret_values(
                actual_workflow.variables,
                &variable_fields,
                fixture::KEY,
            )
            .unwrap();
            assert_eq!(variables["whole"], canonical);
            assert_ne!(
                variables["whole"], canonical["whole"],
                "shorthand is never result[name]"
            );
            assert!(variables.get("not_fired").is_none());
            assert!(variables.get("ignored").is_none());
            let proof: serde_json::Value = sqlx::query_scalar("SELECT proof FROM runtime_input_receipt WHERE execution=$1 AND entity_kind='workflow_variables' AND destination_path='/whole' AND proof->'native'->>'recipe'='result_shorthand'")
                .bind(parent.id).fetch_one(&mut *tx).await.unwrap();
            let expected = if matches!(case, NativeResultCase::LegacyWholeResult) {
                "/workflow/tasks/0/publish/0"
            } else if matches!(case, NativeResultCase::ConditionalWholeResult) {
                "/workflow/tasks/0/next/1/publish/0"
            } else {
                "/workflow/tasks/0/next/0/publish/0"
            };
            assert_eq!(
                proof["native"]["directive_path"],
                serde_json::json!(expected)
            );
            assert!(proof
                .get("declaration")
                .is_none_or(serde_json::Value::is_null));
            assert!(proof.get("literal").is_none_or(serde_json::Value::is_null));
        }
        NativeResultCase::DependentShorthand => {
            let actual_workflow = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap();
            let fields = ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *tx,
                attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
                parent.id,
            )
            .await
            .unwrap();
            let variables = attune_common::secret_values::restore_secret_values(
                actual_workflow.variables,
                &fields,
                fixture::KEY,
            )
            .unwrap();
            assert_eq!(variables["whole"], canonical);
            assert_eq!(variables["copied"], canonical);
        }
        NativeResultCase::DefaultGeneratedSuccess
        | NativeResultCase::FalseDecisionGeneratedSuccess
        | NativeResultCase::GeneratedWarningCandidate
        | NativeResultCase::ProspectiveResultOutput
        | NativeResultCase::ProspectiveResultChild
        | NativeResultCase::ProspectiveCompositeChild
        | NativeResultCase::IndependentRecoveryChild
        | NativeResultCase::FiniteOriginalInputChild
        | NativeResultCase::FiniteOriginalPublicationPair => {
            unreachable!("negative case uses source-denial test")
        }
        NativeResultCase::AuthoredFalseStatus | NativeResultCase::AuthoredFalseWithControls => {
            unreachable!("dedicated authored status recipe case")
        }
    }
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_result_view_release WHERE source_execution=$1",
    )
    .bind(child.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        releases, 1,
        "shorthand and output share one actual ResultView release"
    );
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    success.ensure_fresh_on(&mut tx).await.unwrap();
    sources.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    success.ensure_fresh().unwrap();
    sources.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_shorthand_caller_publishes_the_entire_result_not_the_named_field() {
    complete_native_result(fixture::NativeResultCase::WholeResult).await;
}
#[tokio::test]
async fn ordinary_conditional_shorthand_caller_retains_false_and_true_real_decisions() {
    complete_native_result(fixture::NativeResultCase::ConditionalWholeResult).await;
}
#[tokio::test]
async fn ordinary_stdout_output_caller_decodes_data_once_and_keeps_template_strings_literal() {
    complete_native_result(fixture::NativeResultCase::StdoutOutput).await;
}
#[tokio::test]
async fn ordinary_data_output_caller_preserves_envelope_collisions_and_ignores_stdout_aliases() {
    complete_native_result(fixture::NativeResultCase::DataOutput).await;
}
#[tokio::test]
async fn ordinary_shorthand_uses_the_actual_private_stage_for_a_dependent_publication() {
    complete_native_result(fixture::NativeResultCase::DependentShorthand).await;
}

#[tokio::test]
async fn ordinary_legacy_task_publish_uses_canonical_normalization_and_original_directive() {
    complete_native_result(fixture::NativeResultCase::LegacyWholeResult).await;
}

#[tokio::test]
async fn ordinary_explicit_next_does_not_publish_ignored_legacy_task_directives() {
    complete_native_result(fixture::NativeResultCase::ExplicitNextOverridesLegacy).await;
}
