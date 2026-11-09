use attune_common::{
    models::enums::{
        ArtifactBodyState, ArtifactClassification, ArtifactType, ArtifactVisibility,
        ExecutionStatus, LogStreamBackend, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        execution::{CreateExecutionInput, ExecutionRepository},
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        log_stream::LogStreamRepository,
        maintenance::MaintenanceRepository,
        object_maintenance::ObjectMaintenanceRepository,
        storage_maintenance::StorageMaintenanceRepository,
        Create,
    },
};
use chrono::{Duration, Utc};

#[path = "storage_maintenance_repository_tests/bounded_admission.rs"]
mod bounded_admission;
mod helpers;
use helpers::{create_test_pool, unique_test_id};

fn artifact_input(suffix: &str) -> CreateArtifactInput {
    let unique = unique_test_id();
    CreateArtifactInput {
        r#ref: format!("storage_{suffix}_{unique}"),
        scope: OwnerType::System,
        owner: format!("storage_owner_{unique}"),
        r#type: ArtifactType::FileBinary,
        visibility: ArtifactVisibility::Private,
        classification: ArtifactClassification::General,
        retention_policy: RetentionPolicyType::Versions,
        retention_limit: 5,
        name: None,
        description: None,
        content_type: None,
        data: None,
    }
}

// Exercise the same graph-first claim/metadata-delete seam as the supervisor.
// Exact provider cleanup remains separately owned by the deletion ledger.
async fn collect_version_overflow(pool: &sqlx::PgPool, artifact: i64) -> usize {
    let mut deleted = 0;
    for candidate in MaintenanceRepository::find_expired_artifact_versions(pool, 100)
        .await
        .unwrap()
    {
        if candidate.artifact != artifact {
            continue;
        }
        let mut tx = pool.begin().await.unwrap();
        Evidence::lock_reclamation(&mut tx).await.unwrap();
        if MaintenanceRepository::claim_expired_artifact_version(&mut tx, candidate.id)
            .await
            .unwrap()
            .is_some()
            && Evidence::request_cleanup(&mut tx, &[candidate.id])
                .await
                .unwrap()
                == [candidate.id]
            && ArtifactVersionRepository::delete(&mut *tx, candidate.id)
                .await
                .unwrap()
        {
            deleted += 1;
        }
        tx.commit().await.unwrap();
    }
    deleted
}

#[tokio::test]
async fn ledger_rechecks_shared_exact_references_and_retries_stale_claims() {
    let pool = create_test_pool().await.expect("test database");
    ObjectMaintenanceRepository::reserve_upload(&pool, "shared/key", "artifact")
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(&pool, "shared/key", "e:v1", 12)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO artifact_version (artifact, version, content_type, size_bytes, body_state, object_key, provider_version, sha256) \
         SELECT id, 1, 'application/octet-stream', 12, 'ready', 'shared/key', 'e:v1', repeat('a', 64) FROM artifact WHERE id = $1",
    )
    .bind(
        ArtifactRepository::create(&pool, artifact_input("shared"))
            .await
            .unwrap()
            .id,
    )
    .execute(&*pool)
    .await
    .unwrap();
    assert_eq!(
        ObjectMaintenanceRepository::schedule_unreferenced(
            &pool,
            Utc::now() + Duration::hours(1),
            Utc::now(),
        )
        .await
        .unwrap(),
        0
    );
    assert!(
        ObjectMaintenanceRepository::exact_reference_exists(&pool, "shared/key", "e:v1")
            .await
            .unwrap()
    );

    sqlx::query("DELETE FROM artifact_version WHERE object_key = 'shared/key'")
        .execute(&*pool)
        .await
        .unwrap();
    sqlx::query("UPDATE object_maintenance_ledger SET eligible_at = NOW() - INTERVAL '2 hours'")
        .execute(&*pool)
        .await
        .unwrap();
    let first =
        ObjectMaintenanceRepository::claim_deletions(&pool, Utc::now() - Duration::hours(1), 1)
            .await
            .unwrap();
    assert_eq!(first.len(), 1);
    assert!(ObjectMaintenanceRepository::claim_deletions(
        &pool,
        Utc::now() - Duration::hours(1),
        1,
    )
    .await
    .unwrap()
    .is_empty());
    sqlx::query("UPDATE object_maintenance_ledger SET updated = NOW() - INTERVAL '2 hours'")
        .execute(&*pool)
        .await
        .unwrap();
    let retry =
        ObjectMaintenanceRepository::claim_deletions(&pool, Utc::now() - Duration::hours(1), 1)
            .await
            .unwrap();
    assert_eq!(retry[0].attempts, 2);
}

#[tokio::test]
async fn claimed_deletion_cannot_be_revived_by_upload_calls() {
    let pool = create_test_pool().await.expect("test database");
    ObjectMaintenanceRepository::reserve_upload(&pool, "claimed/key", "artifact")
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(&pool, "claimed/key", "e:v1", 12)
        .await
        .unwrap();
    ObjectMaintenanceRepository::schedule_unreferenced(
        &pool,
        Utc::now() + Duration::hours(1),
        Utc::now() - Duration::hours(2),
    )
    .await
    .unwrap();
    let claimed =
        ObjectMaintenanceRepository::claim_deletions(&pool, Utc::now() - Duration::hours(1), 1)
            .await
            .unwrap();
    assert_eq!(claimed.len(), 1);

    assert!(
        ObjectMaintenanceRepository::reserve_upload(&pool, "claimed/key", "artifact")
            .await
            .is_err()
    );
    assert!(
        ObjectMaintenanceRepository::record_uploaded(&pool, "claimed/key", "e:v1", 12)
            .await
            .is_err()
    );
    let state: String = sqlx::query_scalar(
        "SELECT state FROM object_maintenance_ledger WHERE object_key = 'claimed/key'",
    )
    .fetch_one(&*pool)
    .await
    .unwrap();
    assert_eq!(state, "deleting");
}

#[tokio::test]
async fn stale_upload_cleanup_does_not_delete_a_refreshed_reservation() {
    let pool = create_test_pool().await.expect("test database");
    ObjectMaintenanceRepository::reserve_upload(&pool, "refreshed/key", "artifact")
        .await
        .unwrap();
    sqlx::query(
        "UPDATE object_maintenance_ledger SET updated = NOW() - INTERVAL '2 hours' \
         WHERE object_key = 'refreshed/key'",
    )
    .execute(&*pool)
    .await
    .unwrap();

    let observed =
        ObjectMaintenanceRepository::stale_uploads(&pool, Utc::now() - Duration::hours(1), 1)
            .await
            .unwrap()
            .pop()
            .expect("stale reservation");
    ObjectMaintenanceRepository::reserve_upload(&pool, "refreshed/key", "artifact")
        .await
        .unwrap();

    assert!(!ObjectMaintenanceRepository::remove_stale_upload(
        &pool,
        observed.id,
        observed.updated,
    )
    .await
    .unwrap());
    let state: String = sqlx::query_scalar(
        "SELECT state FROM object_maintenance_ledger WHERE object_key = 'refreshed/key'",
    )
    .fetch_one(&*pool)
    .await
    .unwrap();
    assert_eq!(state, "uploading");
}

#[tokio::test]
async fn upload_reservation_wins_or_claim_rechecks_exact_references() {
    let pool = create_test_pool().await.expect("test database");
    ObjectMaintenanceRepository::reserve_upload(&pool, "republished/key", "artifact")
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(&pool, "republished/key", "e:v1", 12)
        .await
        .unwrap();
    ObjectMaintenanceRepository::schedule_unreferenced(
        &pool,
        Utc::now() + Duration::hours(1),
        Utc::now() - Duration::hours(2),
    )
    .await
    .unwrap();

    ObjectMaintenanceRepository::reserve_upload(&pool, "republished/key", "artifact")
        .await
        .unwrap();
    assert_eq!(
        ObjectMaintenanceRepository::schedule_unreferenced(
            &pool,
            Utc::now() - Duration::hours(1),
            Utc::now() - Duration::hours(2),
        )
        .await
        .unwrap(),
        0
    );
    assert!(ObjectMaintenanceRepository::claim_deletions(
        &pool,
        Utc::now() - Duration::hours(1),
        1,
    )
    .await
    .unwrap()
    .is_empty());

    ObjectMaintenanceRepository::schedule_unreferenced(
        &pool,
        Utc::now() + Duration::hours(1),
        Utc::now() - Duration::hours(2),
    )
    .await
    .unwrap();
    let artifact = ArtifactRepository::create(&pool, artifact_input("republished"))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO artifact_version (artifact, version, content_type, size_bytes, body_state, object_key, provider_version, sha256) \
         VALUES ($1, 1, 'application/octet-stream', 12, 'ready', 'republished/key', 'e:v1', repeat('a', 64))",
    )
    .bind(artifact.id)
    .execute(&*pool)
    .await
    .unwrap();
    assert!(ObjectMaintenanceRepository::claim_deletions(
        &pool,
        Utc::now() - Duration::hours(1),
        1,
    )
    .await
    .unwrap()
    .is_empty());
}

#[tokio::test]
async fn ready_object_pages_advance_past_a_healthy_prefix() {
    let pool = create_test_pool().await.expect("test database");
    let artifact = ArtifactRepository::create(&pool, artifact_input("ready_pages"))
        .await
        .unwrap();
    let mut ids = Vec::new();
    for version in 1..=3 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO artifact_version (artifact, version, content_type, size_bytes, body_state, object_key, provider_version, sha256) \
             VALUES ($1, $2, 'application/octet-stream', 12, 'ready', $3, $4, repeat('a', 64)) RETURNING id",
        )
        .bind(artifact.id)
        .bind(version)
        .bind(format!("ready/page/{version}"))
        .bind(format!("e:v{version}"))
        .fetch_one(&*pool)
        .await
        .unwrap();
        ids.push(id);
    }

    let first = StorageMaintenanceRepository::ready_objects(&pool, 0, 2)
        .await
        .unwrap();
    let second = StorageMaintenanceRepository::ready_objects(&pool, first[1].id, 2)
        .await
        .unwrap();
    assert_eq!(first.iter().map(|row| row.id).collect::<Vec<_>>(), ids[..2]);
    assert_eq!(second[0].id, ids[2]);
}

#[tokio::test]
async fn versions_policy_ignores_pending_uploads_and_defers_ready_cleanup_to_graph_first_gc() {
    let pool = create_test_pool().await.expect("test database");
    let mut input = artifact_input("versions");
    input.retention_limit = 1;
    let artifact = ArtifactRepository::create(&pool, input).await.unwrap();

    let first = ArtifactVersionRepository::create_object_pending(
        &pool,
        artifact.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let first_key = first.object_key.as_deref().unwrap();
    ObjectMaintenanceRepository::reserve_upload(&pool, first_key, "artifact")
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(&pool, first_key, "e:version-1", 1)
        .await
        .unwrap();
    ArtifactVersionRepository::mark_body_ready(&pool, first.id, "e:version-1", 1, &"a".repeat(64))
        .await
        .unwrap();

    let abandoned = ArtifactVersionRepository::create_object_pending(
        &pool,
        artifact.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(ArtifactVersionRepository::find_by_id(&pool, first.id)
        .await
        .unwrap()
        .is_some());
    assert!(StorageMaintenanceRepository::claim_abandoned_pending(
        &pool,
        abandoned.id,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap());
    assert!(
        StorageMaintenanceRepository::delete_cleanup_claimed(&pool, abandoned.id,)
            .await
            .unwrap()
    );

    let second = ArtifactVersionRepository::create_object_pending(
        &pool,
        artifact.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let second_key = second.object_key.as_deref().unwrap();
    ObjectMaintenanceRepository::reserve_upload(&pool, second_key, "artifact")
        .await
        .unwrap();
    ObjectMaintenanceRepository::record_uploaded(&pool, second_key, "e:version-2", 2)
        .await
        .unwrap();
    ArtifactVersionRepository::mark_body_ready(&pool, second.id, "e:version-2", 2, &"b".repeat(64))
        .await
        .unwrap();

    assert_eq!(
        ArtifactVersionRepository::count_by_artifact(&pool, artifact.id)
            .await
            .unwrap(),
        2,
        "Ready publication must not perform reclamation in its effect transaction"
    );
    assert_eq!(collect_version_overflow(&pool, artifact.id).await, 1);
    assert_eq!(
        ArtifactVersionRepository::count_by_artifact(&pool, artifact.id)
            .await
            .unwrap(),
        1
    );
    assert!(ArtifactVersionRepository::find_by_id(&pool, first.id)
        .await
        .unwrap()
        .is_none());
    assert!(ArtifactVersionRepository::find_by_id(&pool, second.id)
        .await
        .unwrap()
        .is_some());
    let queued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM object_maintenance_ledger WHERE state = 'deletion_pending'",
    )
    .fetch_one(&*pool)
    .await
    .unwrap();
    assert_eq!(queued, 1);
}

#[tokio::test]
async fn concurrent_ready_transitions_defer_the_versions_limit_to_graph_first_gc() {
    let pool = create_test_pool().await.expect("test database");
    let mut input = artifact_input("concurrent_versions");
    input.retention_limit = 1;
    let artifact = ArtifactRepository::create(&pool, input).await.unwrap();
    let first = ArtifactVersionRepository::create_object_pending(
        &pool,
        artifact.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let second = ArtifactVersionRepository::create_object_pending(
        &pool,
        artifact.id,
        None,
        "application/octet-stream".into(),
        None,
        None,
    )
    .await
    .unwrap();

    let first_digest = "a".repeat(64);
    let second_digest = "b".repeat(64);
    let first_ready =
        ArtifactVersionRepository::mark_body_ready(&pool, first.id, "e:first", 1, &first_digest);
    let second_ready =
        ArtifactVersionRepository::mark_body_ready(&pool, second.id, "e:second", 1, &second_digest);
    let (first_result, second_result) = tokio::join!(first_ready, second_ready);
    first_result.unwrap();
    second_result.unwrap();

    assert_eq!(
        ArtifactVersionRepository::count_by_artifact(&pool, artifact.id)
            .await
            .unwrap(),
        2
    );
    assert_eq!(collect_version_overflow(&pool, artifact.id).await, 1);
    assert_eq!(
        ArtifactVersionRepository::count_by_artifact(&pool, artifact.id)
            .await
            .unwrap(),
        1
    );
    assert!(ArtifactVersionRepository::find_by_id(&pool, second.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn object_lifecycle_recovery_is_delayed_and_idempotent() {
    let pool = create_test_pool().await.expect("test database");
    let artifact = ArtifactRepository::create(&pool, artifact_input("pending"))
        .await
        .unwrap();
    let pending = ArtifactVersionRepository::create_object_pending(
        &pool,
        artifact.id,
        None,
        "application/octet-stream".to_string(),
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(pending.id)
    .execute(&*pool)
    .await
    .unwrap();

    let abandoned =
        StorageMaintenanceRepository::abandoned_pending(&pool, Utc::now() - Duration::hours(1), 10)
            .await
            .unwrap();
    assert_eq!(abandoned.len(), 1);
    assert!(StorageMaintenanceRepository::mark_deleting_from_pending(
        &pool,
        pending.id,
        "e:version-1",
        12,
        &"a".repeat(64),
    )
    .await
    .unwrap());
    assert!(!StorageMaintenanceRepository::mark_deleting_from_pending(
        &pool,
        pending.id,
        "e:version-1",
        12,
        &"a".repeat(64),
    )
    .await
    .unwrap());
    assert!(StorageMaintenanceRepository::deleting_objects(
        &pool,
        Utc::now() - Duration::hours(1),
        10,
    )
    .await
    .unwrap()
    .is_empty());

    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(pending.id)
    .execute(&*pool)
    .await
    .unwrap();
    let deleting =
        StorageMaintenanceRepository::deleting_objects(&pool, Utc::now() - Duration::hours(1), 10)
            .await
            .unwrap();
    assert_eq!(deleting[0].provider_version.as_deref(), Some("e:version-1"));
    assert!(
        StorageMaintenanceRepository::delete_deleting(&pool, pending.id)
            .await
            .unwrap()
    );
    assert!(
        !StorageMaintenanceRepository::delete_deleting(&pool, pending.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn abandoned_shared_logs_are_selected_only_after_the_pending_grace() {
    let pool = create_test_pool().await.expect("test database");
    let artifact = ArtifactRepository::create(&pool, artifact_input("shared_pending"))
        .await
        .unwrap();
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: None,
            action_ref: "core.test".to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Running,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    let pending = ArtifactVersionRepository::create_log_pending(
        &pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::SharedFile,
        "text/plain".to_string(),
        Some(execution.id),
        None,
        Some("worker".to_string()),
    )
    .await
    .unwrap();
    LogStreamRepository::create_with_backend(
        &pool,
        pending.id,
        LogStreamBackend::SharedFile,
        1024,
        500,
    )
    .await
    .unwrap();

    let cutoff = Utc::now() - Duration::hours(1);
    assert!(
        StorageMaintenanceRepository::abandoned_shared_log_pending(&pool, cutoff, 10)
            .await
            .unwrap()
            .is_empty()
    );

    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(pending.id)
    .execute(&*pool)
    .await
    .unwrap();
    assert!(
        StorageMaintenanceRepository::abandoned_shared_log_pending(&pool, cutoff, 10)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE execution SET status = 'abandoned' WHERE id = $1")
        .bind(execution.id)
        .execute(&*pool)
        .await
        .unwrap();
    let abandoned = StorageMaintenanceRepository::abandoned_shared_log_pending(&pool, cutoff, 10)
        .await
        .unwrap();
    assert_eq!(abandoned.len(), 1);
    assert_eq!(abandoned[0].id, pending.id);
    assert_eq!(abandoned[0].file_path, pending.file_path.unwrap());

    assert!(StorageMaintenanceRepository::claim_abandoned_shared_log_pending(
        &pool, pending.id, cutoff,
    )
    .await
    .unwrap());
    assert!(StorageMaintenanceRepository::claim_abandoned_shared_log_pending(
        &pool, pending.id, cutoff,
    )
    .await
    .unwrap());
    assert!(
        StorageMaintenanceRepository::delete_cleanup_claimed(&pool, pending.id)
            .await
            .unwrap()
    );
    assert!(
        !StorageMaintenanceRepository::delete_cleanup_claimed(&pool, pending.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn active_object_logs_are_not_abandoned_even_when_the_body_timestamp_is_old() {
    let pool = create_test_pool().await.expect("test database");
    let artifact = ArtifactRepository::create(&pool, artifact_input("active_object_log"))
        .await
        .unwrap();
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: None,
            action_ref: "core.test".to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Running,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    let pending = ArtifactVersionRepository::create_log_pending(
        &pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::ObjectSegments,
        "text/plain".to_string(),
        Some(execution.id),
        None,
        Some("worker".to_string()),
    )
    .await
    .unwrap();
    LogStreamRepository::create(&pool, pending.id, 1024, 500)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(pending.id)
    .execute(&*pool)
    .await
    .unwrap();
    let cutoff = Utc::now() - Duration::hours(1);

    assert!(
        StorageMaintenanceRepository::abandoned_pending(&pool, cutoff, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !StorageMaintenanceRepository::claim_abandoned_pending(&pool, pending.id, cutoff)
            .await
            .unwrap()
    );

    sqlx::query("UPDATE execution SET status = 'failed' WHERE id = $1")
        .bind(execution.id)
        .execute(&*pool)
        .await
        .unwrap();
    let abandoned = StorageMaintenanceRepository::abandoned_pending(&pool, cutoff, 10)
        .await
        .unwrap();
    assert_eq!(abandoned.len(), 1);
    assert_eq!(abandoned[0].id, pending.id);
    assert!(
        StorageMaintenanceRepository::claim_abandoned_pending(&pool, pending.id, cutoff)
            .await
            .unwrap()
    );
    let stream = LogStreamRepository::find_by_artifact_version(&pool, pending.id)
        .await
        .unwrap()
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let claimed_stream = LogStreamRepository::lock(&mut tx, stream.id).await.unwrap();
    assert!(LogStreamRepository::commit_segment(
        &mut tx,
        &claimed_stream,
        0,
        1,
        &"a".repeat(64),
        "logs/claimed/0",
        "provider-0",
    )
    .await
    .is_err());
}

#[tokio::test]
async fn shared_log_seal_and_cleanup_claim_have_exactly_one_winner() {
    let pool = create_test_pool().await.expect("test database");
    let artifact = ArtifactRepository::create(&pool, artifact_input("seal_cleanup_race"))
        .await
        .unwrap();
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: None,
            action_ref: "core.test".to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Failed,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    let pending = ArtifactVersionRepository::create_log_pending(
        &pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::SharedFile,
        "text/plain".to_string(),
        Some(execution.id),
        None,
        Some("worker".to_string()),
    )
    .await
    .unwrap();
    let stream = LogStreamRepository::create_with_backend(
        &pool,
        pending.id,
        LogStreamBackend::SharedFile,
        1024,
        500,
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(pending.id)
    .execute(&*pool)
    .await
    .unwrap();
    let cutoff = Utc::now() - Duration::hours(1);

    let seal_pool = pool.clone();
    let seal = async move {
        let mut tx = seal_pool.begin().await.unwrap();
        let current = ArtifactVersionRepository::find_by_id_for_update(&mut tx, pending.id)
            .await
            .unwrap()
            .unwrap();
        let locked = LogStreamRepository::lock(&mut tx, stream.id).await.unwrap();
        if current.body_state != Some(ArtifactBodyState::Pending) {
            return false;
        }
        LogStreamRepository::seal_shared_file(&mut tx, locked.id, 4, false)
            .await
            .unwrap();
        let ready = ArtifactVersionRepository::mark_log_body_ready_in_transaction(
            &mut tx,
            pending.id,
            4,
            &"a".repeat(64),
        )
        .await
        .unwrap()
        .is_some();
        if ready {
            tx.commit().await.unwrap();
        }
        ready
    };
    let claim =
        StorageMaintenanceRepository::claim_abandoned_shared_log_pending(&pool, pending.id, cutoff);
    let (sealed, claimed) = tokio::join!(seal, claim);
    let claimed = claimed.unwrap();

    assert_ne!(sealed, claimed);
    let final_version = ArtifactVersionRepository::find_by_id(&pool, pending.id)
        .await
        .unwrap()
        .unwrap();
    let final_stream = LogStreamRepository::find_by_artifact_version(&pool, pending.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        final_version.body_state == Some(ArtifactBodyState::Ready),
        sealed
    );
    assert_eq!(
        final_version.body_state == Some(ArtifactBodyState::CleanupClaimed),
        claimed
    );
    assert_eq!(final_stream.sealed, sealed);
}

#[tokio::test]
async fn migration_switch_is_atomic_and_preserves_snapshot_pointer() {
    let pool = create_test_pool().await.expect("test database");
    let mut input = artifact_input("legacy");
    input.r#type = ArtifactType::FileBinary;
    let artifact = ArtifactRepository::create(&pool, input).await.unwrap();
    let legacy = ArtifactVersionRepository::create(
        &pool,
        CreateArtifactVersionInput {
            artifact: artifact.id,
            execution: None,
            content_type: Some("application/octet-stream".to_string()),
            content: None,
            content_json: None,
            file_path: Some("legacy/v1.bin".to_string()),
            meta: None,
            created_by: None,
        },
    )
    .await
    .unwrap();
    let expires = Utc::now() + Duration::days(7);
    let mut tx = pool.begin().await.unwrap();
    assert!(StorageMaintenanceRepository::switch_artifact_file(
        &mut tx,
        legacy.id,
        "artifacts/1/v1",
        "e:version-1",
        7,
        &"b".repeat(64),
        expires,
    )
    .await
    .unwrap());
    tx.commit().await.unwrap();

    let switched = ArtifactVersionRepository::find_by_id(&pool, legacy.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(switched.body_state, Some(ArtifactBodyState::Ready));
    assert_eq!(switched.file_path.as_deref(), Some("legacy/v1.bin"));

    let mut retry = pool.begin().await.unwrap();
    assert!(!StorageMaintenanceRepository::switch_artifact_file(
        &mut retry,
        legacy.id,
        "artifacts/1/v1",
        "e:version-1",
        7,
        &"b".repeat(64),
        expires,
    )
    .await
    .unwrap());
    retry.rollback().await.unwrap();
}

#[tokio::test]
async fn body_discovery_bounds_windows_and_reaches_past_pinned_prefix() {
    let db = create_test_pool().await.unwrap();
    let artifact = ArtifactRepository::create(&db, artifact_input("discovery"))
        .await
        .unwrap();
    let ids: Vec<i64> = sqlx::query_scalar("INSERT INTO artifact_version (artifact,version,body_state,object_key,body_updated) SELECT $1,n,'pending','owned-discovery/'||n, NOW()-interval '2 days' FROM generate_series(1,17) n RETURNING id")
        .bind(artifact.id).fetch_all(&*db).await.unwrap();
    sqlx::query("UPDATE artifact_version SET body_updated=NOW()+INTERVAL '1 hour' WHERE id=$1")
        .bind(ids[0])
        .execute(&*db)
        .await
        .unwrap();
    let owner = ids.last().unwrap() + 100000;
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read) VALUES($1,$2,$1)")
        .bind(owner)
        .bind(artifact.id)
        .execute(&*db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) SELECT $1,'artifact_version',unnest($2::bigint[])").bind(owner).bind(&ids[1..16]).execute(&*db).await.unwrap();
    let mut found = Vec::new();
    for page in 0..5 {
        let candidates = StorageMaintenanceRepository::abandoned_pending(
            &db,
            Utc::now() - Duration::hours(1),
            4,
        )
        .await
        .unwrap();
        assert!(candidates.len() <= 4);
        found.extend(candidates.into_iter().map(|candidate| candidate.id));
        assert_eq!(
            StorageMaintenanceRepository::abandoned_discovery_pending(&db)
                .await
                .unwrap(),
            page < 4
        );
    }
    assert_eq!(
        found,
        vec![ids[16]],
        "pinned prefix must not starve the later eligible row"
    );
    let cursor: i64 =
        sqlx::query_scalar("SELECT last_id FROM object_body_discovery WHERE kind='abandoned'")
            .fetch_one(&*db)
            .await
            .unwrap();
    assert_eq!(cursor, ids[16]);
    // A completed sweep wraps; none of the retained pinned rows becomes cleanup authority.
    assert!(StorageMaintenanceRepository::abandoned_pending(
        &db,
        Utc::now() - Duration::hours(1),
        4
    )
    .await
    .unwrap()
    .is_empty());
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artifact_version WHERE id=ANY($1)")
            .bind(&ids)
            .fetch_one(&*db)
            .await
            .unwrap();
    assert_eq!(retained, 17);
    // Eligibility can change behind the cursor. Continuous new arrivals must
    // not postpone revisiting the earlier, newly old-enough row beyond a finite sweep.
    sqlx::query("UPDATE artifact_version SET body_updated=NOW()-INTERVAL '2 days' WHERE id=$1")
        .bind(ids[0])
        .execute(&*db)
        .await
        .unwrap();
    let mut revisited = false;
    for page in 0..10 {
        sqlx::query("INSERT INTO artifact_version (artifact,version,body_state,object_key,body_updated) VALUES($1,$2,'pending',$3,NOW()-interval '2 days')")
            .bind(artifact.id).bind(18 + page).bind(format!("owned-discovery/new-{page}"))
            .execute(&*db).await.unwrap();
        let candidates = StorageMaintenanceRepository::abandoned_pending(
            &db,
            Utc::now() - Duration::hours(1),
            4,
        )
        .await
        .unwrap();
        assert!(candidates.len() <= 4);
        if candidates.iter().any(|candidate| candidate.id == ids[0]) {
            revisited = true;
            break;
        }
    }
    assert!(
        revisited,
        "fixed high-water sweeps must revisit changed eligibility despite inserts"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn body_discovery_workload_visits_bounded_windows_through_ineligible_prefix() {
    let db = create_test_pool().await.unwrap();
    let artifact = ArtifactRepository::create(&db, artifact_input("discovery_workload"))
        .await
        .unwrap();
    // Pending bodies with retained evidence reproduce an eligibility-filtered
    // prefix: only the final unpinned body can be reclaimed.
    let ids: Vec<i64> = sqlx::query_scalar("INSERT INTO artifact_version (artifact,version,body_state,object_key,body_updated) SELECT $1,n,'pending','owned-workload/'||n,NOW()-interval '2 days' FROM generate_series(1,4097) n ORDER BY n RETURNING id")
        .bind(artifact.id).fetch_all(&*db).await.unwrap();
    let tail = *ids.last().unwrap();
    let owner = tail + 100000;
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read) VALUES($1,$2,$1)")
        .bind(owner)
        .bind(artifact.id)
        .execute(&*db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) SELECT $1,'artifact_version',unnest($2::bigint[])")
        .bind(owner).bind(&ids[..4096]).execute(&*db).await.unwrap();
    sqlx::query("ANALYZE artifact_version, file_evidence_pin, file_evidence_owner")
        .execute(&*db)
        .await
        .unwrap();
    let explain = std::env::var_os("ATTUNE_BODY_DISCOVERY_EXPLAIN").is_some();
    let search_path = format!("{},public", db.schema());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            Box::pin(async move {
                if explain {
                    // Optional owned-fixture instrumentation, never required from
                    // production roles or ordinary correctness test environments.
                    for statement in [
                        "LOAD 'auto_explain'",
                        "SET auto_explain.log_min_duration=0",
                        "SET auto_explain.log_analyze=on",
                        "SET auto_explain.log_buffers=on",
                        "SET auto_explain.log_format='json'",
                    ] {
                        sqlx::query(statement).execute(&mut *conn).await?;
                    }
                }
                Ok(())
            })
        })
        .connect_with(
            db.connect_options()
                .as_ref()
                .clone()
                .options([("search_path", search_path.as_str())]),
        )
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let mut found = Vec::new();
    for page in 0..17 {
        let before = std::time::Instant::now();
        let candidates = StorageMaintenanceRepository::abandoned_pending(
            &pool,
            Utc::now() - Duration::hours(1),
            256,
        )
        .await
        .unwrap();
        eprintln!(
            "body discovery page={page} candidates={} elapsed_us={}",
            candidates.len(),
            before.elapsed().as_micros()
        );
        assert!(candidates.len() <= 256);
        found.extend(candidates.into_iter().map(|candidate| candidate.id));
        assert_eq!(
            StorageMaintenanceRepository::abandoned_discovery_pending(&pool)
                .await
                .unwrap(),
            page < 16
        );
    }
    assert_eq!(found, vec![tail]);
    eprintln!(
        "body discovery rows=4097 windows=17 total_us={}",
        started.elapsed().as_micros()
    );
    pool.close().await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn expiration_prefilter_matches_count_reference_and_claim_rechecks_policy() {
    use attune_common::repositories::maintenance_admission::MaintenanceBudget;
    let pool = create_test_pool().await.unwrap();
    // Cross policies/limits with gapped versions and mixed body states. Ready
    // shared-file bodies and legacy NULL bodies count; pending bodies do not.
    for policy in ["versions", "days", "hours", "minutes"] {
        for limit in [-1, 0, 1, 3, 9] {
            let artifact = ArtifactRepository::create(&pool, artifact_input("matrix"))
                .await
                .unwrap();
            sqlx::query("UPDATE artifact SET retention_policy=$2::text::artifact_retention_enum,retention_limit=$3 WHERE id=$1")
                .bind(artifact.id)
                .bind(policy)
                .bind(limit)
                .execute(&*pool)
                .await
                .unwrap();
            for (index, version) in [1, 3, 8, 12, 20].into_iter().enumerate() {
                let state = [None, Some("ready"), Some("pending"), None, Some("ready")][index];
                let id: i64 = sqlx::query_scalar("INSERT INTO artifact_version(artifact,version,body_state,file_path,size_bytes,sha256,created)
                    VALUES($1,$2,$3::text::artifact_body_state_enum,'matrix/body',CASE WHEN $3='ready' THEN 1 END,CASE WHEN $3='ready' THEN repeat('a',64) END,
                      NOW()-make_interval(secs=>$4::double precision)) RETURNING id")
                    .bind(artifact.id).bind(version).bind(state).bind([172800,7200,60,0,-3600][index])
                    .fetch_one(&*pool).await.unwrap();
                if index == 0 || index == 1 {
                    sqlx::query("INSERT INTO log_stream(artifact_version,backend,max_unflushed_bytes,max_unflushed_milliseconds) VALUES($1,$2::text::log_stream_backend_enum,1024,1000)")
                        .bind(id).bind(if index==0 {"object_segments"} else {"shared_file"}).execute(&*pool).await.unwrap();
                }
            }
        }
    }
    let pinned: i64 = sqlx::query_scalar("SELECT v.id FROM artifact_version v JOIN artifact a ON a.id=v.artifact WHERE a.retention_policy='versions' AND a.retention_limit=1 AND v.version=3")
        .fetch_one(&*pool).await.unwrap();
    let mut capture = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(990011,990012,990013,true,true)")
        .execute(&mut *capture).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_pin(owner,kind,source) VALUES(990011,'artifact_version',$1)",
    )
    .bind(pinned)
    .execute(&mut *capture)
    .await
    .unwrap();
    capture.commit().await.unwrap();
    // Deliberately retain the original count-all predicate as an independent
    // oracle. Stable now() within the reference is safe: fixtures straddle the
    // cutoffs by minutes/hours, rather than racing wall-clock microseconds.
    let expected: Vec<i64> = sqlx::query_scalar("SELECT av.id FROM artifact_version av JOIN artifact a ON a.id=av.artifact
        WHERE file_evidence_version_eligible(av.id) AND (av.body_state IS NULL OR av.body_state='ready') AND (
          (a.retention_policy='days' AND av.created<NOW()-make_interval(days=>a.retention_limit)) OR
          (a.retention_policy='hours' AND av.created<NOW()-make_interval(hours=>a.retention_limit)) OR
          (a.retention_policy='minutes' AND av.created<NOW()-make_interval(mins=>a.retention_limit)) OR
          (a.retention_policy='versions' AND a.retention_limit>0 AND NOT EXISTS(SELECT 1 FROM log_stream s WHERE s.artifact_version=av.id AND s.backend<>'shared_file' AND NOT s.sealed)
            AND (SELECT count(*) FROM artifact_version n WHERE n.artifact=av.artifact AND n.version>av.version AND (n.body_state IS NULL OR n.body_state='ready'))>=a.retention_limit))
        ORDER BY av.created,av.id")
        .fetch_all(&*pool).await.unwrap();
    assert!(!expected.is_empty());
    assert!(
        !expected.contains(&pinned),
        "preexisting evidence pin must exclude discovery"
    );
    let found = MaintenanceRepository::find_expired_artifact_versions(&pool, 1000)
        .await
        .unwrap();
    assert_eq!(found.iter().map(|v| v.id).collect::<Vec<_>>(), expected);
    assert_eq!(
        MaintenanceRepository::expired_artifact_version_count(&pool)
            .await
            .unwrap(),
        expected.len() as i64
    );
    let bounded = MaintenanceRepository::expired_artifact_candidates_bounded(
        &pool,
        7,
        &MaintenanceBudget {
            statement_timeout: std::time::Duration::from_secs(5),
            page_timeout: std::time::Duration::from_secs(10),
            ..MaintenanceBudget::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        bounded.iter().map(|v| v.id).collect::<Vec<_>>(),
        expected[..7]
    );
    let candidate = &found[0];
    // Policy mutation between discovery and claim must invalidate the advisory
    // candidate, even though its physical identity has not changed.
    sqlx::query("UPDATE artifact SET retention_policy='versions',retention_limit=999 WHERE id=$1")
        .bind(candidate.artifact)
        .execute(&*pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(
        MaintenanceRepository::claim_expired_artifact_version(&mut tx, candidate.id)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    // An unchanged eligible identity still passes authoritative claim.
    let candidate = found
        .iter()
        .find(|v| v.artifact != candidate.artifact)
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(
        MaintenanceRepository::claim_expired_artifact_version(&mut tx, candidate.id)
            .await
            .unwrap()
            .is_some()
    );
    tx.rollback().await.unwrap();
    let mut capture = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,object_done,staging_done) VALUES(990001,990002,990003,true,true)")
        .execute(&mut *capture).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_pin(owner,kind,source) VALUES(990001,'artifact_version',$1)",
    )
    .bind(candidate.id)
    .execute(&mut *capture)
    .await
    .unwrap();
    capture.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(
        MaintenanceRepository::claim_expired_artifact_version(&mut tx, candidate.id)
            .await
            .unwrap()
            .is_none(),
        "a pin committed after discovery must deny claim"
    );
    tx.rollback().await.unwrap();
    pool.cleanup().await.unwrap();
}
