use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use attune_common::{
    blob_store::{BlobStore, ObjectKey, ProviderVersion},
    config::SupervisorMaintenanceConfig,
    repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        maintenance_admission::{MaintenanceBudget, MaintenanceOutcome, MaintenanceResult},
        object_maintenance::ObjectMaintenanceRepository,
        pack_retention::{
            GraphReclamationOptions, GraphReclamationOutcome, PackRetentionRepository,
        },
        storage_maintenance::StorageMaintenanceRepository,
    },
};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectRetentionMetrics {
    pub retained_releases: i64,
    pub deleted_releases: u64,
    pub pending_collection: i64,
    pub deleted_objects: u64,
    pub deleted_bytes: u64,
    pub failures: u64,
    pub graph_candidates: u64,
    pub graph_rows_changed: u64,
    pub graph_lock_deferrals: u64,
    pub graph_deadline_deferrals: u64,
    pub graph_collection_complete: bool,
    pub graph_scan_pending: bool,
    pub next_graph_attempt: Option<chrono::DateTime<Utc>>,
    pub metadata_complete: bool,
    pub metadata_scan_pending: bool,
    pub metadata_lock_deferrals: u64,
    pub metadata_deadline_deferrals: u64,
    pub metadata_rows_changed: u64,
    pub oldest_metadata_deferral_seconds: Option<i64>,
    pub oldest_graph_deferral_seconds: Option<i64>,
}

#[cfg(test)]
pub(super) async fn run_cycle(
    pool: &PgPool,
    blob_store: &Arc<dyn BlobStore>,
    packs_base_dir: &Path,
    config: &SupervisorMaintenanceConfig,
) -> Result<ObjectRetentionMetrics> {
    run_cycle_with_budget(
        pool,
        blob_store,
        packs_base_dir,
        config,
        &MaintenanceBudget::default(),
    )
    .await
}

pub async fn run_cycle_with_budget(
    pool: &PgPool,
    blob_store: &Arc<dyn BlobStore>,
    packs_base_dir: &Path,
    config: &SupervisorMaintenanceConfig,
    budget: &MaintenanceBudget,
) -> Result<ObjectRetentionMetrics> {
    // Each protected metadata unit gets a fresh work window. Provider latency
    // and a preceding graph sweep must not spend a later acknowledgement's
    // allowance. Candidate counts bound the number of units in this cycle.
    let metadata_deadline = || Instant::now() + budget.page_timeout;
    let limit = config.artifact_cleanup_batch_size.clamp(1, 256);
    let cancellation = Default::default();
    let now = Utc::now();
    let upload_cutoff = now - seconds(config.object_upload_abandon_seconds);
    let delete_cutoff = now - seconds(config.object_delete_grace_seconds);
    let mut metrics = ObjectRetentionMetrics {
        graph_collection_complete: true,
        metadata_complete: true,
        ..Default::default()
    };
    let _ = metadata_value(
        &mut metrics, "object_log_close_recovery",
        attune_common::repositories::log_stream::LogStreamRepository::recover_expired_closes_bounded(
            pool, limit, budget, metadata_deadline(), &cancellation).await,
    );
    let _ = metadata_value(
        &mut metrics,
        "file_evidence_audit",
        Evidence::audit_pending_bounded(
            pool,
            limit,
            budget,
            // Owner work remains page-bounded. Reserve separate bounded drain,
            // failed-visit bookkeeping, and bookkeeping rollback allowances.
            Instant::now() + budget.page_timeout + budget.statement_timeout * 3,
            &cancellation,
        )
        .await,
    );

    if config.pack_release_retention_enabled {
        let options = GraphReclamationOptions {
            delete_limit: config.pack_release_cleanup_batch_size.clamp(1, 256),
            ..Default::default()
        };
        let result = PackRetentionRepository::collect_bounded(
            pool,
            now - seconds(config.pack_release_rollback_seconds),
            config.pack_release_newest_inactive,
            &options,
        )
        .await?;
        metrics.graph_collection_complete = result.complete();
        for phase in &result.phases {
            metrics.graph_candidates += phase.candidates as u64;
            metrics.graph_rows_changed += phase.changed;
            match phase.outcome {
                GraphReclamationOutcome::LockDeferred => metrics.graph_lock_deferrals += 1,
                GraphReclamationOutcome::DeadlineDeferred => metrics.graph_deadline_deferrals += 1,
                GraphReclamationOutcome::Failed | GraphReclamationOutcome::CommitUnknown => {
                    metrics.failures += 1;
                    // Database diagnostics can contain row data. Keep routine
                    // maintenance logs limited to the fixed phase and outcome.
                    tracing::warn!(
                        phase = phase.phase.name(),
                        outcome = ?phase.outcome,
                        "Graph reclamation phase did not complete"
                    );
                }
                _ => {}
            }
        }
        metrics.deleted_releases = result.deleted_content.len() as u64;
        PackRetentionRepository::cleanup_unreferenced(
            pool,
            packs_base_dir,
            &result.deleted_content,
        )
        .await?;
        // Reporting and physical cleanup happen after every graph transaction.
        metrics.retained_releases = PackRetentionRepository::retained_count(pool).await?;
        let progress = PackRetentionRepository::reclamation_progress(pool).await?;
        metrics.graph_scan_pending = progress.iter().any(|phase| phase.pending);
        metrics.next_graph_attempt =
            PackRetentionRepository::next_reclamation_attempt(pool).await?;
        metrics.oldest_graph_deferral_seconds = progress
            .iter()
            .filter_map(|phase| phase.deferred_since)
            .map(|since| (Utc::now() - since).num_seconds().max(0))
            .max();
        // A later cheap page must not hide an earlier oversized/deferred visit.
        metrics.graph_collection_complete &= metrics.oldest_graph_deferral_seconds.is_none();
    }

    for pending in
        StorageMaintenanceRepository::abandoned_pending(pool, upload_cutoff, limit).await?
    {
        if metadata_value(
            &mut metrics,
            "abandoned_pending_claim",
            StorageMaintenanceRepository::claim_abandoned_pending_bounded(
                pool,
                pending.id,
                upload_cutoff,
                budget,
                metadata_deadline(),
                &cancellation,
            )
            .await,
        ) != Some(true)
        {
            continue;
        }
        let _ = metadata_value(
            &mut metrics,
            "abandoned_pending_completion",
            StorageMaintenanceRepository::delete_cleanup_claimed_and_refresh_bounded(
                pool,
                pending.id,
                budget,
                metadata_deadline(),
                &cancellation,
            )
            .await,
        );
    }

    for entry in ObjectMaintenanceRepository::stale_uploads(pool, upload_cutoff, limit).await? {
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
                let _ = metadata_value(
                    &mut metrics,
                    "stale_upload_completion",
                    ObjectMaintenanceRepository::remove_stale_upload_bounded(
                        pool,
                        entry.id,
                        entry.updated,
                        budget,
                        metadata_deadline(),
                        &cancellation,
                    )
                    .await,
                );
            }
            Err(error) => {
                metrics.failures += 1;
                ObjectMaintenanceRepository::record_failure(pool, entry.id, &error.to_string())
                    .await?;
            }
        }
    }

    // Metadata cleanup is only eligibility. Native conditional-create fencing
    // below proves old PUTs cannot recreate payload bytes, including after an
    // API restart or a late remote response. Failure leaves every pin intact.
    let plans = metadata_value(
        &mut metrics,
        "file_put_recovery",
        Evidence::recovery_candidates_bounded(
            pool,
            limit,
            budget,
            metadata_deadline(),
            &cancellation,
        )
        .await,
    )
    .unwrap_or_default();
    for plan in plans {
        let key = plan.key()?;
        match attune_common::blob_store::settle_atomic_file_put(
            blob_store.as_ref(),
            &key,
            plan.token(),
        )
        .await
        {
            Ok(settled) => {
                let _ = metadata_value(
                    &mut metrics,
                    "file_put_recovery_completion",
                    Evidence::complete_put_recovery_bounded(
                        pool,
                        &plan,
                        &settled,
                        budget,
                        metadata_deadline(),
                        &cancellation,
                    )
                    .await,
                );
            }
            Err(_) => {
                metrics.failures += 1;
            }
        }
    }

    ObjectMaintenanceRepository::schedule_unreferenced(pool, upload_cutoff, now).await?;
    let entries = metadata_value(
        &mut metrics,
        "object_deletion_claims",
        ObjectMaintenanceRepository::claim_deletions_bounded(
            pool,
            delete_cutoff,
            limit,
            budget,
            // The claim itself remains page-bounded. After acknowledged
            // rollback, leave a separate bounded reserve for retry progress.
            Instant::now() + budget.page_timeout + budget.statement_timeout * 3,
            &cancellation,
        )
        .await,
    )
    .unwrap_or_default();
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
                if metadata_value(
                    &mut metrics,
                    "object_deletion_completion",
                    ObjectMaintenanceRepository::complete_delete_bounded(
                        pool,
                        entry.id,
                        budget,
                        metadata_deadline(),
                        &cancellation,
                    )
                    .await,
                ) == Some(true)
                {
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
    metrics.metadata_scan_pending = ObjectMaintenanceRepository::deletion_scan_pending(pool)
        .await?
        || Evidence::audit_scan_pending(pool).await?
        || StorageMaintenanceRepository::abandoned_discovery_pending(pool).await?;
    metrics.oldest_metadata_deferral_seconds = [
        Evidence::audit_deferred_since(pool).await?,
        ObjectMaintenanceRepository::deletion_deferred_since(pool).await?,
    ]
    .into_iter()
    .flatten()
    .min()
    .map(|since| (Utc::now() - since).num_seconds().max(0));
    metrics.metadata_complete &= !metrics.metadata_scan_pending;
    Ok(metrics)
}

fn metadata_value<T>(
    metrics: &mut ObjectRetentionMetrics,
    phase: &'static str,
    result: MaintenanceResult<T>,
) -> Option<T> {
    metrics.metadata_rows_changed += result.changed;
    match result.outcome {
        MaintenanceOutcome::Progress | MaintenanceOutcome::NoWork => result.value,
        outcome => {
            metrics.metadata_complete = false;
            match outcome {
                MaintenanceOutcome::LockDeferred => metrics.metadata_lock_deferrals += 1,
                MaintenanceOutcome::DeadlineDeferred => metrics.metadata_deadline_deferrals += 1,
                _ => metrics.failures += 1,
            }
            // Counts and fixed phase/outcome labels only: database diagnostics
            // may contain source values and must not enter routine logs.
            tracing::warn!(
                phase,
                ?outcome,
                candidates = result.candidates,
                "Metadata maintenance deferred or failed"
            );
            None
        }
    }
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

    struct SlowSuccessfulStore {
        inner: Arc<FilesystemBlobStore>,
        delay: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl BlobStore for SlowSuccessfulStore {
        async fn preflight(
            &self,
        ) -> std::result::Result<(), attune_common::blob_store::BlobStoreError> {
            self.inner.preflight().await
        }
        async fn put(
            &self,
            key: &ObjectKey,
            body: attune_common::blob_store::BlobBody,
            digest: [u8; 32],
        ) -> std::result::Result<
            attune_common::blob_store::StoredObject,
            attune_common::blob_store::BlobStoreError,
        > {
            self.inner.put(key, body, digest).await
        }
        async fn get(
            &self,
            key: &ObjectKey,
            version: &ProviderVersion,
            range: Option<attune_common::blob_store::ByteRange>,
        ) -> std::result::Result<
            attune_common::blob_store::BlobReader,
            attune_common::blob_store::BlobStoreError,
        > {
            self.inner.get(key, version, range).await
        }
        async fn get_pinned(
            &self,
            key: &ObjectKey,
            version: &ProviderVersion,
            size: u64,
            digest: [u8; 32],
            range: Option<attune_common::blob_store::ByteRange>,
        ) -> std::result::Result<
            attune_common::blob_store::BlobReader,
            attune_common::blob_store::BlobStoreError,
        > {
            self.inner
                .get_pinned(key, version, size, digest, range)
                .await
        }
        async fn head(
            &self,
            key: &ObjectKey,
        ) -> std::result::Result<
            Option<attune_common::blob_store::StoredObject>,
            attune_common::blob_store::BlobStoreError,
        > {
            // Elapsed provider latency is the contract under test, not a barrier
            // for ordering concurrent database operations.
            tokio::time::sleep(self.delay).await;
            self.inner.head(key).await
        }
        async fn delete(
            &self,
            key: &ObjectKey,
            version: &ProviderVersion,
        ) -> std::result::Result<(), attune_common::blob_store::BlobStoreError> {
            tokio::time::sleep(self.delay).await;
            self.inner.delete(key, version).await
        }
    }

    #[tokio::test]
    async fn page_deadline_audit_records_failed_visit_before_trying_cheap_followers() {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> =
            Arc::new(FilesystemBlobStore::new(directory.path()).unwrap());
        let mut seed = database.pool().begin().await.unwrap();
        Evidence::lock_reclamation(&mut seed).await.unwrap();
        // Incomplete historical reservations are rejected by real readmission;
        // a timeout must keep the slow owner pending, not quarantine it.
        sqlx::query("INSERT INTO file_evidence_owner(version,artifact,root_read,evidence_state) VALUES (99701,99801,99901,'pending_audit'),(99702,99802,99902,'pending_audit')")
            .execute(&mut *seed).await.unwrap();
        seed.commit().await.unwrap();
        sqlx::raw_sql("CREATE FUNCTION owned_audit_page_delay() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(0.4); RETURN NEW; END $$; CREATE TRIGGER owned_audit_page_delay BEFORE UPDATE OF evidence_state ON file_evidence_owner FOR EACH ROW WHEN (NEW.version=99701 AND NEW.evidence_state='quarantined') EXECUTE FUNCTION owned_audit_page_delay()")
            .execute(database.pool()).await.unwrap();
        let mut maintenance = config.maintenance;
        maintenance.pack_release_retention_enabled = false;
        maintenance.artifact_cleanup_batch_size = 2;
        let budget = MaintenanceBudget {
            page_timeout: std::time::Duration::from_millis(250),
            statement_timeout: std::time::Duration::from_secs(2),
            ..Default::default()
        };
        let first = run_cycle_with_budget(
            database.pool(),
            &store,
            directory.path(),
            &maintenance,
            &budget,
        )
        .await
        .unwrap();
        assert!(first.metadata_deadline_deferrals > 0, "{first:?}");
        let second = run_cycle_with_budget(
            database.pool(),
            &store,
            directory.path(),
            &maintenance,
            &budget,
        )
        .await
        .unwrap();
        let states: Vec<(i64, String)> = sqlx::query_as(
            "SELECT version,evidence_state FROM file_evidence_owner ORDER BY version",
        )
        .fetch_all(database.pool())
        .await
        .unwrap();
        assert_eq!(
            states,
            vec![
                (99701, "pending_audit".into()),
                (99702, "quarantined".into())
            ],
            "{second:?}"
        );
        assert!(Evidence::audit_deferred_since(database.pool())
            .await
            .unwrap()
            .is_some());
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn object_claim_page_deadline_keeps_retry_progress_and_visits_cheap_followers() {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().unwrap();
        let inner = Arc::new(FilesystemBlobStore::new(directory.path()).unwrap());
        for path in ["claim-budget/slow", "claim-budget/cheap"] {
            let key = ObjectKey::new(path).unwrap();
            let bytes = Bytes::from_static(b"owned claim-page regression");
            ObjectMaintenanceRepository::reserve_upload(database.pool(), path, "artifact")
                .await
                .unwrap();
            let stored = inner
                .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
                .await
                .unwrap();
            ObjectMaintenanceRepository::record_uploaded(
                database.pool(),
                path,
                stored.provider_version.as_stored(),
                stored.size as i64,
            )
            .await
            .unwrap();
        }
        ObjectMaintenanceRepository::schedule_unreferenced(
            database.pool(),
            Utc::now() + Duration::hours(1),
            Utc::now() - Duration::hours(1),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE graph_reclamation_progress SET probe_limit=1 WHERE phase='object_deletions'",
        )
        .execute(database.pool())
        .await
        .unwrap();
        // Elapsed claim time is the contract: individual owner work reaches its
        // whole-page deadline, leaving only the separate bookkeeping reserve.
        sqlx::raw_sql("CREATE FUNCTION owned_slow_object_claim() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(1); RETURN NEW; END $$; CREATE TRIGGER owned_slow_object_claim BEFORE UPDATE ON object_maintenance_ledger FOR EACH ROW WHEN (OLD.object_key='claim-budget/slow' AND NEW.state='deleting') EXECUTE FUNCTION owned_slow_object_claim()")
            .execute(database.pool()).await.unwrap();
        let mut maintenance = config.maintenance;
        maintenance.pack_release_retention_enabled = false;
        maintenance.artifact_cleanup_batch_size = 2;
        maintenance.object_delete_grace_seconds = 1;
        let budget = MaintenanceBudget {
            page_timeout: std::time::Duration::from_millis(500),
            statement_timeout: std::time::Duration::from_secs(2),
            ..Default::default()
        };
        let store: Arc<dyn BlobStore> = inner.clone();
        let first = run_cycle_with_budget(
            database.pool(),
            &store,
            directory.path(),
            &maintenance,
            &budget,
        )
        .await
        .unwrap();
        assert!(first.metadata_deadline_deferrals > 0, "{first:?}");
        let second = run_cycle_with_budget(
            database.pool(),
            &store,
            directory.path(),
            &maintenance,
            &budget,
        )
        .await
        .unwrap();
        assert_eq!(
            second.deleted_objects, 1,
            "the failed singleton must not hide the cheap follower: {second:?}"
        );
        assert!(inner
            .head(&ObjectKey::new("claim-budget/cheap").unwrap())
            .await
            .unwrap()
            .is_none());
        assert!(inner
            .head(&ObjectKey::new("claim-budget/slow").unwrap())
            .await
            .unwrap()
            .is_some());
        assert!(
            second.oldest_metadata_deferral_seconds.is_some(),
            "a cheap claim cannot clear the failed visit's age: {second:?}"
        );
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn slow_successful_storage_preserves_completion_and_later_phase_progress() {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().unwrap();
        let inner = Arc::new(FilesystemBlobStore::new(directory.path()).unwrap());
        ObjectMaintenanceRepository::reserve_upload(
            database.pool(),
            "slow/stale-upload",
            "artifact",
        )
        .await
        .unwrap();
        let key = ObjectKey::new("slow/delete").unwrap();
        let bytes = Bytes::from_static(b"owned slow deletion");
        ObjectMaintenanceRepository::reserve_upload(database.pool(), key.as_str(), "artifact")
            .await
            .unwrap();
        let stored = inner
            .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
            .await
            .unwrap();
        ObjectMaintenanceRepository::record_uploaded(
            database.pool(),
            key.as_str(),
            stored.provider_version.as_stored(),
            stored.size as i64,
        )
        .await
        .unwrap();
        ObjectMaintenanceRepository::schedule_unreferenced(
            database.pool(),
            Utc::now() + Duration::hours(1),
            Utc::now() - Duration::hours(1),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE object_maintenance_ledger SET updated=clock_timestamp()-interval '2 days'",
        )
        .execute(database.pool())
        .await
        .unwrap();
        let mut maintenance = config.maintenance;
        maintenance.pack_release_retention_enabled = false;
        maintenance.object_upload_abandon_seconds = 1;
        maintenance.object_delete_grace_seconds = 1;
        let store: Arc<dyn BlobStore> = Arc::new(SlowSuccessfulStore {
            inner: inner.clone(),
            delay: std::time::Duration::from_secs(11),
        });
        let metrics = run_cycle(database.pool(), &store, directory.path(), &maintenance)
            .await
            .unwrap();
        assert_eq!(metrics.deleted_objects, 1, "{metrics:?}");
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM object_maintenance_ledger")
            .fetch_one(database.pool())
            .await
            .unwrap();
        assert_eq!(
            remaining, 0,
            "successful provider work must be acknowledged despite slow I/O"
        );
        assert!(inner.head(&key).await.unwrap().is_none());
        assert!(metrics.metadata_complete, "{metrics:?}");
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn graph_deferral_is_reported_and_retried_without_losing_the_candidate() {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let database = TestDatabase::create(&config.database).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> =
            Arc::new(FilesystemBlobStore::new(directory.path()).unwrap());
        let mut maintenance = config.maintenance;
        maintenance.pack_release_retention_enabled = true;
        sqlx::query("INSERT INTO runtime_cache_iteration_epoch(iteration,creation_transaction) VALUES(910001,'owned-supervisor-deferral')")
            .execute(database.pool()).await.unwrap();
        let mut capture = database.pool().begin().await.unwrap();
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(
            &mut capture,
        )
        .await
        .unwrap();
        // Keep the shared holder open for the ENTIRE object cycle, including
        // recovery and deletion claims after pack collection has deferred.
        let metrics = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run_cycle(database.pool(), &store, directory.path(), &maintenance),
        )
        .await;
        capture.rollback().await.unwrap();
        let metrics = metrics
            .expect("object retention queued an unbounded graph writer")
            .unwrap();
        assert!(!metrics.graph_collection_complete);
        assert!(metrics.graph_lock_deferrals > 0);
        assert!(metrics.oldest_graph_deferral_seconds.is_some());
        let retained: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_cache_iteration_epoch WHERE iteration=910001)",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert!(retained);
        sqlx::query("UPDATE graph_reclamation_progress SET next_attempt=clock_timestamp()")
            .execute(database.pool())
            .await
            .unwrap();
        let retry = run_cycle(database.pool(), &store, directory.path(), &maintenance)
            .await
            .unwrap();
        assert!(retry.graph_collection_complete, "{retry:?}");
        assert!(retry.graph_rows_changed > 0);
        assert_eq!(retry.oldest_graph_deferral_seconds, None);
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
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
