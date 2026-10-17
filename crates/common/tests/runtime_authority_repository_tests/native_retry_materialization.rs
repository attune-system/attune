//! Real admitted workflow task retries; no worker/broker or current-IAM fallback.
use super::*;
use attune_common::{
    models::{Execution, WorkflowExecution, WorkflowTaskMetadata},
    repositories::{
        action::ActionRepository,
        declared_input_materialization::DeclaredInputMaterializationRepository as Inputs,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        native_retry_materialization::NativeRetryMaterializationRepository as Retries,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository as Workflows,
        },
        FindById, FindByRef,
    },
    secret_values::{
        prepare_secret_values, redaction_marker, SecretSource, SecretValueInput,
        ENTITY_EXECUTION_CONFIG,
    },
};

const KEY: &str = "native-retry-test-owned-encryption-key";

async fn rejected_broker_confirmation_rolls_back_retry_mark(confirmation: lapin::Confirmation) {
    use attune_common::{
        mq::publisher::classify_confirmation,
        repositories::maintenance::MaintenanceRepository as Maintenance,
    };
    use lapin::Confirmation;
    let fixture = retry_fixture().await;
    let retry = created(&fixture).await;
    let mut tx = fixture.db.begin().await.unwrap();
    let before = Admissions::require(&mut tx, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    let prepared = Retries::prepare_requested_recovery(&mut tx, &retry, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    Maintenance::mark_execution_reschedule_attempt(
        &mut *tx,
        retry.id,
        "retry-confirmation-test",
        "negative or missing broker confirmation",
        3,
        0,
        false,
    )
    .await
    .unwrap()
    .unwrap();
    let result = prepared
        .publish_bounded(&mut tx, async move {
            classify_confirmation(true, confirmation).map(|_| ())
        })
        .await;
    assert!(matches!(result, Err(attune_common::Error::Internal(_))));
    tx.rollback().await.unwrap();
    let marks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution_reschedule_state WHERE execution_id=$1")
            .bind(retry.id)
            .fetch_one(&*fixture.db)
            .await
            .unwrap();
    assert_eq!(marks, 0);
    let mut recovery = fixture.db.begin().await.unwrap();
    let actual = ExecutionRepository::find_by_id(&mut *recovery, retry.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, ExecutionStatus::Requested);
    let after = Admissions::require(&mut recovery, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    assert_eq!(before.id, after.id);
    assert_eq!(before.authority.attempt, after.authority.attempt);
    let prepared = Retries::prepare_requested_recovery(&mut recovery, &actual, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut recovery).await.unwrap();
    let mark = Maintenance::mark_execution_reschedule_attempt(
        &mut *recovery,
        retry.id,
        "retry-confirmation-test",
        "same-row recovery",
        3,
        0,
        false,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(mark.attempt_count, 1);
    prepared
        .publish_bounded(&mut recovery, async {
            classify_confirmation(true, Confirmation::Ack(None)).map(|_| ())
        })
        .await
        .unwrap();
    recovery.commit().await.unwrap();
    let children: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution WHERE original_execution=$1")
            .bind(fixture.source.id)
            .fetch_one(&*fixture.db)
            .await
            .unwrap();
    assert_eq!(children, 1);
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn actual_broker_nack_rolls_back_retry_mark_and_reuses_same_admission() {
    rejected_broker_confirmation_rolls_back_retry_mark(lapin::Confirmation::Nack(None)).await;
}

#[tokio::test]
async fn missing_requested_broker_confirm_rolls_back_retry_mark_and_reuses_same_admission() {
    rejected_broker_confirmation_rolls_back_retry_mark(lapin::Confirmation::NotRequested).await;
}

#[tokio::test]
async fn requested_scanner_recovers_a_committed_retry_without_completion_redelivery() {
    use attune_common::repositories::maintenance::MaintenanceRepository as Maintenance;
    let fixture = retry_fixture().await;
    let retry = created(&fixture).await; // Simulate process death before publish.
    let candidates =
        Maintenance::find_requested_executions_for_reschedule(fixture.db.pool(), 0, 3, 100)
            .await
            .unwrap();
    assert!(candidates
        .iter()
        .any(|candidate| candidate.execution_id == retry.id));
    let mut tx = fixture.db.begin().await.unwrap();
    let before = Admissions::require(&mut tx, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    let prepared = Retries::prepare_requested_recovery(&mut tx, &retry, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    let mark = Maintenance::mark_execution_reschedule_attempt(
        &mut *tx,
        retry.id,
        "retry-recovery-test",
        "simulate missing broker request",
        3,
        0,
        false,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(mark.execution_id, retry.id);
    prepared
        .publish_bounded(&mut tx, async { Ok(()) })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut verify = fixture.db.begin().await.unwrap();
    let after = Admissions::require(&mut verify, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    assert_eq!(before.id, after.id);
    assert_eq!(before.authority.attempt, after.authority.attempt);
    let children: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution WHERE original_execution=$1")
            .bind(fixture.source.id)
            .fetch_one(&mut *verify)
            .await
            .unwrap();
    assert_eq!(children, 1);
    verify.commit().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn requested_recovery_does_not_reset_a_committed_backoff_deadline() {
    let fixture = retry_fixture().await;
    let mut tx = fixture.db.begin().await.unwrap();
    let prepared = Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
        .await
        .unwrap();
    let mut next = metadata(&fixture.source);
    next.next_retry_at = Some(Utc::now() + Duration::hours(1));
    let retry = prepared.materialize(&mut tx, next).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    let mut recovery = fixture.db.begin().await.unwrap();
    assert!(
        Retries::prepare_requested_recovery(&mut recovery, &retry, KEY)
            .await
            .unwrap()
            .is_none()
    );
    recovery.rollback().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn requested_recovery_requires_the_genuine_retry_edge_and_live_original_approval() {
    for expired in [false, true] {
        let fixture = retry_fixture_with(
            SourceAcceptance::Sealed,
            expired.then(|| Duration::seconds(15)),
        )
        .await;
        let retry = created(&fixture).await;
        if expired {
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= expires_at FROM runtime_external_approval WHERE id=$1")
                        .bind(fixture.base.approval.id).fetch_one(&*fixture.db).await.unwrap();
                    if expired { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await.expect("original issued approval did not expire");
        } else {
            sqlx::query("DELETE FROM runtime_admission_dependency WHERE admission=(SELECT id FROM runtime_authority_admission WHERE owner_kind='execution' AND owner_id=$1) AND dependency_kind='retry'").bind(retry.id).execute(&*fixture.db).await.unwrap();
        }
        let mut tx = fixture.db.begin().await.unwrap();
        assert!(Retries::prepare_requested_recovery(&mut tx, &retry, KEY)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        fixture.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn requested_retry_with_missing_task_metadata_cannot_use_raw_recovery() {
    let fixture = retry_fixture().await;
    let retry = created(&fixture).await;
    sqlx::query("UPDATE execution SET workflow_task=NULL WHERE id=$1")
        .bind(retry.id)
        .execute(&*fixture.db)
        .await
        .unwrap();
    let actual = ExecutionRepository::find_by_id(&fixture.db, retry.id)
        .await
        .unwrap()
        .unwrap();
    assert!(actual.retry_count > 0);
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(Retries::prepare_requested_recovery(&mut tx, &actual, KEY)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn requested_recovery_uses_retained_edges_after_original_execution_retention() {
    let fixture = retry_fixture().await;
    let retry = created(&fixture).await;
    ExecutionRepository::delete(&fixture.db, fixture.source.id)
        .await
        .unwrap();
    let actual = ExecutionRepository::find_by_id(&fixture.db, retry.id)
        .await
        .unwrap()
        .unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    let prepared = Retries::prepare_requested_recovery(&mut tx, &actual, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    prepared
        .publish_bounded(&mut tx, async { Ok(()) })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn broker_callback_row_lock_wait_is_bounded_and_unknown_ack_remains_recoverable() {
    use attune_common::repositories::maintenance::MaintenanceRepository as Maintenance;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let fixture = retry_fixture().await;
    let retry = created(&fixture).await;
    let mut tx = fixture.db.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let prepared = Retries::prepare_requested_recovery(&mut tx, &retry, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    Maintenance::mark_execution_reschedule_attempt(
        &mut *tx,
        retry.id,
        "retry-unknown-ack",
        "simulate callback wait",
        3,
        0,
        false,
    )
    .await
    .unwrap()
    .unwrap();
    let pool = fixture.db.pool().clone();
    let id = retry.id;
    let (send, start) = tokio::sync::oneshot::channel();
    let (acked, ack) = tokio::sync::oneshot::channel();
    let mut callback = tokio::spawn(async move {
        start.await.unwrap();
        let mut callback = pool.begin().await.unwrap();
        ExecutionRepository::find_by_id_for_update(&mut *callback, id)
            .await
            .unwrap()
            .unwrap();
        callback.rollback().await.unwrap();
        let _ = acked.send(());
    });
    let waited = Arc::new(AtomicBool::new(false));
    let saw_wait = waited.clone();
    let pool = fixture.db.pool().clone();
    let result = prepared.publish_bounded(&mut tx, async move {
        let _ = send.send(());
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%execution%')")
                .bind(blocker_pid).fetch_one(&pool).await.unwrap();
            if blocked { saw_wait.store(true, Ordering::SeqCst); break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let _ = ack.await;
        Ok(())
    }).await;
    tx.rollback().await.unwrap();
    let joined = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut callback).await
    {
        Ok(result) => result.is_ok(),
        Err(_) => {
            callback.abort();
            let _ = callback.await;
            false
        }
    };
    let marks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution_reschedule_state WHERE execution_id=$1")
            .bind(retry.id)
            .fetch_one(&*fixture.db)
            .await
            .unwrap();
    let mut recovery = fixture.db.begin().await.unwrap();
    let recovered = Retries::prepare_requested_recovery(&mut recovery, &retry, KEY)
        .await
        .unwrap()
        .unwrap();
    recovered.lock_target(&mut recovery).await.unwrap();
    recovered
        .publish_bounded(&mut recovery, async { Ok(()) })
        .await
        .unwrap();
    recovery.commit().await.unwrap();
    fixture.db.cleanup().await.unwrap();
    assert!(matches!(result, Err(attune_common::Error::Timeout(_))));
    assert!(
        waited.load(Ordering::SeqCst),
        "never observed the actual callback DB lock wait"
    );
    assert!(joined);
    assert_eq!(marks, 0, "unknown acknowledgement consumed a recovery mark");
}

#[tokio::test]
async fn retry_target_advisory_contention_never_waits_behind_its_source_share_fence() {
    let fixture = retry_fixture().await;
    let mut blocker = fixture.db.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(fixture.workflow.id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    let plan = Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
        .await
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        plan.materialize(&mut tx, metadata(&fixture.source)),
    )
    .await;
    tx.rollback().await.unwrap();
    blocker.commit().await.unwrap();
    fixture.db.cleanup().await.unwrap();
    assert!(
        matches!(result, Ok(Err(_))),
        "target advisory lock waited behind retained source owner locks"
    );
}

struct RetryFixture {
    db: TestDatabase,
    base: Fixture,
    source: Execution,
    parent: Execution,
    workflow: WorkflowExecution,
}

async fn retry_fixture() -> RetryFixture {
    retry_fixture_with(SourceAcceptance::Sealed, None).await
}

enum SourceAcceptance {
    Sealed,
    Unsealed,
}

async fn retry_fixture_with(
    acceptance: SourceAcceptance,
    lifetime: Option<Duration>,
) -> RetryFixture {
    fixture_with_source_status(acceptance, lifetime, ExecutionStatus::Failed).await
}

async fn fixture_with_source_status(
    acceptance: SourceAcceptance,
    lifetime: Option<Duration>,
    status: ExecutionStatus,
) -> RetryFixture {
    fixture_with_workflow_source(acceptance, lifetime, status, false).await
}

async fn fixture_with_workflow_source(
    acceptance: SourceAcceptance,
    lifetime: Option<Duration>,
    status: ExecutionStatus,
    stale_leaf_source: bool,
) -> RetryFixture {
    let (db, base) = fixture_with_approval_lifetime(
        Some(json!({
            "token":{"type":"string","secret":true}, "public":{"type":"string"},
        })),
        None,
        lifetime,
    )
    .await;
    let owner = &base.policy.policy.owner;
    let leaf = ActionRepository::find_by_ref(&db, &base.credential.action_ref)
        .await
        .unwrap()
        .unwrap();
    let root = ActionFixture::new(owner.pack_id, &owner.pack_ref, "retry_flow")
        .create(&db)
        .await
        .unwrap();
    let definition = json!({"version":"1.0","tasks":[{"name":"run","action":leaf.r#ref,"permission_set_refs":[base.policy.policy.invocations[0].permission_sets.keys().next().unwrap()],"retry":{"count":2,"delay":0}}]});
    let authored_graph = definition.clone();
    let _: attune_common::workflow::WorkflowDefinition =
        serde_json::from_value(definition.clone()).unwrap();
    let definition = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: root.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Retry flow".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            definition,
            tags: vec![],
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, root.id, definition.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![root.id, leaf.id],
        'b',
    )
    .await;
    let mut policy = base.policy.policy.0.clone();
    policy.release_id = release.id;
    policy.release_digest = release.digest.clone();
    for invocation in &mut policy.invocations {
        invocation.target_release_id = release.id;
        invocation.target_release_digest = release.digest.clone();
    }
    let mut root_invocation = policy.invocations[0].clone();
    root_invocation.binding = InvocationBinding::Action {
        action_ref: root.r#ref.clone(),
    };
    // This is a new workflow policy at original creation, not the leaf's
    // retained source identity. Let capture_policy bind the actual root.
    root_invocation.source_id = stale_leaf_source.then_some(leaf.id);
    root_invocation.dispatch_template_digest = None;
    root_invocation.metadata_revision = None;
    root_invocation.foreign_workflow_policy = None;
    root_invocation.permission_sets.clear();
    let root_binding = root_invocation.binding.clone();
    let mut task = policy.invocations[0].clone();
    task.binding = InvocationBinding::WorkflowTask {
        workflow_ref: root.r#ref.clone(),
        task_path: "/tasks/0".into(),
        action_ref: leaf.r#ref.clone(),
    };
    task.source_id = stale_leaf_source.then_some(leaf.id);
    task.dispatch_template_digest = None;
    task.metadata_revision = None;
    task.foreign_workflow_policy = None;
    let task_binding = task.binding.clone();
    let refs = task.permission_sets.keys().cloned().collect::<Vec<_>>();
    policy.invocations.extend([root_invocation, task]);
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let parent_snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, root.id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(
            &parent_snapshot
                .executable
                .workflow_definition
                .as_ref()
                .unwrap()
                .definition
        )
        .unwrap(),
        authored_graph
    );
    if !stale_leaf_source {
        for binding in [&root_binding, &task_binding] {
            assert_eq!(
                policy
                    .policy
                    .invocations
                    .iter()
                    .find(|invocation| &invocation.binding == binding)
                    .unwrap()
                    .source_id,
                Some(root.id)
            );
        }
    }
    let parent = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(root.id),
            action_ref: root.r#ref,
            executor: Some(base.human_id),
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            ..Default::default()
        },
        &parent_snapshot,
    )
    .await
    .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            base.admission.authority.attribution.clone(),
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(parent.id),
        &authority,
    )
    .await
    .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, &parent_snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, parent.id, &schema)
        .await
        .unwrap();
    Contributors::capture_human(&mut tx, "execution", parent.id, base.human_id)
        .await
        .unwrap();
    Inputs::seal_execution_inputs(&mut tx, parent.id, KEY)
        .await
        .unwrap();
    let workflow = Workflows::create(&mut *tx, CreateWorkflowExecutionInput {
        execution: parent.id, workflow_def: definition.id,
        // First-attempt recovery exercises WorkflowContext's supported raw
        // retained-definition form, identical to the original pinned source.
        task_graph: if status == ExecutionStatus::Requested { authored_graph } else { json!({"nodes":{"run":{"name":"run","task_type":"action","action":leaf.r#ref,"input":{},"retry":{"count":2,"delay":0,"backoff":"constant"},"permission_set_refs":refs,"transitions":[],"inbound_tasks":[]}},"entry_points":["run"],"inbound_edges":{"run":[]},"outbound_edges":{"run":[]}}) },
        variables: json!({}), status: ExecutionStatus::Running,
    }).await.unwrap();
    let workflow = Workflows::update(
        &mut *tx,
        workflow.id,
        attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
            current_tasks: Some(vec!["run".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, leaf.id)
        .await
        .unwrap();
    let source = ExecutionRepository::create_pinned(&mut *tx, CreateExecutionInput {
        action: Some(leaf.id), action_ref: leaf.r#ref, parent: Some(parent.id), executor: Some(base.human_id),
        config: Some(json!({"token":redaction_marker(),"public":"original"})),
        permission_set_refs: refs, status: ExecutionStatus::Running,
        workflow_task: Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"run","retry_count":0,"max_retries":2,"timed_out":false})).unwrap()),
        ..Default::default()
    }, &snapshot).await.unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &task_binding,
            &source.permission_set_refs,
            RequestAttribution::ParentExecution {
                execution_id: parent.id,
            },
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(source.id),
        &authority,
    )
    .await
    .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, source.id, &schema)
        .await
        .unwrap();
    Contributors::capture_human(&mut tx, "execution", source.id, base.human_id)
        .await
        .unwrap();
    let fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: json!("{{ this_is_a_literal_not_a_new_render }}"),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(
                    &SecretSource::ParameterSchema {
                        path: "/token".into(),
                    }
                    .provenance(),
                )
                .unwrap(),
            ),
        }],
        KEY,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, ENTITY_EXECUTION_CONFIG, source.id, &fields)
        .await
        .unwrap();
    if matches!(acceptance, SourceAcceptance::Sealed) {
        Inputs::seal_execution_inputs(&mut tx, source.id, KEY)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let source = ExecutionRepository::update(
        &db,
        source.id,
        UpdateExecutionInput {
            status: Some(status),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    RetryFixture {
        db,
        base,
        source,
        parent,
        workflow,
    }
}

fn metadata(source: &Execution) -> WorkflowTaskMetadata {
    let mut metadata = source.workflow_task.clone().unwrap();
    metadata.retry_count += 1;
    metadata.next_retry_at = Some(Utc::now());
    metadata.started_at = None;
    metadata.completed_at = None;
    metadata.duration_ms = None;
    metadata.timed_out = false;
    metadata
}

async fn created(fixture: &RetryFixture) -> Execution {
    let mut tx = fixture.db.begin().await.unwrap();
    let plan = Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
        .await
        .unwrap();
    let retry = plan
        .materialize(&mut tx, metadata(&fixture.source))
        .await
        .unwrap()
        .unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    retry
}

pub(super) async fn requested_retry_fixture() -> (TestDatabase, Execution) {
    let fixture = retry_fixture().await;
    let retry = created(&fixture).await;
    (fixture.db, retry)
}

pub(super) async fn requested_first_attempt_fixture() -> (TestDatabase, Execution, Id) {
    let fixture =
        fixture_with_source_status(SourceAcceptance::Sealed, None, ExecutionStatus::Requested)
            .await;
    (fixture.db, fixture.source, fixture.workflow.id)
}

pub(super) async fn requested_first_attempt_with_stale_source_fixture() -> (TestDatabase, Execution)
{
    // Preserve the formerly malformed source as a denial counterfactual. Its
    // metadata is supplied before capture/admission, never edited afterward.
    let fixture = fixture_with_workflow_source(
        SourceAcceptance::Sealed,
        None,
        ExecutionStatus::Requested,
        true,
    )
    .await;
    (fixture.db, fixture.source)
}

#[tokio::test]
async fn identical_retry_survives_human_freeze_and_ordinary_disable_without_recapture() {
    let fixture = retry_fixture().await;
    IdentityRepository::update(
        &fixture.db,
        fixture.base.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ActionRepository::update(
        &fixture.db,
        fixture.source.action.unwrap(),
        attune_common::repositories::action::UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let original =
        Secrets::find_stored_by_entity(&fixture.db, ENTITY_EXECUTION_CONFIG, fixture.source.id)
            .await
            .unwrap();
    let retry = created(&fixture).await;
    assert_eq!(retry.config, fixture.source.config);
    assert!(retry.worker.is_none());
    assert!(retry.result.is_none());
    assert_eq!(retry.original_execution, Some(fixture.source.id));
    let fields = Secrets::find_stored_by_entity(&fixture.db, ENTITY_EXECUTION_CONFIG, retry.id)
        .await
        .unwrap();
    assert_eq!(fields.len(), original.len());
    for (actual, original) in fields.iter().zip(&original) {
        assert_eq!(actual.encrypted_value, original.encrypted_value);
        assert_eq!(actual.encryption_key_hash, original.encryption_key_hash);
        assert_ne!(
            actual.source_ref, original.source_ref,
            "first destination frames are unique, not copied aliases"
        );
    }
    let mut tx = fixture.db.begin().await.unwrap();
    let source = Admissions::require(&mut tx, RuntimeOwner::Execution(fixture.source.id))
        .await
        .unwrap();
    let target = Admissions::require(&mut tx, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    assert_ne!(source.authority.attempt, target.authority.attempt);
    assert_eq!(source.authority.owner, target.authority.owner);
    assert_eq!(
        Schemas::find(&mut tx, BindingOwner::Execution, fixture.source.id)
            .await
            .unwrap(),
        Schemas::find(&mut tx, BindingOwner::Execution, retry.id)
            .await
            .unwrap()
    );
    let publication = Retries::prepare_publication(&mut tx, &retry, fixture.source.id, KEY)
        .await
        .unwrap();
    publication.lock_target(&mut tx).await.unwrap();
    publication.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn changed_public_config_is_not_accepted_as_an_identical_retry() {
    let mut fixture = retry_fixture().await;
    sqlx::query(
        "UPDATE execution SET config = jsonb_set(config,'{public}','\"changed\"') WHERE id=$1",
    )
    .bind(fixture.source.id)
    .execute(&*fixture.db)
    .await
    .unwrap();
    fixture.source = ExecutionRepository::find_by_id(&fixture.db, fixture.source.id)
        .await
        .unwrap()
        .unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(
        Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn changed_ciphertext_and_a_missing_original_seal_cannot_be_repaired_by_retry() {
    for missing in [false, true] {
        let fixture = retry_fixture_with(
            if missing {
                SourceAcceptance::Unsealed
            } else {
                SourceAcceptance::Sealed
            },
            None,
        )
        .await;
        if !missing {
            sqlx::query("UPDATE execution_secret_value SET encrypted_value='{}' WHERE entity_type='execution_config' AND entity_id=$1").bind(fixture.source.id).execute(&*fixture.db).await.unwrap();
        }
        if missing {
            let seals: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM runtime_accepted_input WHERE execution=$1",
            )
            .bind(fixture.source.id)
            .fetch_one(&*fixture.db)
            .await
            .unwrap();
            assert_eq!(seals, 0, "negative fixture must be genuinely unsealed");
        }
        let mut tx = fixture.db.begin().await.unwrap();
        assert!(
            Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
        let children: i64 =
            sqlx::query_scalar("SELECT count(*) FROM execution WHERE original_execution=$1")
                .bind(fixture.source.id)
                .fetch_one(&*fixture.db)
                .await
                .unwrap();
        assert_eq!(children, 0);
        fixture.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_accepted_observation_does_not_relax_the_input_delivery_gate() {
    let fixture = retry_fixture().await;
    assert_eq!(fixture.source.status, ExecutionStatus::Failed);
    let mut tx = fixture.db.begin().await.unwrap();
    assert!(
        Inputs::prepare_execution_input_receipts(&mut tx, fixture.source.id, KEY)
            .await
            .is_err(),
        "terminal sources must still reject new input delivery"
    );
    let plan = Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
        .await
        .unwrap();
    let retry = plan
        .materialize(&mut tx, metadata(&fixture.source))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.config, fixture.source.config);
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn accepted_archive_restrict_guard_survives_and_retry_needs_no_deletion() {
    let fixture = retry_fixture().await;
    let mut tx = fixture.db.begin().await.unwrap();
    let error = sqlx::query("DELETE FROM runtime_accepted_input WHERE execution=$1")
        .bind(fixture.source.id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("runtime_input_receipt_accepted_accepted_fkey")
    );
    tx.rollback().await.unwrap();
    let retry = created(&fixture).await;
    assert_eq!(retry.config, fixture.source.config);
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn changed_failed_outcome_after_source_preparation_rolls_back_without_a_child() {
    let fixture = retry_fixture().await;
    let mut tx = fixture.db.begin().await.unwrap();
    let plan = Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
        .await
        .unwrap();
    // Preparation retains a canonical source SHARE fence. Inject the changed
    // outcome in this transaction rather than deadlocking a competing writer.
    ExecutionRepository::update(
        &mut *tx,
        fixture.source.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(plan
        .materialize(&mut tx, metadata(&fixture.source))
        .await
        .is_err());
    tx.rollback().await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution WHERE original_execution=$1")
            .bind(fixture.source.id)
            .fetch_one(&*fixture.db)
            .await
            .unwrap();
    assert_eq!(count, 0);
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancelled_parent_and_revoked_original_authority_block_continuation() {
    for revoke in [false, true] {
        let fixture = retry_fixture().await;
        if revoke {
            let mut tx = fixture.db.begin().await.unwrap();
            RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.base.approval.id)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        } else {
            ExecutionRepository::update(
                &fixture.db,
                fixture.parent.id,
                UpdateExecutionInput {
                    status: Some(ExecutionStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        let mut tx = fixture.db.begin().await.unwrap();
        let outcome = Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY).await;
        if let Ok(plan) = outcome {
            assert!(plan
                .materialize(&mut tx, metadata(&fixture.source))
                .await
                .is_err());
        }
        tx.rollback().await.unwrap();
        let children: i64 =
            sqlx::query_scalar("SELECT count(*) FROM execution WHERE original_execution=$1")
                .bind(fixture.source.id)
                .fetch_one(&*fixture.db)
                .await
                .unwrap();
        assert_eq!(children, 0);
        fixture.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn duplicate_completion_does_not_create_or_rotate_another_successor() {
    let fixture = retry_fixture().await;
    let first = created(&fixture).await;
    let mut tx = fixture.db.begin().await.unwrap();
    let plan = Retries::prepare(&mut tx, &fixture.source, &fixture.workflow, KEY)
        .await
        .unwrap();
    let recovered = plan
        .materialize(&mut tx, metadata(&fixture.source))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered.id, first.id,
        "recover publication of the same committed successor"
    );
    tx.commit().await.unwrap();
    let rows: Vec<Id> = sqlx::query_scalar("SELECT id FROM execution WHERE original_execution=$1")
        .bind(fixture.source.id)
        .fetch_all(&*fixture.db)
        .await
        .unwrap();
    assert_eq!(rows, vec![first.id]);
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancellation_or_revocation_during_backoff_prevents_publication() {
    for revoke in [false, true] {
        let fixture = retry_fixture().await;
        let retry = created(&fixture).await;
        if revoke {
            let mut tx = fixture.db.begin().await.unwrap();
            RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.base.approval.id)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        } else {
            ExecutionRepository::update(
                &fixture.db,
                retry.id,
                UpdateExecutionInput {
                    status: Some(ExecutionStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        let mut tx = fixture.db.begin().await.unwrap();
        let prepared = Retries::prepare_publication(&mut tx, &retry, fixture.source.id, KEY).await;
        if let Ok(prepared) = prepared {
            assert!(prepared.lock_target(&mut tx).await.is_err());
        }
        tx.rollback().await.unwrap();
        fixture.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn retry_worker_attempt_is_new_and_rejects_source_or_wrong_worker_credentials() {
    let fixture = retry_fixture().await;
    let retry = created(&fixture).await;
    let worker = WorkerRepository::create(
        &fixture.db,
        CreateWorkerInput {
            name: format!("retry-worker-{}", uuid::Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &fixture.db,
        retry.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            worker: Some(worker.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    let source = Admissions::require(&mut tx, RuntimeOwner::Execution(fixture.source.id))
        .await
        .unwrap();
    let before = Admissions::require(&mut tx, RuntimeOwner::Execution(retry.id))
        .await
        .unwrap();
    assert_ne!(source.authority.attempt, before.authority.attempt);
    let attempt = uuid::Uuid::new_v4();
    let credential =
        RuntimeAuthorityRepository::begin_execution_attempt(&mut tx, retry.id, worker.id, attempt)
            .await
            .unwrap();
    assert_eq!(credential.execution_id, retry.id);
    assert_eq!(credential.admission_id, before.id);
    assert_eq!(credential.attempt, attempt);
    let mut wrong_attempt = credential.clone();
    wrong_attempt.attempt = source.authority.attempt;
    assert!(
        RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &wrong_attempt)
            .await
            .is_err()
    );
    assert!(RuntimeAuthorityRepository::begin_execution_attempt(
        &mut tx,
        retry.id,
        i64::MAX,
        uuid::Uuid::new_v4()
    )
    .await
    .is_err());
    RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &credential)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn approval_expiry_after_actual_workflow_target_wait_rolls_back_retry_creation() {
    let fixture = retry_fixture_with(SourceAcceptance::Sealed, Some(Duration::seconds(15))).await;
    let mut blocker = fixture.db.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    Workflows::find_by_id_for_update(&mut *blocker, fixture.workflow.id)
        .await
        .unwrap()
        .unwrap();
    let pool = fixture.db.pool().clone();
    let source = fixture.source.clone();
    let workflow = fixture.workflow.clone();
    let (ready, observed) = tokio::sync::oneshot::channel();
    let mut writer = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        let prepared = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Retries::prepare(&mut tx, &source, &workflow, KEY),
        )
        .await;
        let plan = match prepared {
            Ok(Ok(plan)) => plan,
            _ => {
                let _ = ready.send(false);
                tx.rollback().await.unwrap();
                return false;
            }
        };
        let _ = ready.send(true);
        let denied = plan.materialize(&mut tx, metadata(&source)).await.is_err();
        tx.rollback().await.unwrap();
        denied
    });
    let prepared = tokio::time::timeout(std::time::Duration::from_secs(6), observed).await;
    let source_ready = matches!(prepared, Ok(Ok(true)));
    let waited = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%workflow_execution%')")
                .bind(blocker_pid).fetch_one(&*fixture.db).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.is_ok();
    let expired = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp() >= expires_at FROM runtime_external_approval WHERE id=$1",
            )
            .bind(fixture.base.approval.id)
            .fetch_one(&*fixture.db)
            .await
            .unwrap();
            if expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    blocker.commit().await.unwrap();
    let (denied, writer_error) =
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut writer).await {
            Ok(Ok(denied)) => (denied, None),
            Ok(Err(error)) => (false, Some(error.to_string())),
            Err(_) => {
                writer.abort();
                let stopped = writer.await;
                (
                    false,
                    Some(format!(
                        "retry writer timed out and was joined: {stopped:?}"
                    )),
                )
            }
        };
    let children: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution WHERE original_execution=$1")
            .bind(fixture.source.id)
            .fetch_one(&*fixture.db)
            .await
            .unwrap();
    fixture.db.cleanup().await.unwrap();
    assert!(writer_error.is_none(), "{writer_error:?}");
    assert!(
        source_ready,
        "source preparation waited on the workflow target"
    );
    assert!(waited, "test never observed the actual workflow-row wait");
    assert!(expired);
    assert!(denied);
    assert_eq!(children, 0);
}
