use super::*;
use attune_common::{
    artifact_transport::{ArtifactFileTransport, VolumeTransport},
    auth::jwt::JwtConfig,
    models::{LogStreamBackend, RetentionPolicyType, WorkerType},
    repositories::{
        runtime::{CreateWorkerInput, WorkerRepository},
        worker_delivery::WorkerDeliveryRepository,
        FindById,
    },
};
use attune_worker::completion::{allocate_log, register, CompletionTransport};
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn recorded_shared_volume_completion_is_bounded_and_revalidates_terminal_source() {
    attune_common::auth::crypto_provider::install();
    let fixture = fixture(true).await;
    let worker = WorkerRepository::create(
        &fixture.db,
        CreateWorkerInput {
            name: "completion-worker".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await
    .unwrap();
    let execution = ExecutionRepository::update(
        &fixture.db,
        fixture.execution,
        UpdateExecutionInput {
            worker: Some(worker.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let instance = Uuid::new_v4();
    let mutation = sqlx::query("UPDATE execution SET executable_snapshot=jsonb_set(executable_snapshot,'{executable,action,id}',to_jsonb($2::bigint)) WHERE id=$1")
        .bind(execution.id).bind(i64::MAX).execute(&fixture.db).await.unwrap_err();
    assert!(matches!(mutation, sqlx::Error::Database(ref error)
        if error.message().contains("Admitted runtime executable metadata is immutable")));
    // A stale caller copy must not choose the completion owner or deadline.
    let mut stale = execution.clone();
    stale.action_ref = "untrusted.reused_name".into();
    stale.timeout_seconds = Some(1);
    stale
        .executable_snapshot
        .as_mut()
        .unwrap()
        .executable
        .action
        .id = i64::MAX;
    let pinned: attune_common::models::ExecutionExecutableSnapshot =
        execution.executable_snapshot.clone().unwrap();
    let expected_timeout = [
        execution.timeout_seconds,
        execution
            .workflow_task
            .as_ref()
            .and_then(|task| task.timeout_seconds),
        pinned.executable.action.timeout_seconds,
    ]
    .into_iter()
    .flatten()
    .filter_map(|seconds| u64::try_from(seconds).ok())
    .find(|seconds| *seconds > 0)
    .unwrap_or_else(attune_common::config::app_default_execution_timeout_seconds);
    let registered_after = chrono::Utc::now();
    let context = register(fixture.db.pool(), &stale, worker.id, instance)
        .await
        .unwrap()
        .unwrap();
    assert!(
        context.deadline
            >= registered_after + chrono::Duration::seconds(expected_timeout as i64 + 60)
    );
    let (path, version) = allocate_log(
        fixture.db.pool(),
        &stale,
        &context,
        "stdout",
        LogStreamBackend::SharedFile,
        RetentionPolicyType::Versions,
        10,
        1024,
        500,
        4,
    )
    .await
    .unwrap();
    let pinned_root = attune_common::worker_delivery::log_reference(&execution, "stdout").unwrap();
    assert!(path.starts_with(&pinned_root.replace('.', "/")));
    let root = tempfile::tempdir().unwrap();
    let base: Arc<dyn ArtifactFileTransport> =
        Arc::new(VolumeTransport::new(root.path().to_str().unwrap()));
    assert!(CompletionTransport::for_version(
        fixture.db.pool(),
        version,
        worker.id,
        Uuid::new_v4(),
        base.clone(),
        "http://127.0.0.1:1",
        &JwtConfig::default()
    )
    .await
    .is_err());
    let transport = CompletionTransport::for_version(
        fixture.db.pool(),
        version,
        worker.id,
        instance,
        base,
        "http://127.0.0.1:1",
        &JwtConfig::default(),
    )
    .await
    .unwrap();
    transport
        .write_file(&path, b"", Some("text/plain"))
        .await
        .unwrap();
    ExecutionRepository::update(
        &fixture.db,
        execution.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(allocate_log(
        fixture.db.pool(),
        &execution,
        &context,
        "stderr",
        LogStreamBackend::SharedFile,
        RetentionPolicyType::Versions,
        10,
        1024,
        500,
        4
    )
    .await
    .is_err());
    let encoded = attune_common::runtime_read_log::encode(
        "worker-completion-frame-test-key",
        execution.id,
        version,
        0,
        b"abcd",
    )
    .unwrap();
    assert!(encoded.len() > 4);
    transport.append_log_file(&path, &encoded).await.unwrap();
    let oversized =
        vec![b'x'; attune_common::runtime_read_log::maximum_encoded_bytes(4).unwrap() as usize];
    assert!(transport.append_log_file(&path, &oversized).await.is_err());
    assert_eq!(
        tokio::fs::read(root.path().join(&path)).await.unwrap(),
        encoded
    );
    let mut conn = fixture.db.acquire().await.unwrap();
    assert!(
        WorkerDeliveryRepository::start(&mut conn, execution.id, worker.id)
            .await
            .is_err()
    );
    drop(conn);
    assert!(transport.open_reader(&path, 0).await.is_err());
    assert!(transport.delete_file(&path).await.is_err());
    IdentityRepository::update(
        &fixture.db,
        fixture.sponsor,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Revocation is checked before any shared file append, even when terminal.
    assert!(transport.append_log_file(&path, b"").await.is_err());
    drop(transport);
    root.close().unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn empty_selection_still_registers_a_purpose_specific_worker_delivery_permit() {
    attune_common::auth::crypto_provider::install();
    use attune_common::models::ExecutionExecutableSnapshot;
    use attune_worker::recorded_credentials::{
        mint_recorded_execution_token, RecordedConsumerReadiness,
    };
    let fixture = fixture(true).await;
    let worker = WorkerRepository::create(
        &fixture.db,
        CreateWorkerInput {
            name: "empty-completion-worker".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await
    .unwrap();
    let original = ExecutionRepository::find_by_id(&fixture.db, fixture.execution)
        .await
        .unwrap()
        .unwrap();
    let snapshot: ExecutionExecutableSnapshot = original.executable_snapshot.clone().unwrap();
    let mut tx = fixture.db.begin().await.unwrap();
    let source = Admission::require(&mut tx, RuntimeOwner::Execution(fixture.execution))
        .await
        .unwrap();
    let policy = Authority::find_policy(&mut tx, source.policy_revision)
        .await
        .unwrap()
        .unwrap();
    let selected = policy
        .policy
        .select(
            policy.id,
            &source.authority.binding,
            &[],
            source.authority.attribution.clone(),
        )
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: original.action,
            action_ref: original.action_ref.clone(),
            executor: original.executor,
            worker: Some(worker.id),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    Authority::record_admission(&mut tx, RuntimeOwner::Execution(execution.id), &selected)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let config = JwtConfig::default();
    assert!(mint_recorded_execution_token(
        fixture.db.pool(),
        &execution,
        worker.id,
        Uuid::new_v4(),
        60,
        &config,
        RecordedConsumerReadiness::default()
    )
    .await
    .unwrap()
    .is_none());
    let instance = Uuid::new_v4();
    let context = register(fixture.db.pool(), &execution, worker.id, instance)
        .await
        .unwrap()
        .unwrap();
    let (_, version) = allocate_log(
        fixture.db.pool(),
        &execution,
        &context,
        "stderr",
        LogStreamBackend::SharedFile,
        RetentionPolicyType::Versions,
        10,
        1024,
        500,
        0,
    )
    .await
    .unwrap();
    let mut conn = fixture.db.acquire().await.unwrap();
    let permit = WorkerDeliveryRepository::for_version(&mut conn, version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(permit.context, context.id);
    let token = attune_common::worker_delivery::mint(&context, &permit, &config).unwrap();
    let claims = attune_common::auth::jwt::validate_token(&token, &config).unwrap();
    assert!(attune_common::worker_delivery::credential(&claims)
        .unwrap()
        .is_some());
    assert!(claims.admitted_execution_credential().unwrap().is_none());
    drop(conn);
    fixture.db.cleanup().await.unwrap();
}
