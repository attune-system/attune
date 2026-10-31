use super::setup_db;
use attune_common::{
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        maintenance::MaintenanceRepository,
        maintenance_admission::{MaintenanceBudget, MaintenanceOutcome},
        Create, FindById,
    },
};
use std::time::Duration;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

async fn artifact(pool: &sqlx::PgPool) -> i64 {
    ArtifactRepository::create(
        pool,
        CreateArtifactInput {
            r#ref: format!("bounded.metadata_{}", uuid::Uuid::new_v4().simple()),
            scope: OwnerType::System,
            owner: "maintenance-metadata-test".into(),
            r#type: ArtifactType::FileBinary,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn bounded_empty_artifact_deletion_defers_and_retries_without_a_queued_writer() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let mut capture = db.pool().begin().await.unwrap();
    Evidence::lock_capture(&mut capture).await.unwrap();
    let deferred = timeout(
        Duration::from_secs(2),
        MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
            db.pool(),
            id,
            &budget,
            deadline(),
            &cancel,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        deferred.outcome,
        MaintenanceOutcome::LockDeferred,
        "{deferred:?}"
    );
    assert!(deferred.value.is_none());
    // Keep the original capture open. An abandoned queued writer would block
    // this second reader and recreate the foreground regression.
    let mut reader = db.pool().begin().await.unwrap();
    timeout(Duration::from_secs(2), Evidence::lock_capture(&mut reader))
        .await
        .unwrap()
        .unwrap();
    reader.rollback().await.unwrap();
    capture.rollback().await.unwrap();
    let deleted = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        id,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(deleted.outcome, MaintenanceOutcome::Progress, "{deleted:?}");
    assert_eq!(deleted.value, Some(true));
    let empty = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        id,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(empty.outcome, MaintenanceOutcome::NoWork, "{empty:?}");
    assert_eq!(empty.candidates, 0);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn bounded_metadata_refresh_keeps_latest_ready_version_and_pinned_empty_artifacts() {
    let db = setup_db().await;
    let id = artifact(db.pool()).await;
    ArtifactVersionRepository::create(
        db.pool(),
        CreateArtifactVersionInput {
            artifact: id,
            file_path: None,
            content: Some(vec![0; 19]),
            content_json: None,
            content_type: Some("application/owned-fixture".into()),
            execution: None,
            meta: None,
            created_by: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE artifact SET size_bytes=0 WHERE id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let budget = MaintenanceBudget::default();
    let cancel = CancellationToken::new();
    let refreshed = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        id,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(
        refreshed.outcome,
        MaintenanceOutcome::Progress,
        "{refreshed:?}"
    );
    assert_eq!(
        refreshed.value,
        Some(false),
        "false means refreshed, not deleted"
    );
    let size: Option<i64> = sqlx::query_scalar("SELECT size_bytes FROM artifact WHERE id=$1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(size, Some(19));

    let empty = artifact(db.pool()).await;
    let mut seed = db.pool().begin().await.unwrap();
    Evidence::lock_reclamation(&mut seed).await.unwrap();
    sqlx::query(
        "INSERT INTO file_evidence_owner(version,artifact,root_read) VALUES(99401,99402,99403)",
    )
    .execute(&mut *seed)
    .await
    .unwrap();
    sqlx::query("INSERT INTO file_evidence_pin(owner,kind,source) VALUES(99401,'artifact',$1)")
        .bind(empty)
        .execute(&mut *seed)
        .await
        .unwrap();
    seed.commit().await.unwrap();
    let pinned = MaintenanceRepository::refresh_or_delete_artifact_metadata_bounded(
        db.pool(),
        empty,
        &budget,
        deadline(),
        &cancel,
    )
    .await;
    assert_eq!(pinned.outcome, MaintenanceOutcome::NoWork, "{pinned:?}");
    assert_eq!(pinned.value, Some(false));
    assert!(ArtifactRepository::find_by_id(db.pool(), empty)
        .await
        .unwrap()
        .is_some());
    db.cleanup().await.unwrap();
}
