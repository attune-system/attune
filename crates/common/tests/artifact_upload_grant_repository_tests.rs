mod helpers;

use attune_common::{
    models::enums::ArtifactUploadGrantState,
    models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        artifact_upload_grant::{ArtifactUploadGrantRepository, CreateArtifactUploadGrantInput},
        object_maintenance::ObjectMaintenanceRepository,
        storage_maintenance::StorageMaintenanceRepository,
        Create,
    },
};
use chrono::{Duration, Utc};
use uuid::Uuid;

use helpers::{create_test_pool, unique_test_id};

async fn pending_version(
    pool: &sqlx::PgPool,
) -> attune_common::models::artifact_version::ArtifactVersion {
    let unique = unique_test_id();
    let artifact = ArtifactRepository::create(
        pool,
        CreateArtifactInput {
            r#ref: format!("direct_upload_{unique}"),
            scope: OwnerType::System,
            owner: "artifact-upload-grant-test".to_string(),
            r#type: ArtifactType::FileBinary,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: None,
            description: None,
            content_type: Some("application/octet-stream".to_string()),
            data: None,
        },
    )
    .await
    .unwrap();
    ArtifactVersionRepository::create_object_pending(
        pool,
        artifact.id,
        None,
        "application/octet-stream".to_string(),
        None,
        Some("test".to_string()),
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn grant_creation_lock_and_completion_are_transactional_and_idempotent() {
    let database = create_test_pool().await.expect("test database");
    let pool = database.pool();
    let version = pending_version(pool).await;
    let object_key = version.object_key.clone().unwrap();
    let grant_token = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::minutes(10);
    let settle_until = expires_at + Duration::minutes(5);

    let mut create_tx = pool.begin().await.unwrap();
    ObjectMaintenanceRepository::reserve_upload(&mut *create_tx, &object_key, "artifact")
        .await
        .unwrap();
    ArtifactUploadGrantRepository::create(
        &mut *create_tx,
        CreateArtifactUploadGrantInput {
            token: grant_token,
            artifact_version: version.id,
            segment_sequence: None,
            object_key: object_key.clone(),
            expected_size: 12,
            expected_sha256: "a".repeat(64),
            content_type: "application/octet-stream".to_string(),
            expires_at,
            settle_until,
        },
    )
    .await
    .unwrap();
    create_tx.commit().await.unwrap();

    let found = ArtifactUploadGrantRepository::find_by_artifact_version(pool, version.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.token, grant_token);
    assert_eq!(found.state, ArtifactUploadGrantState::Issued);

    let mut lock_tx = pool.begin().await.unwrap();
    let locked = ArtifactUploadGrantRepository::find_by_token_for_update(&mut lock_tx, grant_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(locked.artifact_version, version.id);

    let mut competing_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *competing_tx)
        .await
        .unwrap();
    let blocked =
        sqlx::query("UPDATE artifact_upload_grant SET content_type = content_type WHERE id = $1")
            .bind(locked.id)
            .execute(&mut *competing_tx)
            .await;
    assert!(blocked.is_err());
    competing_tx.rollback().await.unwrap();

    assert!(ArtifactUploadGrantRepository::mark_completed(
        &mut lock_tx,
        locked.id,
        "provider-version-1",
    )
    .await
    .unwrap());
    assert!(!ArtifactUploadGrantRepository::mark_completed(
        &mut lock_tx,
        locked.id,
        "provider-version-2",
    )
    .await
    .unwrap());
    lock_tx.commit().await.unwrap();

    let completed = ArtifactUploadGrantRepository::find_by_artifact_version(pool, version.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.state, ArtifactUploadGrantState::Completed);
    assert_eq!(
        completed.completed_provider_version.as_deref(),
        Some("provider-version-1")
    );
    assert!(completed.completed_at.is_some());
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn issued_grant_protects_object_and_pending_artifact_cleanup_until_settled() {
    let database = create_test_pool().await.expect("test database");
    let pool = database.pool();
    let version = pending_version(pool).await;
    let object_key = version.object_key.clone().unwrap();
    let grant_token = Uuid::new_v4();
    let now = Utc::now();

    let mut tx = pool.begin().await.unwrap();
    ObjectMaintenanceRepository::reserve_upload(&mut *tx, &object_key, "artifact")
        .await
        .unwrap();
    ArtifactUploadGrantRepository::create(
        &mut *tx,
        CreateArtifactUploadGrantInput {
            token: grant_token,
            artifact_version: version.id,
            segment_sequence: None,
            object_key: object_key.clone(),
            expected_size: 12,
            expected_sha256: "b".repeat(64),
            content_type: "application/octet-stream".to_string(),
            expires_at: now + Duration::minutes(5),
            settle_until: now + Duration::minutes(10),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    sqlx::query("UPDATE object_maintenance_ledger SET updated = NOW() - INTERVAL '2 hours' WHERE object_key = $1")
        .bind(&object_key)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(version.id)
    .execute(pool)
    .await
    .unwrap();
    let observed_updated =
        sqlx::query_scalar("SELECT updated FROM object_maintenance_ledger WHERE object_key = $1")
            .bind(&object_key)
            .fetch_one(pool)
            .await
            .unwrap();

    assert!(
        ObjectMaintenanceRepository::stale_uploads(pool, now - Duration::hours(1), 10,)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!ObjectMaintenanceRepository::remove_stale_upload(
        pool,
        sqlx::query_scalar("SELECT id FROM object_maintenance_ledger WHERE object_key = $1")
            .bind(&object_key)
            .fetch_one(pool)
            .await
            .unwrap(),
        observed_updated,
    )
    .await
    .unwrap());

    ObjectMaintenanceRepository::record_uploaded(pool, &object_key, "provider-version", 12)
        .await
        .unwrap();
    sqlx::query("UPDATE object_maintenance_ledger SET updated = NOW() - INTERVAL '2 hours' WHERE object_key = $1")
        .bind(&object_key)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        ObjectMaintenanceRepository::schedule_unreferenced(
            pool,
            now - Duration::hours(1),
            now - Duration::hours(2),
        )
        .await
        .unwrap(),
        0
    );

    sqlx::query("UPDATE object_maintenance_ledger SET state = 'deletion_pending', eligible_at = NOW() - INTERVAL '2 hours' WHERE object_key = $1")
        .bind(&object_key)
        .execute(pool)
        .await
        .unwrap();
    assert!(
        ObjectMaintenanceRepository::claim_deletions(pool, now - Duration::hours(1), 10,)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        StorageMaintenanceRepository::abandoned_pending(pool, now - Duration::hours(1), 10,)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!StorageMaintenanceRepository::claim_abandoned_pending(
        pool,
        version.id,
        now - Duration::hours(1),
    )
    .await
    .unwrap());
    assert!(!StorageMaintenanceRepository::mark_deleting_from_pending(
        pool,
        version.id,
        "provider-version",
        12,
        &"b".repeat(64),
    )
    .await
    .unwrap());

    let mut complete_tx = pool.begin().await.unwrap();
    assert!(ArtifactUploadGrantRepository::mark_completed(
        &mut complete_tx,
        ArtifactUploadGrantRepository::find_by_token(pool, grant_token)
            .await
            .unwrap()
            .unwrap()
            .id,
        "provider-version",
    )
    .await
    .unwrap());
    complete_tx.commit().await.unwrap();
    assert_eq!(
        StorageMaintenanceRepository::abandoned_pending(pool, now - Duration::hours(1), 10,)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn grants_expire_after_the_settlement_window() {
    let database = create_test_pool().await.expect("test database");
    let pool = database.pool();
    let version = pending_version(pool).await;
    let now = Utc::now();
    ArtifactUploadGrantRepository::create(
        pool,
        CreateArtifactUploadGrantInput {
            token: Uuid::new_v4(),
            artifact_version: version.id,
            segment_sequence: None,
            object_key: version.object_key.unwrap(),
            expected_size: 0,
            expected_sha256: "0".repeat(64),
            content_type: "application/octet-stream".to_string(),
            expires_at: now - Duration::minutes(2),
            settle_until: now - Duration::minutes(1),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        ArtifactUploadGrantRepository::mark_expired(pool, 10)
            .await
            .unwrap(),
        1
    );
    let expired = ArtifactUploadGrantRepository::find_by_artifact_version(pool, version.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired.state, ArtifactUploadGrantState::Expired);
    assert_eq!(
        ArtifactUploadGrantRepository::purge_terminal(pool, now + Duration::seconds(1), 10)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn settled_grant_can_be_renewed_without_replacing_its_identity() {
    let database = create_test_pool().await.expect("test database");
    let pool = database.pool();
    let version = pending_version(pool).await;
    let grant_token = Uuid::new_v4();
    let now = Utc::now();
    ArtifactUploadGrantRepository::create(
        pool,
        CreateArtifactUploadGrantInput {
            token: grant_token,
            artifact_version: version.id,
            segment_sequence: None,
            object_key: version.object_key.unwrap(),
            expected_size: 12,
            expected_sha256: "c".repeat(64),
            content_type: "application/octet-stream".to_string(),
            expires_at: now - Duration::minutes(2),
            settle_until: now - Duration::minutes(1),
        },
    )
    .await
    .unwrap();

    let expires_at = now + Duration::minutes(10);
    let settle_until = expires_at + Duration::minutes(5);
    let mut tx = pool.begin().await.unwrap();
    assert!(ArtifactUploadGrantRepository::renew(
        &mut tx,
        ArtifactUploadGrantRepository::find_by_token(pool, grant_token)
            .await
            .unwrap()
            .unwrap()
            .id,
        expires_at,
        settle_until,
    )
    .await
    .unwrap());
    tx.commit().await.unwrap();

    let renewed = ArtifactUploadGrantRepository::find_by_token(pool, grant_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renewed.state, ArtifactUploadGrantState::Issued);
    assert_eq!(
        renewed.expires_at.timestamp_micros(),
        expires_at.timestamp_micros()
    );
    assert_eq!(
        renewed.settle_until.timestamp_micros(),
        settle_until.timestamp_micros()
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn expired_url_cannot_be_renewed_during_its_settlement_window() {
    let database = create_test_pool().await.expect("test database");
    let pool = database.pool();
    let version = pending_version(pool).await;
    let grant_token = Uuid::new_v4();
    let now = Utc::now();
    ArtifactUploadGrantRepository::create(
        pool,
        CreateArtifactUploadGrantInput {
            token: grant_token,
            artifact_version: version.id,
            segment_sequence: None,
            object_key: version.object_key.unwrap(),
            expected_size: 12,
            expected_sha256: "d".repeat(64),
            content_type: "application/octet-stream".to_string(),
            expires_at: now - Duration::minutes(1),
            settle_until: now + Duration::minutes(4),
        },
    )
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    assert!(!ArtifactUploadGrantRepository::renew(
        &mut tx,
        ArtifactUploadGrantRepository::find_by_token(pool, grant_token)
            .await
            .unwrap()
            .unwrap()
            .id,
        now + Duration::minutes(10),
        now + Duration::minutes(15),
    )
    .await
    .unwrap());
    tx.rollback().await.unwrap();
}
