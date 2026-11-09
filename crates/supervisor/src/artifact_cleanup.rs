use anyhow::Result;
use attune_common::{
    artifact_transport::ArtifactFileTransport,
    config::SupervisorMaintenanceConfig,
    repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository,
        maintenance::{ArtifactCleanupResult, MaintenanceRepository},
        maintenance_admission::{MaintenanceBudget, PhysicalCleanup},
        storage_maintenance::StorageMaintenanceRepository,
    },
};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use tokio::time::Instant;

pub async fn cleanup_expired_artifacts(
    pool: &PgPool,
    artifact_transport: &dyn ArtifactFileTransport,
    maintenance: &SupervisorMaintenanceConfig,
) -> Result<ArtifactCleanupResult> {
    cleanup_expired_artifacts_with_budget(
        pool,
        artifact_transport,
        maintenance,
        &MaintenanceBudget::default(),
    )
    .await
}

/// Do not cancel this borrowed transport operation during physical deletion.
/// Production calls it from an owned housekeeping task. Metadata/admission are
/// bounded, filesystem work has no safe cancellation protocol and is awaited.
pub async fn cleanup_expired_artifacts_with_budget(
    pool: &PgPool,
    artifact_transport: &dyn ArtifactFileTransport,
    maintenance: &SupervisorMaintenanceConfig,
    budget: &MaintenanceBudget,
) -> Result<ArtifactCleanupResult> {
    let page = MaintenanceRepository::expired_artifact_candidates_bounded(
        pool,
        maintenance.artifact_cleanup_batch_size,
        budget,
    )
    .await?;

    let mut result = ArtifactCleanupResult {
        candidates: page.versions.len() as i64,
        scan_pending: page.pending,
        deleted_versions: 0,
        deleted_files: 0,
        deleted_artifacts: 0,
    };

    for version in page.versions {
        let id = version.id;
        let mut cleanup = PhysicalCleanup::begin(pool, budget).await?;
        let current = cleanup
            .metadata(move |conn| {
                Box::pin(async move {
                    if FileEvidenceRetentionRepository::request_cleanup(conn, &[id]).await? != [id]
                    {
                        return Ok(None);
                    }
                    MaintenanceRepository::claim_expired_artifact_version(conn, id).await
                })
            })
            .await;
        let Some(current) = (match current {
            Ok(current) => current,
            Err(error) => {
                cleanup.rollback().await?;
                return Err(error.into());
            }
        }) else {
            cleanup.rollback().await?;
            anyhow::bail!("Expired artifact eligibility changed; retry required");
        };
        // The current path and retention policy were rechecked under the gate.
        if let Some(file_path) = current.file_path.as_deref() {
            if let Err(error) = artifact_transport.delete_file(file_path).await {
                cleanup.rollback().await?;
                return Err(error.into());
            }
            result.deleted_files += 1;
        }

        let artifact = current.artifact;
        let completed = cleanup
            .metadata(move |conn| {
                Box::pin(async move {
                    MaintenanceRepository::delete_artifact_version_and_refresh_with_conn(
                        conn, id, artifact,
                    )
                    .await
                })
            })
            .await;
        let (removed, parent_deleted) = match completed {
            Ok(value) => value,
            Err(error) => {
                cleanup.rollback().await?;
                return Err(error.into());
            }
        };
        if removed {
            cleanup.commit().await?;
            result.deleted_versions += 1;
            if parent_deleted {
                result.deleted_artifacts += 1;
            }
        } else {
            cleanup.rollback().await?;
            anyhow::bail!("Expired artifact metadata deletion deferred; retry required");
        }
    }

    // Metadata deletion must not leave a readable orphan. The guard survives
    // failure; release it only after the native cleaner verifies removal.
    for (path, version) in MaintenanceRepository::orphaned_staging_bounded(
        pool,
        maintenance.artifact_cleanup_batch_size,
        budget,
    )
    .await?
    {
        artifact_transport.delete_file(&path).await?;
        if artifact_transport.file_exists(&path).await? {
            anyhow::bail!("Orphaned staging removal incomplete; retry required");
        }
        MaintenanceRepository::release_orphaned_staging_bounded(pool, path, version, budget)
            .await
            .into_result()?;
    }

    Ok(result)
}

pub async fn cleanup_abandoned_shared_logs(
    pool: &PgPool,
    artifact_transport: &dyn ArtifactFileTransport,
    maintenance: &SupervisorMaintenanceConfig,
) -> Result<i64> {
    cleanup_abandoned_shared_logs_with_budget(
        pool,
        artifact_transport,
        maintenance,
        &MaintenanceBudget::default(),
    )
    .await
}

pub async fn cleanup_abandoned_shared_logs_with_budget(
    pool: &PgPool,
    artifact_transport: &dyn ArtifactFileTransport,
    maintenance: &SupervisorMaintenanceConfig,
    budget: &MaintenanceBudget,
) -> Result<i64> {
    let cutoff = Utc::now()
        - Duration::seconds(
            maintenance
                .object_upload_abandon_seconds
                .min(i64::MAX as u64) as i64,
        );
    let mut deleted = 0;
    let mut deferred = None;

    let cancel = Default::default();
    for candidate in StorageMaintenanceRepository::abandoned_shared_log_pending_bounded(
        pool,
        cutoff,
        maintenance.artifact_cleanup_batch_size,
        budget,
    )
    .await?
    {
        // A held file lock or failed claim is local to this candidate. Preserve
        // the failure for scheduling, but still visit independent shared logs.
        let completed: Result<()> = async {
            let candidate =
                StorageMaintenanceRepository::claim_abandoned_shared_log_pending_bounded(
                    pool,
                    candidate,
                    cutoff,
                    budget,
                    Instant::now() + budget.page_timeout,
                    &cancel,
                )
                .await
                .into_result()?
                .flatten();
            let Some(candidate) = candidate else {
                anyhow::bail!("Shared log cleanup claim changed or deferred; retry required");
            };
            if !artifact_transport
                .delete_abandoned_log_file(&candidate.file_path)
                .await?
            {
                anyhow::bail!("Shared log physical cleanup deferred by an active file writer");
            }
            if StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
                pool,
                candidate.id,
                budget,
                Instant::now() + budget.page_timeout,
                &cancel,
            )
            .await
            .into_result()?
                == Some(true)
            {
                Ok(())
            } else {
                anyhow::bail!("Shared log metadata completion changed or deferred; retry required");
            }
        }
        .await;
        match completed {
            Ok(()) => deleted += 1,
            Err(error) => {
                if deferred.is_none() {
                    deferred = Some(error);
                }
            }
        }
    }

    if let Some(error) = deferred {
        return Err(error);
    }
    Ok(deleted)
}
