use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use attune_common::{
    blob_store::{BlobStore, ObjectKey, ProviderVersion},
    config::SupervisorMaintenanceConfig,
    repositories::{
        maintenance::MaintenanceRepository, object_maintenance::ObjectMaintenanceRepository,
        pack_retention::PackRetentionRepository, storage_maintenance::StorageMaintenanceRepository,
    },
};
use chrono::{Duration, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectRetentionMetrics {
    pub retained_releases: i64,
    pub deleted_releases: u64,
    pub pending_collection: i64,
    pub deleted_objects: u64,
    pub deleted_bytes: u64,
    pub failures: u64,
}

pub async fn run_cycle(
    pool: &PgPool,
    blob_store: &Arc<dyn BlobStore>,
    packs_base_dir: &Path,
    config: &SupervisorMaintenanceConfig,
) -> Result<ObjectRetentionMetrics> {
    let now = Utc::now();
    let upload_cutoff = now - seconds(config.object_upload_abandon_seconds);
    let delete_cutoff = now - seconds(config.object_delete_grace_seconds);
    let mut metrics = ObjectRetentionMetrics::default();

    if config.pack_release_retention_enabled {
        let result = PackRetentionRepository::collect(
            pool,
            packs_base_dir,
            now - seconds(config.pack_release_rollback_seconds),
            config.pack_release_newest_inactive,
            config.pack_release_cleanup_batch_size,
        )
        .await?;
        metrics.retained_releases = result.retained;
        metrics.deleted_releases = result.deleted;
    }

    for pending in StorageMaintenanceRepository::abandoned_pending(
        pool,
        upload_cutoff,
        config.artifact_cleanup_batch_size,
    )
    .await?
    {
        if !StorageMaintenanceRepository::claim_abandoned_pending(pool, pending.id, upload_cutoff)
            .await?
        {
            continue;
        }
        if StorageMaintenanceRepository::delete_cleanup_claimed(pool, pending.id).await? {
            MaintenanceRepository::refresh_or_delete_artifact_metadata(pool, pending.artifact)
                .await?;
        }
    }

    for entry in ObjectMaintenanceRepository::stale_uploads(
        pool,
        upload_cutoff,
        config.artifact_cleanup_batch_size,
    )
    .await?
    {
        let key = ObjectKey::new(entry.object_key)?;
        match blob_store.head(&key).await {
            Ok(Some(object)) => {
                ObjectMaintenanceRepository::record_uploaded(
                    pool,
                    key.as_str(),
                    object.provider_version.as_stored(),
                    object.size as i64,
                )
                .await?;
            }
            Ok(None) => {
                ObjectMaintenanceRepository::remove_stale_upload(pool, entry.id, entry.updated)
                    .await?;
            }
            Err(error) => {
                metrics.failures += 1;
                ObjectMaintenanceRepository::record_failure(pool, entry.id, &error.to_string())
                    .await?;
            }
        }
    }

    ObjectMaintenanceRepository::schedule_unreferenced(pool, upload_cutoff, now).await?;
    let entries = ObjectMaintenanceRepository::claim_deletions(
        pool,
        delete_cutoff,
        config.artifact_cleanup_batch_size,
    )
    .await?;
    for entry in entries {
        let Some(version) = entry.provider_version.as_deref() else {
            metrics.failures += 1;
            ObjectMaintenanceRepository::record_failure(
                pool,
                entry.id,
                "deletion entry has no provider version",
            )
            .await?;
            continue;
        };
        if ObjectMaintenanceRepository::exact_reference_exists(pool, &entry.object_key, version)
            .await?
        {
            ObjectMaintenanceRepository::defer_referenced(pool, entry.id).await?;
            continue;
        }
        let key = ObjectKey::new(entry.object_key.clone())?;
        let version = ProviderVersion::from_stored(version.to_string())?;
        match blob_store.delete(&key, &version).await {
            Ok(()) => {
                if ObjectMaintenanceRepository::complete_delete(pool, entry.id).await? {
                    metrics.deleted_objects += 1;
                    metrics.deleted_bytes += entry.size_bytes.unwrap_or_default().max(0) as u64;
                }
            }
            Err(error) => {
                metrics.failures += 1;
                ObjectMaintenanceRepository::record_failure(pool, entry.id, &error.to_string())
                    .await?;
            }
        }
    }
    metrics.pending_collection = ObjectMaintenanceRepository::pending_count(pool).await?;
    Ok(metrics)
}

fn seconds(value: u64) -> Duration {
    Duration::seconds(value.min(i64::MAX as u64) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        blob_store::{body_from_bytes, sha256, FilesystemBlobStore},
        config::Config,
        models::enums::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, ExecutionStatus,
            LogStreamBackend, OwnerType, RetentionPolicyType,
        },
        repositories::{
            artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
            execution::{CreateExecutionInput, ExecutionRepository},
            log_stream::LogStreamRepository,
            Create,
        },
        test_database::TestDatabase,
    };
    use bytes::Bytes;

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn interrupted_exact_delete_completes_idempotently_on_retry() {
        let path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = Config::load_from_file(&path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> =
            Arc::new(FilesystemBlobStore::new(directory.path()).unwrap());
        let key = ObjectKey::new("interrupted/delete").unwrap();
        let body = Bytes::from_static(b"delete me");
        ObjectMaintenanceRepository::reserve_upload(&database, key.as_str(), "artifact")
            .await
            .unwrap();
        let stored = store
            .put(&key, body_from_bytes(body.clone()), sha256(&body))
            .await
            .unwrap();
        ObjectMaintenanceRepository::record_uploaded(
            &database,
            key.as_str(),
            stored.provider_version.as_stored(),
            stored.size as i64,
        )
        .await
        .unwrap();
        ObjectMaintenanceRepository::schedule_unreferenced(
            &database,
            Utc::now() + Duration::hours(1),
            Utc::now(),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE object_maintenance_ledger SET state = 'deleting', attempts = 1, updated = NOW() - INTERVAL '2 hours'",
        )
        .execute(&*database)
        .await
        .unwrap();

        // Simulate a crash after provider deletion but before ledger completion.
        store.delete(&key, &stored.provider_version).await.unwrap();
        let mut maintenance = config.maintenance;
        maintenance.object_delete_grace_seconds = 1;
        maintenance.pack_release_retention_enabled = false;
        let metrics = run_cycle(&database, &store, directory.path(), &maintenance)
            .await
            .unwrap();

        assert_eq!(metrics.deleted_objects, 1);
        assert_eq!(metrics.deleted_bytes, body.len() as u64);
        assert_eq!(metrics.failures, 0);
        assert_eq!(metrics.pending_collection, 0);
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn cycle_preserves_active_object_logs_and_removes_abandoned_pending_versions() {
        let path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = Config::load_from_file(&path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> =
            Arc::new(FilesystemBlobStore::new(directory.path()).unwrap());
        let artifact = ArtifactRepository::create(
            &database,
            CreateArtifactInput {
                r#ref: "supervisor_cleanup_policy".to_string(),
                scope: OwnerType::System,
                owner: "supervisor-test".to_string(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 10,
                name: None,
                description: None,
                content_type: None,
                data: None,
            },
        )
        .await
        .unwrap();
        let create_execution = |status| CreateExecutionInput {
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
            status,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        };
        let running =
            ExecutionRepository::create(&database, create_execution(ExecutionStatus::Running))
                .await
                .unwrap();
        let terminal =
            ExecutionRepository::create(&database, create_execution(ExecutionStatus::Failed))
                .await
                .unwrap();
        let ordinary = ArtifactVersionRepository::create_object_pending(
            &database,
            artifact.id,
            None,
            "application/octet-stream".to_string(),
            None,
            None,
        )
        .await
        .unwrap();
        let active_log = ArtifactVersionRepository::create_log_pending(
            &database,
            artifact.id,
            &artifact.r#ref,
            LogStreamBackend::ObjectSegments,
            "text/plain".to_string(),
            Some(running.id),
            None,
            None,
        )
        .await
        .unwrap();
        let terminal_log = ArtifactVersionRepository::create_log_pending(
            &database,
            artifact.id,
            &artifact.r#ref,
            LogStreamBackend::ObjectSegments,
            "text/plain".to_string(),
            Some(terminal.id),
            None,
            None,
        )
        .await
        .unwrap();
        for version in [&active_log, &terminal_log] {
            LogStreamRepository::create(&database, version.id, 1024, 500)
                .await
                .unwrap();
        }
        sqlx::query(
            "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' \
             WHERE id = ANY($1)",
        )
        .bind(vec![ordinary.id, active_log.id, terminal_log.id])
        .execute(&*database)
        .await
        .unwrap();

        let mut maintenance = config.maintenance;
        maintenance.object_upload_abandon_seconds = 1;
        maintenance.pack_release_retention_enabled = false;
        run_cycle(&database, &store, directory.path(), &maintenance)
            .await
            .unwrap();

        assert!(
            ArtifactVersionRepository::find_by_id(&database, ordinary.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            ArtifactVersionRepository::find_by_id(&database, terminal_log.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            ArtifactVersionRepository::find_by_id(&database, active_log.id)
                .await
                .unwrap()
                .is_some()
        );
    }
}
