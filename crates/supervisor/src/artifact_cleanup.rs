use anyhow::Result;
use attune_common::{
    artifact_transport::ArtifactFileTransport,
    config::SupervisorMaintenanceConfig,
    repositories::{
        maintenance::{ArtifactCleanupResult, MaintenanceRepository},
        storage_maintenance::StorageMaintenanceRepository,
    },
};
use chrono::{Duration, Utc};
use sqlx::PgPool;

pub async fn cleanup_expired_artifacts(
    pool: &PgPool,
    artifact_transport: &dyn ArtifactFileTransport,
    maintenance: &SupervisorMaintenanceConfig,
) -> Result<ArtifactCleanupResult> {
    let candidates = MaintenanceRepository::expired_artifact_version_count(pool).await?;
    let versions = MaintenanceRepository::find_expired_artifact_versions(
        pool,
        maintenance.artifact_cleanup_batch_size,
    )
    .await?;

    let mut result = ArtifactCleanupResult {
        candidates,
        deleted_versions: 0,
        deleted_files: 0,
        deleted_artifacts: 0,
    };

    for version in versions {
        if let Some(file_path) = version.file_path.as_deref() {
            artifact_transport.delete_file(file_path).await?;
            result.deleted_files += 1;
        }

        if MaintenanceRepository::delete_artifact_version(pool, version.id).await? {
            result.deleted_versions += 1;
            if MaintenanceRepository::refresh_or_delete_artifact_metadata(pool, version.artifact)
                .await?
            {
                result.deleted_artifacts += 1;
            }
        }
    }

    Ok(result)
}

pub async fn cleanup_abandoned_shared_logs(
    pool: &PgPool,
    artifact_transport: &dyn ArtifactFileTransport,
    maintenance: &SupervisorMaintenanceConfig,
) -> Result<i64> {
    let cutoff = Utc::now()
        - Duration::seconds(
            maintenance
                .object_upload_abandon_seconds
                .min(i64::MAX as u64) as i64,
        );
    let mut deleted = 0;

    for candidate in StorageMaintenanceRepository::abandoned_shared_log_pending(
        pool,
        cutoff,
        maintenance.artifact_cleanup_batch_size,
    )
    .await?
    {
        if !StorageMaintenanceRepository::claim_abandoned_shared_log_pending(
            pool,
            candidate.id,
            cutoff,
        )
        .await?
        {
            continue;
        }
        if !artifact_transport
            .delete_abandoned_log_file(&candidate.file_path)
            .await?
        {
            continue;
        }
        if StorageMaintenanceRepository::delete_cleanup_claimed(pool, candidate.id).await? {
            MaintenanceRepository::refresh_or_delete_artifact_metadata(pool, candidate.artifact)
                .await?;
            deleted += 1;
        }
    }

    Ok(deleted)
}
