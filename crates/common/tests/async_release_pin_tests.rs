//! Release activation race tests for durable asynchronous work.

mod helpers;

use attune_common::{
    models::{
        enums::{
            EnforcementCondition, EnforcementStatus, ExecutionStatus, WorkerStatus, WorkerType,
        },
        ActionReferenceVisibility, ExecutionExecutableSnapshot, WorkQueueBatchMode,
        WorkQueueItemStatus, WorkQueueUpdateStrategy,
    },
    repositories::{
        action::{ActionRepository, CreateActionInput, UpdateActionInput},
        component_lifecycle::{ComponentLifecycleRepository, PackProjectionIds},
        event::{CreateEnforcementInput, EnforcementRepository},
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository},
        pack::PackRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        pack_retention::PackRetentionRepository,
        runtime::{CreateWorkerInput, WorkerRepository},
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadRepository,
        },
        work_queue::{
            CreateWorkQueueInput, CreateWorkQueueItemInput, LeaseWorkQueueItemsInput,
            WorkQueueItemRepository, WorkQueueRepository,
        },
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
        Create, Delete, FindById, Update,
    },
};
use chrono::{Duration, Utc};
use helpers::{create_test_pool, ActionFixture, PackFixture, RuntimeFixture, SensorFixture};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn create_release(
    pool: &PgPool,
    pack: &attune_common::models::Pack,
    version: &str,
    marker: char,
) -> attune_common::models::PackRelease {
    let digest = marker.to_string().repeat(64);
    let mut tx = pool.begin().await.expect("begin release transaction");
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: version.to_string(),
            digest,
            object_key: format!("packs/blobs/{version}.tar.gz"),
            provider_version: "e:test-version".to_string(),
            content_path: format!("/packs/.releases/{version}"),
            archive_size: 1,
            manifest: json!({"version": version}),
        },
    )
    .await
    .expect("create release");
    tx.commit().await.expect("commit release transaction");
    release
}

async fn activate(pool: &PgPool, pack_id: i64, release_id: i64) {
    let mut tx = pool.begin().await.expect("begin activation transaction");
    PackReleaseRepository::activate(&mut tx, pack_id, release_id)
        .await
        .expect("activate release");
    tx.commit().await.expect("commit activation transaction");
}

async fn activate_projected(
    pool: &PgPool,
    pack_id: i64,
    release_id: i64,
    projections: &PackProjectionIds,
) {
    let mut tx = pool.begin().await.expect("begin activation transaction");
    PackReleaseRepository::activate_projected(&mut tx, pack_id, release_id, projections)
        .await
        .expect("activate projected release");
    tx.commit().await.expect("commit activation transaction");
}

fn execution_input(
    action: &attune_common::models::Action,
    parent: Option<i64>,
) -> CreateExecutionInput {
    CreateExecutionInput {
        action: Some(action.id),
        action_ref: action.r#ref.clone(),
        parent,
        status: ExecutionStatus::Requested,
        ..Default::default()
    }
}

async fn action_fixture() -> (
    attune_common::test_database::TestDatabase,
    attune_common::models::Pack,
    attune_common::models::Action,
    attune_common::models::PackRelease,
    ExecutionExecutableSnapshot,
) {
    let pool = create_test_pool().await.expect("test database");
    let pack = PackFixture::new_unique("async_pin")
        .create(&pool)
        .await
        .expect("pack");
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "run")
        .with_entrypoint("release-a.py")
        .create(&pool)
        .await
        .expect("action");
    let release_a = create_release(&pool, &pack, "1.0.0", 'a').await;
    activate_projected(
        &pool,
        pack.id,
        release_a.id,
        &PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await;
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&pool, action.id)
        .await
        .expect("release A snapshot");
    (pool, pack, action, release_a, snapshot)
}

#[tokio::test]
async fn ad_hoc_action_snapshots_current_definition_against_active_release() {
    let pool = create_test_pool().await.expect("test database");
    let pack = PackFixture::new_unique("adhoc_pin")
        .create(&pool)
        .await
        .expect("pack");
    let release = create_release(&pool, &pack, "1.0.0", 'a').await;
    activate(&pool, pack.id, release.id).await;
    let action_ref = format!("{}.adhoc", pack.r#ref);
    let action = ActionRepository::create(
        &pool,
        CreateActionInput {
            r#ref: action_ref,
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Ad hoc".to_string(),
            description: None,
            entrypoint: "printf first".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            param_schema: None,
            out_schema: None,
            is_adhoc: true,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        },
    )
    .await
    .expect("ad hoc action");

    let first = ExecutableSnapshotRepository::resolve_for_action(&pool, action.id)
        .await
        .expect("first ad hoc snapshot");
    ActionRepository::update(
        &pool,
        action.id,
        UpdateActionInput {
            entrypoint: Some("printf second".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("update ad hoc action");
    let second = ExecutableSnapshotRepository::resolve_for_action(&pool, action.id)
        .await
        .expect("second ad hoc snapshot");

    assert_eq!(first.release.id, release.id);
    assert_eq!(first.executable.action.entrypoint, "printf first");
    assert_eq!(second.release.id, release.id);
    assert_eq!(second.executable.action.entrypoint, "printf second");
}

#[tokio::test]
async fn queued_dispatch_keeps_release_a_after_b_activates() {
    let (pool, pack, action, release_a, snapshot_a) = action_fixture().await;
    let queue = WorkQueueRepository::create(
        &pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.inbox", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Inbox".to_string(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Immutable,
            batch_mode: WorkQueueBatchMode::Single,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .expect("queue");
    let item = WorkQueueItemRepository::create_pinned(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref,
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Queued,
            payload: json!({"value": "A"}),
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "test".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
        &snapshot_a,
    )
    .await
    .expect("queued item");

    let release_b = create_release(&pool, &pack, "2.0.0", 'b').await;
    activate(&pool, pack.id, release_b.id).await;

    let leased = WorkQueueItemRepository::lease_next_batch(
        &pool,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 1,
            batch_coalescing: None,
            leased_execution: None,
            lease_token: Uuid::new_v4(),
            lease_expires_at: Utc::now() + Duration::minutes(1),
        },
    )
    .await
    .expect("lease queued item after release B activation");

    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].id, item.id);
    assert_eq!(leased[0].pack_release, Some(release_a.id));
    assert_eq!(
        leased[0].executable_snapshot.as_ref().unwrap()["release"]["id"],
        release_a.id
    );
}

#[tokio::test]
async fn retired_action_keeps_pinned_execution_and_external_queue_reference() {
    let (pool, pack, action, release, snapshot) = action_fixture().await;
    let execution =
        ExecutionRepository::create_pinned(&pool, execution_input(&action, None), &snapshot)
            .await
            .expect("pinned execution");
    let queue_pack = PackFixture::new_unique("external_queue")
        .create(&pool)
        .await
        .expect("queue pack");
    let queue = WorkQueueRepository::create(
        &pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.inbox", queue_pack.r#ref),
            pack: Some(queue_pack.id),
            pack_ref: Some(queue_pack.r#ref),
            is_adhoc: false,
            label: "External queue".to_string(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Immutable,
            batch_mode: WorkQueueBatchMode::Single,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .expect("external queue");

    let mut connection = pool.acquire().await.expect("connection");
    ComponentLifecycleRepository::reconcile_omissions(
        &mut connection,
        pack.id,
        &PackProjectionIds::default(),
    )
    .await
    .expect("retire action");

    assert!(ActionRepository::find_by_id(&pool, action.id)
        .await
        .expect("active action lookup")
        .is_none());
    assert_eq!(
        ActionRepository::find_by_id_including_retired(&pool, action.id)
            .await
            .expect("historical action lookup")
            .expect("retired action")
            .id,
        action.id
    );
    let stored_execution = ExecutionRepository::find_by_id(&pool, execution.id)
        .await
        .expect("execution lookup")
        .expect("execution");
    assert_eq!(
        stored_execution
            .executable_snapshot
            .expect("retained execution snapshot")
            .release
            .id,
        release.id
    );
    assert_eq!(
        WorkQueueRepository::find_by_id(&pool, queue.id)
            .await
            .expect("queue lookup")
            .expect("external queue")
            .dispatch_action,
        Some(action.id)
    );
}

#[tokio::test]
async fn workflow_transition_keeps_root_release_a_after_b_activates() {
    let (pool, pack, action, release_a, snapshot_a) = action_fixture().await;
    let root =
        ExecutionRepository::create_pinned(&pool, execution_input(&action, None), &snapshot_a)
            .await
            .expect("workflow root");
    let release_b = create_release(&pool, &pack, "2.0.0", 'b').await;
    activate(&pool, pack.id, release_b.id).await;

    let child = ExecutionRepository::create_pinned(
        &pool,
        execution_input(&action, Some(root.id)),
        root.executable_snapshot.as_ref().expect("root snapshot"),
    )
    .await
    .expect("workflow child");

    assert_eq!(child.pack_release, Some(release_a.id));
    assert_eq!(child.executable_snapshot.unwrap().release.id, release_a.id);
}

#[tokio::test]
async fn workflow_snapshot_pins_each_action_to_its_defining_release() {
    let pool = create_test_pool().await.expect("test database");
    let pack = PackFixture::new_unique("mixed_workflow")
        .create(&pool)
        .await
        .expect("pack");
    let child = ActionFixture::new_unique(pack.id, &pack.r#ref, "child")
        .with_entrypoint("child-a.py")
        .create(&pool)
        .await
        .expect("child action");
    let root = ActionFixture::new_unique(pack.id, &pack.r#ref, "workflow")
        .with_entrypoint("workflow-a.yaml")
        .create(&pool)
        .await
        .expect("workflow action");
    let workflow = WorkflowDefinitionRepository::create(
        &pool,
        CreateWorkflowDefinitionInput {
            r#ref: root.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Mixed release workflow".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({}),
            tags: Vec::new(),
        },
    )
    .await
    .expect("workflow definition");
    let root = ActionRepository::link_workflow_def(&pool, root.id, workflow.id)
        .await
        .expect("link workflow action");

    let release_a = create_release(&pool, &pack, "1.0.0", 'a').await;
    activate_projected(
        &pool,
        pack.id,
        release_a.id,
        &PackProjectionIds {
            actions: vec![child.id, root.id],
            workflows: vec![workflow.id],
            ..Default::default()
        },
    )
    .await;

    let release_b = create_release(&pool, &pack, "2.0.0", 'b').await;
    activate_projected(
        &pool,
        pack.id,
        release_b.id,
        &PackProjectionIds {
            actions: vec![root.id],
            workflows: vec![workflow.id],
            ..Default::default()
        },
    )
    .await;

    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&pool, root.id)
        .await
        .expect("mixed release workflow snapshot");
    let child_snapshot = snapshot
        .pack_executables
        .get(&child.r#ref)
        .expect("child snapshot");

    assert_eq!(snapshot.release.id, release_b.id);
    assert_eq!(child_snapshot.release.id, release_a.id);
    assert_eq!(child_snapshot.executable.action.entrypoint, "child-a.py");
}

#[tokio::test]
async fn retry_keeps_original_release_a_after_b_activates() {
    let (pool, pack, action, release_a, snapshot_a) = action_fixture().await;
    let original =
        ExecutionRepository::create_pinned(&pool, execution_input(&action, None), &snapshot_a)
            .await
            .expect("original execution");
    let release_b = create_release(&pool, &pack, "2.0.0", 'b').await;
    activate(&pool, pack.id, release_b.id).await;

    let retry = ExecutionRepository::create_retry(
        &pool,
        execution_input(&action, None),
        original
            .executable_snapshot
            .as_ref()
            .expect("original snapshot"),
        1,
        Some(1),
        Some("failed".to_string()),
        original.id,
    )
    .await
    .expect("retry execution");

    assert_eq!(retry.pack_release, Some(release_a.id));
    assert_eq!(retry.executable_snapshot.unwrap().release.id, release_a.id);

    let history: serde_json::Value = sqlx::query_scalar(
        "SELECT new_values FROM execution_history WHERE entity_id = $1 AND operation = 'INSERT' ORDER BY time DESC LIMIT 1",
    )
    .bind(original.id)
    .fetch_one(&*pool)
    .await
    .expect("execution history");
    assert_eq!(history["pack_release"], release_a.id);
    assert!(history["executable_snapshot"]["digest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("md5:")));
}

#[tokio::test]
async fn managed_sensor_replacement_keeps_desired_release_a_after_b_activates() {
    let pool = create_test_pool().await.expect("test database");
    let pack = PackFixture::new_unique("sensor_pin")
        .create(&pool)
        .await
        .expect("pack");
    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "runtime")
        .create(&pool)
        .await
        .expect("runtime");
    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref,
        "sensor",
    )
    .create(&pool)
    .await
    .expect("sensor");
    let release_a = create_release(&pool, &pack, "1.0.0", 'a').await;
    activate_projected(
        &pool,
        pack.id,
        release_a.id,
        &PackProjectionIds {
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            ..Default::default()
        },
    )
    .await;
    SensorWorkloadRepository::ensure_default_for_sensor(&pool, sensor.id)
        .await
        .expect("desired release A workload");
    let release_b = create_release(&pool, &pack, "2.0.0", 'b').await;
    activate_projected(
        &pool,
        pack.id,
        release_b.id,
        &PackProjectionIds {
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            ..Default::default()
        },
    )
    .await;

    let worker_id = WorkerRepository::create(
        &pool,
        CreateWorkerInput {
            name: format!("sensor-race-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await
    .expect("worker")
    .id;
    let instance_a = Uuid::new_v4();
    let lease_a = match SensorWorkloadRepository::acquire_or_renew(
        &pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id,
            worker_instance: instance_a,
            lease_seconds: 60,
        },
    )
    .await
    .expect("first lease")
    {
        AcquireSensorWorkloadOutcome::Acquired(lease) => lease,
        AcquireSensorWorkloadOutcome::HeldByOther(_) => panic!("workload unexpectedly held"),
    };
    let first = SensorWorkloadRepository::begin_process(&pool, lease_a)
        .await
        .expect("begin first process")
        .expect("owned first process");
    SensorWorkloadRepository::release(&pool, first.fence())
        .await
        .expect("release first process");

    let lease_b = match SensorWorkloadRepository::acquire_or_renew(
        &pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id,
            worker_instance: Uuid::new_v4(),
            lease_seconds: 60,
        },
    )
    .await
    .expect("replacement lease")
    {
        AcquireSensorWorkloadOutcome::Acquired(lease) => lease,
        AcquireSensorWorkloadOutcome::HeldByOther(_) => panic!("replacement unexpectedly held"),
    };
    let replacement = SensorWorkloadRepository::begin_process(&pool, lease_b)
        .await
        .expect("begin replacement")
        .expect("owned replacement");

    assert_eq!(replacement.pack_release, Some(release_a.id));
    let snapshot = replacement.executable_snapshot.expect("sensor snapshot");
    assert_eq!(snapshot["release"]["id"], release_a.id);
    assert!(replacement.lease_expires_at > Utc::now() + Duration::seconds(30));

    let refreshed = SensorWorkloadRepository::refresh_default_for_sensor(&pool, sensor.id)
        .await
        .expect("refresh desired release");
    assert_eq!(refreshed.pack_release, Some(release_b.id));
}

#[tokio::test]
async fn retention_preserves_active_pinned_and_rollback_window_releases() {
    let (pool, pack, action, release_a, snapshot_a) = action_fixture().await;
    ExecutionRepository::create_pinned(&pool, execution_input(&action, None), &snapshot_a)
        .await
        .expect("durable release pin");
    let release_b = create_release(&pool, &pack, "2.0.0", 'b').await;
    activate(&pool, pack.id, release_b.id).await;
    let release_c = create_release(&pool, &pack, "3.0.0", 'c').await;
    activate(&pool, pack.id, release_c.id).await;
    sqlx::query(
        "UPDATE pack_release SET inactive_since = CASE WHEN id = $1 THEN NOW() - INTERVAL '2 days' ELSE NOW() - INTERVAL '5 minutes' END WHERE id IN ($1, $2)",
    )
    .bind(release_a.id)
    .bind(release_b.id)
    .execute(&*pool)
    .await
    .unwrap();

    let packs = tempfile::tempdir().unwrap();
    let result = PackRetentionRepository::collect(
        &pool,
        packs.path(),
        Utc::now() - Duration::hours(1),
        0,
        10,
    )
    .await
    .unwrap();

    assert_eq!(result.deleted, 0);
    assert!(PackReleaseRepository::find_by_id(&pool, release_a.id)
        .await
        .unwrap()
        .is_some());
    assert!(PackReleaseRepository::find_by_id(&pool, release_b.id)
        .await
        .unwrap()
        .is_some());
    assert!(PackReleaseRepository::find_by_id(&pool, release_c.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn concurrent_pin_commit_wins_before_retention_can_delete_release() {
    let (pool, pack, action, release_a, snapshot_a) = action_fixture().await;
    let release_b = create_release(&pool, &pack, "2.0.0", 'b').await;
    activate(&pool, pack.id, release_b.id).await;
    sqlx::query("UPDATE pack_release SET inactive_since = NOW() - INTERVAL '2 days' WHERE id = $1")
        .bind(release_a.id)
        .execute(&*pool)
        .await
        .unwrap();

    let mut pin_tx = pool.begin().await.unwrap();
    ExecutionRepository::create_pinned(&mut *pin_tx, execution_input(&action, None), &snapshot_a)
        .await
        .unwrap();
    let collector_pool = pool.clone();
    let packs = tempfile::tempdir().unwrap();
    let packs_path = packs.path().to_path_buf();
    let collector = tokio::spawn(async move {
        PackRetentionRepository::collect(
            &collector_pool,
            &packs_path,
            Utc::now() - Duration::hours(1),
            0,
            10,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    pin_tx.commit().await.unwrap();

    assert_eq!(collector.await.unwrap().unwrap().deleted, 0);
    assert!(PackReleaseRepository::find_by_id(&pool, release_a.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn retention_removes_local_tree_only_after_last_shared_reference() {
    let pool = create_test_pool().await.expect("test database");
    let first_pack = PackFixture::new_unique("local_release_first")
        .create(&pool)
        .await
        .unwrap();
    let second_pack = PackFixture::new_unique("local_release_second")
        .create(&pool)
        .await
        .unwrap();
    let packs = tempfile::tempdir().unwrap();
    let digest = "d".repeat(64);
    let release_dir = packs.path().join(".releases/sha256").join(&digest);
    let content_path = release_dir.join("pack");
    std::fs::create_dir_all(&content_path).unwrap();
    std::fs::write(content_path.join("pack.yaml"), "ref: shared\n").unwrap();

    let mut tx = pool.begin().await.unwrap();
    let first = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: first_pack.id,
            pack_ref: first_pack.r#ref,
            version: "1.0.0".into(),
            digest: digest.clone(),
            object_key: "packs/local-shared-first".into(),
            provider_version: "e:first".into(),
            content_path: content_path.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    let second = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: second_pack.id,
            pack_ref: second_pack.r#ref,
            version: "1.0.0".into(),
            digest: digest.clone(),
            object_key: "packs/local-shared-second".into(),
            provider_version: "e:second".into(),
            content_path: content_path.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE pack_release SET inactive_since = NOW() - INTERVAL '2 days' WHERE id = $1")
        .bind(first.id)
        .execute(&*pool)
        .await
        .unwrap();

    let first_result = PackRetentionRepository::collect(
        &pool,
        packs.path(),
        Utc::now() - Duration::hours(1),
        0,
        10,
    )
    .await
    .unwrap();
    assert_eq!(first_result.deleted, 1);
    assert!(release_dir.exists());

    sqlx::query("UPDATE pack_release SET inactive_since = NOW() - INTERVAL '2 days' WHERE id = $1")
        .bind(second.id)
        .execute(&*pool)
        .await
        .unwrap();
    let second_result = PackRetentionRepository::collect(
        &pool,
        packs.path(),
        Utc::now() - Duration::hours(1),
        0,
        10,
    )
    .await
    .unwrap();

    assert_eq!(second_result.deleted, 1);
    assert!(!release_dir.exists());
}

#[tokio::test]
async fn pack_delete_cascade_cleans_release_tree_after_commit() {
    let pool = create_test_pool().await.expect("test database");
    let pack = PackFixture::new_unique("cascade_release_cleanup")
        .create(&pool)
        .await
        .unwrap();
    let packs = tempfile::tempdir().unwrap();
    let digest = "e".repeat(64);
    let release_dir = packs.path().join(".releases/sha256").join(&digest);
    let content_path = release_dir.join("pack");
    std::fs::create_dir_all(&content_path).unwrap();
    std::fs::write(content_path.join("pack.yaml"), "ref: cascade\n").unwrap();

    let mut tx = pool.begin().await.unwrap();
    PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref,
            version: "1.0.0".into(),
            digest,
            object_key: "packs/cascade-release".into(),
            provider_version: "e:cascade".into(),
            content_path: content_path.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut delete_tx = pool.begin().await.unwrap();
    let deleted_content = PackRetentionRepository::content_for_pack(&mut delete_tx, pack.id)
        .await
        .unwrap();
    assert!(PackRepository::delete(&mut *delete_tx, pack.id)
        .await
        .unwrap());
    assert!(release_dir.exists());
    delete_tx.commit().await.unwrap();

    PackRetentionRepository::cleanup_unreferenced(&pool, packs.path(), &deleted_content)
        .await
        .unwrap();
    assert!(!release_dir.exists());
}

#[tokio::test]
async fn deleting_one_release_does_not_collect_a_shared_exact_object() {
    use attune_common::repositories::object_maintenance::ObjectMaintenanceRepository;

    let pool = create_test_pool().await.expect("test database");
    let first_pack = PackFixture::new_unique("shared_first")
        .create(&pool)
        .await
        .unwrap();
    let second_pack = PackFixture::new_unique("shared_second")
        .create(&pool)
        .await
        .unwrap();
    ObjectMaintenanceRepository::reserve_upload(&pool, "packs/shared", "pack")
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(&pool, "packs/shared", "e:shared", 1)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let first = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: first_pack.id,
            pack_ref: first_pack.r#ref,
            version: "1.0.0".into(),
            digest: "d".repeat(64),
            object_key: "packs/shared".into(),
            provider_version: "e:shared".into(),
            content_path: "/packs/first".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: second_pack.id,
            pack_ref: second_pack.r#ref,
            version: "1.0.0".into(),
            digest: "d".repeat(64),
            object_key: "packs/shared".into(),
            provider_version: "e:shared".into(),
            content_path: "/packs/second".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    sqlx::query("DELETE FROM pack_release WHERE id = $1")
        .bind(first.id)
        .execute(&*pool)
        .await
        .unwrap();
    assert!(
        ObjectMaintenanceRepository::exact_reference_exists(&pool, "packs/shared", "e:shared")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn deleting_pack_preserves_historical_release_snapshot() {
    let (pool, pack, action, release, snapshot) = action_fixture().await;
    let mut input = execution_input(&action, None);
    input.status = ExecutionStatus::Completed;
    let execution = ExecutionRepository::create_pinned(&*pool, input, &snapshot)
        .await
        .expect("pinned execution");
    let enforcement = EnforcementRepository::create_or_get_by_rule_event_pinned(
        &mut *pool.acquire().await.unwrap(),
        CreateEnforcementInput {
            rule: None,
            rule_ref: format!("{}.historical_rule", pack.r#ref),
            trigger_ref: format!("{}.historical_trigger", pack.r#ref),
            config: None,
            event: None,
            status: EnforcementStatus::Processed,
            payload: json!({}),
            condition: EnforcementCondition::All,
            conditions: json!([]),
        },
        &snapshot,
    )
    .await
    .expect("historical enforcement")
    .enforcement;
    let queue = WorkQueueRepository::create(
        &*pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.historical", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Historical".to_string(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Immutable,
            batch_mode: WorkQueueBatchMode::Single,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .expect("historical queue");
    let queue_item = WorkQueueItemRepository::create_pinned(
        &*pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref,
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Completed,
            payload: json!({}),
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "test".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
        &snapshot,
    )
    .await
    .expect("historical queue item");

    assert!(PackRepository::delete(&*pool, pack.id)
        .await
        .expect("delete pack with historical pin"));
    assert!(PackReleaseRepository::find_by_id(&*pool, release.id)
        .await
        .unwrap()
        .is_none());

    let historical = ExecutionRepository::find_by_id(&*pool, execution.id)
        .await
        .unwrap()
        .expect("historical execution");
    assert_eq!(historical.pack_release, None);
    assert_eq!(
        historical.pack_release_digest.as_deref(),
        Some(release.digest.as_str())
    );
    assert_eq!(
        historical
            .executable_snapshot
            .as_ref()
            .expect("preserved executable snapshot")
            .release
            .id,
        release.id
    );

    let historical_enforcement = EnforcementRepository::find_by_id(&*pool, enforcement.id)
        .await
        .unwrap()
        .expect("historical enforcement");
    assert_eq!(historical_enforcement.pack_release, None);
    assert_eq!(
        historical_enforcement.pack_release_digest.as_deref(),
        Some(release.digest.as_str())
    );
    assert_eq!(
        historical_enforcement.executable_snapshot.unwrap()["release"]["id"],
        release.id
    );

    let historical_item = WorkQueueItemRepository::find_by_id(&*pool, queue_item.id)
        .await
        .unwrap()
        .expect("historical queue item");
    assert_eq!(historical_item.pack_release, None);
    assert_eq!(
        historical_item.pack_release_digest.as_deref(),
        Some(release.digest.as_str())
    );
    assert_eq!(
        historical_item.executable_snapshot.unwrap()["release"]["id"],
        release.id
    );
}

#[tokio::test]
async fn deleting_pack_rejects_nonterminal_pinned_work() {
    let (pool, pack, action, release, snapshot) = action_fixture().await;
    ExecutionRepository::create_pinned(&*pool, execution_input(&action, None), &snapshot)
        .await
        .expect("nonterminal execution");
    EnforcementRepository::create_or_get_by_rule_event_pinned(
        &mut *pool.acquire().await.unwrap(),
        CreateEnforcementInput {
            rule: None,
            rule_ref: format!("{}.rule", pack.r#ref),
            trigger_ref: format!("{}.trigger", pack.r#ref),
            config: None,
            event: None,
            status: EnforcementStatus::Created,
            payload: json!({}),
            condition: EnforcementCondition::All,
            conditions: json!([]),
        },
        &snapshot,
    )
    .await
    .expect("nonterminal enforcement");

    let queue = WorkQueueRepository::create(
        &*pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.delete_guard", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Delete guard".to_string(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref.clone(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Immutable,
            batch_mode: WorkQueueBatchMode::Single,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .expect("delete guard queue");
    WorkQueueItemRepository::create_pinned(
        &*pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref,
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Queued,
            payload: json!({}),
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "test".to_string(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
        &snapshot,
    )
    .await
    .expect("nonterminal queue item");

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "sensor")
        .create(&pool)
        .await
        .expect("sensor runtime");
    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref,
        "delete_guard",
    )
    .create(&pool)
    .await
    .expect("sensor");
    activate_projected(
        &pool,
        pack.id,
        release.id,
        &PackProjectionIds {
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            ..Default::default()
        },
    )
    .await;
    let worker_id = WorkerRepository::create(
        &*pool,
        CreateWorkerInput {
            name: format!("delete-guard-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await
    .expect("sensor worker")
    .id;
    SensorWorkloadRepository::acquire_or_renew(
        &pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id,
            worker_instance: Uuid::new_v4(),
            lease_seconds: 60,
        },
    )
    .await
    .expect("active sensor workload");

    let error = PackRepository::delete(&*pool, pack.id)
        .await
        .expect_err("live release pins must reject deletion");
    let message = match error {
        attune_common::Error::PackDeletionBlocked(message) => message,
        error => panic!("unexpected deletion error: {error}"),
    };
    assert!(message.contains("nonterminal executions"));
    assert!(message.contains("nonterminal enforcements"));
    assert!(message.contains("nonterminal work queue items"));
    assert!(message.contains("active sensor workloads"));
    assert!(PackRepository::find_by_id(&*pool, pack.id)
        .await
        .unwrap()
        .is_some());
}
