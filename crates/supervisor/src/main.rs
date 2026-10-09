//! Attune Supervisor Service
//!
//! Owns platform maintenance loops such as runtime database retention.

mod cache_retention;
mod native_maintenance;
mod object_retention;
mod storage_migration;

use attune_supervisor::artifact_cleanup;

use std::{
    process,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Result;
use attune_common::{
    artifact_transport::{ArtifactFileTransport, VolumeTransport},
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome, AuditRepository},
    blob_store::{from_config as blob_store_from_config, BlobStore, ObjectKey},
    config::{CacheRetentionConfig, Config, RetentionConfig, SupervisorMaintenanceConfig},
    db::Database,
    models::{enums::ExecutionStatus, Execution},
    mq::{
        Connection as MqConnection, ExecutionCompletedPayload, ExecutionRequestedPayload,
        MessageEnvelope, MessageQueueConfig, MessageType, Publisher, PublisherConfig,
    },
    observability,
    repositories::{
        artifact_upload_grant::ArtifactUploadGrantRepository,
        execution::{ExecutionRepository, UpdateExecutionInput},
        log_stream::LogStreamRepository,
        maintenance::{
            AdmissionRemediationResult, ArtifactCleanupResult, ExecutionRescheduleAttempt,
            MaintenanceRepository, QueueRemediationResult, StaleExecutionCandidate,
            WorkflowRemediationResult,
        },
        native_maintenance::schedule::{MaintenanceJob, ScheduleRepository},
        retention::{RetentionRepository, RetentionTargetFailure, RetentionTargetResult},
        storage_maintenance::StorageMaintenanceRepository,
        workflow_cache_iteration::{
            StaleSyntheticCacheIterationCompletion, WorkflowCacheIterationRepository,
        },
        FindById,
    },
    system_alert::{emit_core_alert, SystemAlert},
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::{Parser, Subcommand};
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::{broadcast, Mutex};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy)]
enum SupervisorCycleReason {
    StartupRecovery,
    DirtyShutdownRecovery,
    Scheduled,
}

impl SupervisorCycleReason {
    fn log_label(self) -> &'static str {
        match self {
            Self::StartupRecovery => "startup_recovery",
            Self::DirtyShutdownRecovery => "dirty_shutdown_recovery",
            Self::Scheduled => "scheduled",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "attune-supervisor")]
#[command(about = "Attune Supervisor Service - platform maintenance", long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<String>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long)]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Copy legacy files into object storage and switch metadata after verification.
    MigrateStorage {
        /// Override the configured rollback snapshot period.
        #[arg(long)]
        rollback_snapshot_seconds: Option<u64>,
    },
}

#[derive(Clone)]
struct SupervisorService {
    inner: Arc<SupervisorServiceInner>,
}

struct SupervisorServiceInner {
    pool: PgPool,
    config: Config,
    artifact_transport: Arc<dyn ArtifactFileTransport>,
    blob_store: Arc<dyn BlobStore>,
    publisher: Option<Arc<Publisher>>,
    _mq_connection: Option<MqConnection>,
    run_id: Mutex<Option<String>>,
    cache_retention_state: Arc<cache_retention::CacheRetentionState>,
    artifact_reconciliation_cursor: AtomicI64,
    shutdown_requested: AtomicBool,
    shutdown_tx: broadcast::Sender<()>,
}

struct SupervisorAlertRequest {
    severity: &'static str,
    category: &'static str,
    failure_type: &'static str,
    component_type: &'static str,
    component_ref: Option<String>,
    summary: String,
    details: serde_json::Value,
    correlation_id: String,
}

impl SupervisorService {
    async fn new(config: Config) -> Result<Self> {
        let db = Database::new(&config.database).await?;
        RetentionRepository::seed_cache_config_if_empty(db.pool(), &config.cache_retention).await?;
        RetentionRepository::seed_native_config_if_empty(
            db.pool(),
            &config.retention.native_maintenance,
        )
        .await?;
        let (mq_connection, publisher) = Self::initialize_publisher(&config).await?;
        let (shutdown_tx, _) = broadcast::channel(1);

        Ok(Self {
            inner: Arc::new(SupervisorServiceInner {
                pool: db.pool().clone(),
                artifact_transport: Arc::new(VolumeTransport::new(&config.artifacts_dir)),
                blob_store: blob_store_from_config(&config.storage)?,
                publisher,
                _mq_connection: mq_connection,
                config,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        })
    }

    async fn initialize_publisher(
        config: &Config,
    ) -> Result<(Option<MqConnection>, Option<Arc<Publisher>>)> {
        let Some(mq_config) = config.message_queue.as_ref() else {
            warn!(
                "Message queue is not configured; supervisor corrective actions will update database state but cannot publish lifecycle wakeups"
            );
            return Ok((None, None));
        };

        let mq_connection = MqConnection::connect(&mq_config.url).await?;
        let default_mq_config = MessageQueueConfig::default();
        if let Err(err) = mq_connection
            .setup_common_infrastructure(&default_mq_config)
            .await
        {
            warn!(error = %err, "Failed to ensure common MQ infrastructure for supervisor");
        }

        let exchange_name = default_mq_config.rabbitmq.exchanges.executions.name.clone();
        let publisher = Publisher::new(
            &mq_connection,
            PublisherConfig {
                confirm_publish: true,
                timeout_secs: 30,
                exchange: exchange_name,
            },
        )
        .await?;

        Ok((Some(mq_connection), Some(Arc::new(publisher))))
    }

    async fn start(&self) -> Result<()> {
        let mut shutdown_rx = self.inner.shutdown_tx.subscribe();
        let mut interval = Duration::from_secs(60);

        info!(
            fallback_check_interval_seconds = self.inner.config.retention.check_interval_seconds,
            "Supervisor maintenance scheduler started; runtime settings are loaded from the database each cycle"
        );

        let mut cycle_reason = SupervisorCycleReason::StartupRecovery;
        loop {
            match self.run_retention_cycle(cycle_reason).await {
                Ok(next_interval) => {
                    interval = next_interval;
                }
                Err(err) => {
                    error!("Supervisor maintenance cycle failed: {}", err);
                }
            }
            cycle_reason = SupervisorCycleReason::Scheduled;

            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Supervisor shutdown signal received");
                    break;
                }
                _ = tokio::time::sleep(interval) => {}
            }
        }

        self.mark_supervisor_run_clean("graceful_shutdown").await;
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        self.inner.shutdown_requested.store(true, Ordering::Release);
        let _ = self.inner.shutdown_tx.send(());
        Ok(())
    }

    async fn run_retention_cycle(&self, cycle_reason: SupervisorCycleReason) -> Result<Duration> {
        self.run_retention_cycle_at(cycle_reason, Utc::now()).await
    }

    async fn run_retention_cycle_at(
        &self,
        cycle_reason: SupervisorCycleReason,
        now: DateTime<Utc>,
    ) -> Result<Duration> {
        if self.inner.shutdown_requested.load(Ordering::Acquire) {
            return Ok(Duration::from_secs(60));
        }
        let mut shutdown_rx = self.inner.shutdown_tx.subscribe();
        let mut cancelled = false;
        let mut retention = RetentionRepository::load_config(&self.inner.pool).await?;
        let native_validation_error = retention.native_maintenance.validate().err();
        if native_validation_error.is_some() {
            // Retention's worker-history invalidation also uses native timeout
            // settings. Keep its row-delete fallback valid at this boundary.
            retention.native_maintenance = Default::default();
            retention.native_maintenance.enabled = false;
        }
        let interval = native_maintenance::poll_interval(&retention);

        let mut conn = self.inner.pool.acquire().await?;

        if !RetentionRepository::try_advisory_lock(&mut conn, retention.advisory_lock_key).await? {
            info!(
                advisory_lock_key = retention.advisory_lock_key,
                "Another supervisor owns the retention lock; skipping cycle"
            );
            return Ok(interval);
        }
        // Session locks survive transaction rollback. A cancelled/panicking
        // leader must never return its locked connection to the pool.
        conn.close_on_drop();

        let cycle_result = async {
            let cycle_reason = self.ensure_supervisor_run(cycle_reason).await?;
            ScheduleRepository::ensure(&self.inner.pool, now).await?;
            info!(
                cycle_reason = cycle_reason.log_label(),
                check_interval_seconds = retention.check_interval_seconds,
                "Starting supervisor maintenance cycle"
            );
            // Reconcile future routing before source expiry. This is one bounded
            // attempt, never a catch-up loop ahead of housekeeping.
            if native_validation_error.is_none() {
                self.run_native_maintenance(
                    &retention,
                    cycle_reason,
                    now,
                    &[MaintenanceJob::Partition],
                )
                .await;
            }
            if self.inner.shutdown_requested.load(Ordering::Acquire) {
                return Ok(());
            }

            // Housekeeping retains its original cadence. A summary tick does not
            // trigger row retention, cache expiry, or artifact cleanup again.
            let retention_due = ScheduleRepository::begin_attempt(
                &self.inner.pool,
                MaintenanceJob::Retention,
                now,
                retention.check_interval_seconds,
                false,
            )
            .await?;
            let mut retention_success = true;
            if retention_due && retention.enabled {
                let targets = RetentionRepository::configured_targets(&retention.targets);
                info!(
                    target_count = targets.len(),
                    batch_size = retention.batch_size,
                    max_batches_per_target = retention.max_batches_per_target,
                    dry_run = retention.dry_run,
                    "Starting retention target cleanup"
                );

                for target in targets {
                    let Some(max_age_seconds) = target.max_age_seconds else {
                        info!(
                            target = target.target.name(),
                            "Retention target configured to keep forever"
                        );
                        continue;
                    };

                    match RetentionRepository::run_target_bounded(
                        &self.inner.pool,
                        target.target,
                        max_age_seconds,
                        &retention,
                        || {
                            cancelled |= self.inner.shutdown_requested.load(Ordering::Acquire) || !matches!(
                                shutdown_rx.try_recv(),
                                Err(broadcast::error::TryRecvError::Empty)
                            );
                            cancelled
                        },
                    )
                    .await
                    {
                        Ok(result) => {
                            log_target_result(&result);
                            if let Err(err) = self
                                .audit_retention_target_completed(
                                    &result,
                                    max_age_seconds,
                                    &retention,
                                )
                                .await
                            {
                                warn!(
                                    target = result.target.name(),
                                    error = %err,
                                    "Failed to audit retention target completion"
                                );
                            }
                        }
                        Err(err) => {
                            retention_success = false;
                            warn!(
                                target = target.target.name(),
                                cutoff = %err.cutoff,
                                candidates = ?err.candidates,
                                deleted = err.deleted,
                                partitions_dropped = err.partitions_dropped,
                                partition_candidates = err.partition_candidates,
                                candidates_exact = err.candidates_exact,
                                error = %err,
                                "Retention target failed"
                            );
                            if let Err(audit_err) = self
                                .audit_retention_target_failed(
                                    &err,
                                    max_age_seconds,
                                    &retention,
                                )
                                .await
                            {
                                warn!(
                                    target = target.target.name(),
                                    error = %audit_err,
                                    "Failed to audit retention target failure"
                                );
                            }
                        }
                    }
                    if cancelled {
                        ScheduleRepository::finish_attempt(
                            &self.inner.pool,
                            MaintenanceJob::Retention,
                            now,
                            retention.check_interval_seconds,
                            false,
                        )
                        .await?;
                        info!("Retention cancelled at a batch boundary");
                        return Ok(());
                    }
                }
            } else if retention_due {
                info!(
                    check_interval_seconds = retention.check_interval_seconds,
                    "Runtime retention is disabled in database config; running non-retention maintenance only"
                );
            }

            if retention_due {
                self.run_cache_retention_step(&retention.cache_retention).await;
            }

            // Startup remediation still runs when a previous leader completed
            // retention recently. Summary catch-up gets a turn after housekeeping.
            if retention_due || !matches!(cycle_reason, SupervisorCycleReason::Scheduled) {
                self.run_maintenance_cycle(&retention).await;
            }
            if retention_due {
                ScheduleRepository::finish_attempt(
                    &self.inner.pool,
                    MaintenanceJob::Retention,
                    now,
                    retention.check_interval_seconds,
                    retention_success,
                )
                .await?;
            }
            // Invalid persisted native settings disable only native work for
            // this cycle, so they cannot prevent protected-row cleanup/remediation.
            if let Some(error) = native_validation_error {
                warn!(error, "Persisted native maintenance configuration is invalid");
                self.report_native_problem("invalid_config", json!({"invalid_config": true})).await;
            } else {
                self.run_native_maintenance(
                    &retention,
                    cycle_reason,
                    now,
                    &[MaintenanceJob::Summary],
                ).await;
            }

            info!("Supervisor maintenance cycle finished");
            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(err) =
            RetentionRepository::advisory_unlock(&mut conn, retention.advisory_lock_key).await
        {
            warn!(
                advisory_lock_key = retention.advisory_lock_key,
                error = %err,
                "Failed to release retention advisory lock"
            );
        }
        if let Err(error) = conn.close().await {
            warn!(error = %error, "Failed to close supervisor leader session");
        }

        cycle_result.map(|_| interval)
    }

    async fn ensure_supervisor_run(
        &self,
        requested_reason: SupervisorCycleReason,
    ) -> Result<SupervisorCycleReason> {
        let mut run_id_guard = self.inner.run_id.lock().await;
        if let Some(run_id) = run_id_guard.as_deref() {
            MaintenanceRepository::heartbeat_supervisor_run(&self.inner.pool, run_id).await?;
            return Ok(requested_reason);
        }

        let instance_id = supervisor_instance_id(&self.inner.config.service_name);
        let run_id = supervisor_run_id(&self.inner.config.service_name);
        let startup = MaintenanceRepository::start_supervisor_run(
            &self.inner.pool,
            &self.inner.config.service_name,
            &instance_id,
            &run_id,
        )
        .await?;
        *run_id_guard = Some(startup.run_id);

        if startup.dirty_shutdown_detected {
            warn!(
                service_name = self.inner.config.service_name,
                "Dirty supervisor shutdown detected; running startup recovery checks"
            );
            Ok(SupervisorCycleReason::DirtyShutdownRecovery)
        } else {
            // This instance may have skipped its initial startup tick while
            // another leader held the lock. Its first owned turn is still startup.
            Ok(SupervisorCycleReason::StartupRecovery)
        }
    }

    async fn mark_supervisor_run_clean(&self, stop_reason: &str) {
        let run_id = { self.inner.run_id.lock().await.clone() };
        let Some(run_id) = run_id else {
            return;
        };
        if let Err(err) =
            MaintenanceRepository::mark_supervisor_run_clean(&self.inner.pool, &run_id, stop_reason)
                .await
        {
            warn!(run_id, error = %err, "Failed to mark supervisor run as cleanly stopped");
        }
    }

    async fn audit_retention_target_completed(
        &self,
        result: &RetentionTargetResult,
        max_age_seconds: u64,
        retention: &attune_common::config::RetentionConfig,
    ) -> Result<()> {
        if result.candidates == 0
            && result.deleted == 0
            && result.partitions_dropped == 0
            && !result.dry_run
        {
            return Ok(());
        }

        let details = json!({
            "target": result.target.name(),
            "cutoff": result.cutoff.map(|cutoff| cutoff.to_rfc3339()),
            "max_age_seconds": max_age_seconds,
            "candidates": result.candidates,
            "deleted": result.deleted,
            "rows_deleted": result.deleted,
            "partitions_dropped": result.partitions_dropped,
            "partition_candidates": result.partition_candidates,
            "candidates_exact": result.candidates_exact,
            "dry_run": result.dry_run,
            "retention_enabled": retention.enabled,
            "batch_size": retention.batch_size,
            "max_batches_per_target": retention.max_batches_per_target,
            "advisory_lock_key": retention.advisory_lock_key,
            "service_name": self.inner.config.service_name,
            "environment": self.inner.config.environment,
        });

        let event = AuditEventBuilder::new(
            AuditCategory::Admin,
            event_type::maintenance::RETENTION_TARGET_COMPLETED,
            AuditOutcome::Success,
        )
        .actor_login("attune-supervisor")
        .actor_token_type("system")
        .resource("runtime_retention")
        .resource_ref(result.target.name())
        .with_details(details)
        .build();

        AuditRepository::insert(&self.inner.pool, event).await?;
        Ok(())
    }

    async fn audit_retention_target_failed(
        &self,
        failure: &RetentionTargetFailure,
        max_age_seconds: u64,
        retention: &attune_common::config::RetentionConfig,
    ) -> Result<()> {
        let details = json!({
            "target": failure.target.name(),
            "cutoff": failure.cutoff.to_rfc3339(),
            "candidates": failure.candidates,
            "deleted": failure.deleted,
            "rows_deleted": failure.deleted,
            "partitions_dropped": failure.partitions_dropped,
            "partition_candidates": failure.partition_candidates,
            "candidates_exact": failure.candidates_exact,
            "deleted_count_complete": false,
            "max_age_seconds": max_age_seconds,
            "dry_run": failure.dry_run,
            "batch_size": retention.batch_size,
            "max_batches_per_target": retention.max_batches_per_target,
            "advisory_lock_key": retention.advisory_lock_key,
            "service_name": self.inner.config.service_name,
            "environment": self.inner.config.environment,
            "error": failure.to_string(),
        });

        let event = AuditEventBuilder::new(
            AuditCategory::Admin,
            event_type::maintenance::RETENTION_TARGET_FAILED,
            AuditOutcome::Failure,
        )
        .actor_login("attune-supervisor")
        .actor_token_type("system")
        .resource("runtime_retention")
        .resource_ref(failure.target.name())
        .with_details(details)
        .build();

        AuditRepository::insert(&self.inner.pool, event).await?;
        Ok(())
    }

    /// Runs the cache subsystem retention/freshness step as a distinct part
    /// of the existing supervisor retention cycle, reusing its advisory lock
    /// and cadence (see `docs/KEY_CACHE.md`, "Gap 1"). Configuration is loaded
    /// from the database with the enclosing retention config every cycle.
    /// Failures here are logged and never abort the rest of maintenance.
    async fn run_cache_retention_step(&self, cache_retention: &CacheRetentionConfig) {
        if !cache_retention.enabled {
            info!("Cache retention is disabled in configuration; skipping cache cleanup step");
            return;
        }

        let ctx = cache_retention::CacheRetentionContext {
            pool: &self.inner.pool,
            publisher: self.inner.publisher.as_deref(),
            service_name: &self.inner.config.service_name,
            environment: &self.inner.config.environment,
            state: self.inner.cache_retention_state.clone(),
        };

        match cache_retention::run_cache_retention_cycle(&ctx, cache_retention).await {
            Ok(summary) => {
                if summary.had_effect() {
                    if let Err(err) = self
                        .audit_corrective_action_with_outcome(
                            "cache_retention",
                            if summary.maintenance_failures + summary.cleanup_failures + summary.statistics_failures > 0 {
                                "cache_cleanup_cycle_partial_failure"
                            } else {
                                "cache_cleanup_cycle_completed"
                            },
                            json!({
                                "dry_run": summary.dry_run,
                                "namespaces_scanned": summary.namespaces_scanned,
                                "staging_expired": summary.staging_expired,
                                "cleanup_candidates": summary.cleanup_candidates,
                                "entries_deleted": summary.entries_deleted,
                                "generations_deleted": summary.generations_deleted,
                                "maintenance_failures": summary.maintenance_failures,
                                "registered_partitions": summary.registered_partitions,
                                "partitions_created_total": summary.partitions_created_total,
                                "partitions_dropped_total": summary.partitions_dropped_total,
                                "cleanup_backlog_total": summary.cleanup_backlog_total,
                                "oldest_cleanup_age_seconds": summary.oldest_cleanup_age_seconds,
                                "reclamation_duration_ms": summary.reclamation_duration_ms,
                                "statistics_duration_ms": summary.statistics_duration_ms,
                                "statistics_pending": summary.statistics_pending,
                                "storage_observed": summary.storage_observed,
                                "statistics_age_seconds": summary.statistics_age_seconds,
                                "statistics_refreshed": summary.statistics_refreshed,
                                "statistics_lock_deferrals": summary.statistics_lock_deferrals,
                                "statistics_deadline_deferrals": summary.statistics_deadline_deferrals,
                                "statistics_failures": summary.statistics_failures,
                                "cleanup_failures": summary.cleanup_failures,
                                "bytes_reclaimed": summary.bytes_reclaimed,
                                "lock_deferrals": summary.lock_deferrals,
                                "deadline_deferrals": summary.deadline_deferrals,
                                "cleanup_budget_exhausted": summary.cleanup_budget_exhausted,
                                "namespaces_deleted": summary.namespaces_deleted,
                                "freshness_alerts": summary.freshness_alerts,
                                "staging_failure_alerts": summary.staging_failure_alerts,
                            }),
                            if summary.maintenance_failures + summary.cleanup_failures + summary.statistics_failures > 0 {
                                AuditOutcome::Failure
                            } else {
                                AuditOutcome::Success
                            },
                        )
                        .await
                    {
                        warn!(error = %err, "Failed to audit cache retention cycle completion");
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, "Cache retention step failed");
            }
        }
    }

    async fn run_maintenance_cycle(&self, retention: &RetentionConfig) {
        let maintenance = &self.inner.config.maintenance;
        if !maintenance.enabled {
            info!("Supervisor maintenance jobs are disabled; skipping");
            return;
        }

        if maintenance.artifact_cleanup_enabled {
            match artifact_cleanup::cleanup_expired_artifacts(
                &self.inner.pool,
                self.inner.artifact_transport.as_ref(),
                maintenance,
            )
            .await
            {
                Ok(result) => {
                    if result.candidates > 0 || result.deleted_versions > 0 {
                        info!(
                            candidates = result.candidates,
                            deleted_versions = result.deleted_versions,
                            deleted_files = result.deleted_files,
                            deleted_artifacts = result.deleted_artifacts,
                            "Artifact cleanup completed"
                        );
                        if let Err(err) = self.audit_artifact_cleanup_completed(&result).await {
                            warn!(error = %err, "Failed to audit artifact cleanup completion");
                        }
                    }
                }
                Err(err) => {
                    warn!(error = %err, "Artifact cleanup failed");
                }
            }
        }

        if let Err(err) = self.reconcile_artifact_object_metadata(maintenance).await {
            warn!(error = %err, "Artifact object metadata reconciliation failed");
        }

        match object_retention::run_cycle(
            &self.inner.pool,
            &self.inner.blob_store,
            std::path::Path::new(&self.inner.config.packs_base_dir),
            maintenance,
        )
        .await
        {
            Ok(metrics) => info!(
                retained_releases = metrics.retained_releases,
                deleted_releases = metrics.deleted_releases,
                pending_collection = metrics.pending_collection,
                deleted_objects = metrics.deleted_objects,
                deleted_bytes = metrics.deleted_bytes,
                failures = metrics.failures,
                "Object retention cycle completed"
            ),
            Err(err) => warn!(error = %err, "Object retention cycle failed"),
        }

        if let Err(err) = self.delete_expired_legacy_snapshots(maintenance).await {
            warn!(error = %err, "Legacy snapshot cleanup failed");
        }

        if maintenance.monitoring_enabled {
            if let Err(err) = self.emit_stuck_runtime_alerts(maintenance).await {
                warn!(error = %err, "Stuck runtime monitoring failed");
            }

            if let Err(err) = self.emit_retention_lag_alerts(retention, maintenance).await {
                warn!(error = %err, "Retention lag monitoring failed");
            }
        }

        if maintenance.corrective_actions_enabled {
            if let Err(err) = self.run_corrective_actions(maintenance).await {
                warn!(error = %err, "Supervisor corrective actions failed");
            }
        }
    }

    async fn delete_expired_legacy_snapshots(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        for (id, path) in StorageMaintenanceRepository::expired_pack_snapshots(
            &self.inner.pool,
            maintenance.artifact_cleanup_batch_size,
        )
        .await?
        {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            StorageMaintenanceRepository::clear_pack_snapshot(&self.inner.pool, id).await?;
        }
        for (id, path) in StorageMaintenanceRepository::expired_artifact_snapshots(
            &self.inner.pool,
            maintenance.artifact_cleanup_batch_size,
        )
        .await?
        {
            self.inner.artifact_transport.delete_file(&path).await?;
            StorageMaintenanceRepository::clear_artifact_snapshot(&self.inner.pool, id).await?;
        }
        Ok(())
    }

    async fn reconcile_artifact_object_metadata(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        ArtifactUploadGrantRepository::mark_expired(
            &self.inner.pool,
            maintenance.artifact_cleanup_batch_size,
        )
        .await?;
        ArtifactUploadGrantRepository::purge_terminal(
            &self.inner.pool,
            Utc::now() - ChronoDuration::days(1),
            maintenance.artifact_cleanup_batch_size,
        )
        .await?;
        let pending_cutoff = Utc::now()
            - ChronoDuration::seconds(
                maintenance
                    .object_upload_abandon_seconds
                    .min(i64::MAX as u64) as i64,
            );
        for candidate in StorageMaintenanceRepository::abandoned_pending(
            &self.inner.pool,
            pending_cutoff,
            maintenance.artifact_cleanup_batch_size,
        )
        .await?
        {
            if !StorageMaintenanceRepository::claim_abandoned_pending(
                &self.inner.pool,
                candidate.id,
                pending_cutoff,
            )
            .await?
            {
                continue;
            }
            if StorageMaintenanceRepository::delete_cleanup_claimed(&self.inner.pool, candidate.id)
                .await?
            {
                MaintenanceRepository::refresh_or_delete_artifact_metadata(
                    &self.inner.pool,
                    candidate.artifact,
                )
                .await?;
            }
        }

        artifact_cleanup::cleanup_abandoned_shared_logs(
            &self.inner.pool,
            self.inner.artifact_transport.as_ref(),
            maintenance,
        )
        .await?;

        let after_id = self
            .inner
            .artifact_reconciliation_cursor
            .load(Ordering::Relaxed);
        let candidates = StorageMaintenanceRepository::ready_objects(
            &self.inner.pool,
            after_id,
            maintenance.artifact_cleanup_batch_size,
        )
        .await?;
        if candidates.is_empty() {
            self.inner
                .artifact_reconciliation_cursor
                .store(0, Ordering::Relaxed);
        }
        for candidate in candidates {
            self.inner
                .artifact_reconciliation_cursor
                .store(candidate.id, Ordering::Relaxed);
            let matches = if let Some(stream) =
                LogStreamRepository::find_by_artifact_version(&self.inner.pool, candidate.id)
                    .await?
            {
                let mut matches = stream.sealed;
                for segment in LogStreamRepository::segments(&self.inner.pool, stream.id).await? {
                    let key = ObjectKey::new(segment.object_key)?;
                    matches &= self
                        .inner
                        .blob_store
                        .head(&key)
                        .await?
                        .is_some_and(|object| {
                            object.provider_version.as_stored() == segment.provider_version
                                && object.size == segment.size_bytes as u64
                                && hex_digest(&object.sha256) == segment.sha256
                        });
                }
                matches
            } else {
                let key = ObjectKey::new(candidate.object_key.clone())?;
                self.inner
                    .blob_store
                    .head(&key)
                    .await?
                    .is_some_and(|object| {
                        candidate.provider_version.as_deref()
                            == Some(object.provider_version.as_stored())
                            && candidate.size_bytes == Some(object.size as i64)
                            && candidate.sha256.as_deref()
                                == Some(hex_digest(&object.sha256).as_str())
                    })
            };
            if !matches
                && MaintenanceRepository::delete_artifact_version(&self.inner.pool, candidate.id)
                    .await?
            {
                MaintenanceRepository::refresh_or_delete_artifact_metadata(
                    &self.inner.pool,
                    candidate.artifact,
                )
                .await?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    async fn run_object_maintenance(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        self.reconcile_artifact_object_metadata(maintenance).await?;
        object_retention::run_cycle(
            &self.inner.pool,
            &self.inner.blob_store,
            std::path::Path::new(&self.inner.config.packs_base_dir),
            maintenance,
        )
        .await?;
        self.delete_expired_legacy_snapshots(maintenance).await
    }

    async fn emit_stuck_runtime_alerts(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        let snapshots = MaintenanceRepository::stuck_runtime_snapshots(
            &self.inner.pool,
            maintenance.stuck_execution_seconds,
            maintenance.stuck_queue_seconds,
        )
        .await?;

        let mut emitted = 0;
        for snapshot in snapshots {
            if emitted >= maintenance.alert_limit_per_cycle {
                break;
            }

            let correlation_id = format!(
                "supervisor:stuck-runtime:{}:{}",
                snapshot.kind, snapshot.status
            );
            if self
                .alert_recently_emitted(&correlation_id, maintenance)
                .await?
            {
                continue;
            }

            let summary = format!(
                "{} {} rows appear stuck in status '{}'",
                snapshot.count, snapshot.kind, snapshot.status
            );
            self.emit_supervisor_alert(SupervisorAlertRequest {
                severity: "warning",
                category: "maintenance",
                failure_type: "stuck_runtime_state",
                component_type: snapshot.kind,
                component_ref: Some(snapshot.status.clone()),
                summary,
                details: json!({
                    "status": snapshot.status,
                    "count": snapshot.count,
                    "oldest": snapshot.oldest.to_rfc3339(),
                }),
                correlation_id,
            })
            .await?;
            emitted += 1;
        }

        Ok(())
    }

    async fn emit_retention_lag_alerts(
        &self,
        retention: &RetentionConfig,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        if !retention.enabled {
            return Ok(());
        }

        let targets = RetentionRepository::configured_targets(&retention.targets);
        let mut emitted = 0;

        for target in targets {
            if emitted >= maintenance.alert_limit_per_cycle {
                break;
            }

            let Some(max_age_seconds) = target.max_age_seconds else {
                continue;
            };
            let lag_seconds =
                max_age_seconds.saturating_add(maintenance.retention_lag_alert_seconds);
            let cutoff =
                Utc::now() - ChronoDuration::seconds(lag_seconds.min(i64::MAX as u64) as i64);
            let count = RetentionRepository::count_target_candidates(
                &self.inner.pool,
                target.target,
                cutoff,
            )
            .await?;
            if count == 0 {
                continue;
            }

            let correlation_id = format!("supervisor:retention-lag:{}", target.target.name());
            if self
                .alert_recently_emitted(&correlation_id, maintenance)
                .await?
            {
                continue;
            }

            let summary = format!(
                "{} rows remain beyond retention lag threshold for {}",
                count,
                target.target.name()
            );
            self.emit_supervisor_alert(SupervisorAlertRequest {
                severity: "warning",
                category: "maintenance",
                failure_type: "retention_lag",
                component_type: "retention_target",
                component_ref: Some(target.target.name().to_string()),
                summary,
                details: json!({
                    "target": target.target.name(),
                    "count": count,
                    "max_age_seconds": max_age_seconds,
                    "lag_grace_seconds": maintenance.retention_lag_alert_seconds,
                    "cutoff": cutoff.to_rfc3339(),
                }),
                correlation_id,
            })
            .await?;
            emitted += 1;
        }

        Ok(())
    }

    async fn run_corrective_actions(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        self.republish_stale_requested_executions(maintenance)
            .await?;
        self.republish_stale_cache_iteration_completions(maintenance)
            .await?;
        self.remediate_stale_executions(maintenance).await?;

        let queue_result = MaintenanceRepository::remediate_work_queue_state(
            &self.inner.pool,
            maintenance.queue_remediation_seconds,
        )
        .await?;
        if queue_result.dispatches_corrected > 0 || queue_result.items_corrected > 0 {
            self.emit_queue_remediation_alert(&queue_result).await?;
            self.audit_corrective_action(
                "work_queue",
                "stale_queue_leases_reconciled",
                json!({
                    "dispatches_corrected": queue_result.dispatches_corrected,
                    "items_corrected": queue_result.items_corrected,
                }),
            )
            .await?;
        }

        let admission_result = MaintenanceRepository::remediate_admission_state(
            &self.inner.pool,
            maintenance.admission_remediation_seconds,
        )
        .await?;
        if admission_result.entries_removed > 0 {
            self.emit_admission_remediation_alert(&admission_result)
                .await?;
            self.audit_corrective_action(
                "execution_admission",
                "stale_admission_entries_reconciled",
                json!({
                    "entries_removed": admission_result.entries_removed,
                    "active_entries_removed": admission_result.active_entries_removed,
                    "promoted_execution_ids": admission_result.promoted_execution_ids,
                }),
            )
            .await?;
            for execution_id in &admission_result.promoted_execution_ids {
                self.publish_execution_requested(*execution_id).await?;
            }
        }

        let workflow_result = MaintenanceRepository::remediate_workflow_state(
            &self.inner.pool,
            maintenance.execution_remediation_seconds,
        )
        .await?;
        if workflow_result.workflow_executions_corrected > 0 {
            self.emit_workflow_remediation_alert(&workflow_result)
                .await?;
            self.audit_corrective_action(
                "workflow_execution",
                "stale_workflow_state_reconciled",
                json!({
                    "workflow_executions_corrected": workflow_result.workflow_executions_corrected,
                    "parent_executions_corrected": workflow_result.parent_executions_corrected,
                }),
            )
            .await?;
            for execution_id in &workflow_result.parent_executions_corrected {
                if let Some(execution) =
                    ExecutionRepository::find_by_id(&self.inner.pool, *execution_id).await?
                {
                    self.publish_execution_completed(&execution).await?;
                }
            }
        }

        let cache_iteration_result =
            WorkflowCacheIterationRepository::remediate_scanning_for_terminal_workflows(
                &self.inner.pool,
                maintenance.execution_remediation_batch_size,
            )
            .await?;
        if cache_iteration_result.total() > 0 {
            self.audit_corrective_action(
                "workflow_cache_iteration",
                "terminal_workflow_cache_iterations_reconciled",
                json!({
                    "iterations_corrected": cache_iteration_result.total(),
                    "completed": cache_iteration_result.completed,
                    "failed": cache_iteration_result.failed,
                    "cancelled": cache_iteration_result.cancelled,
                }),
            )
            .await?;
        }

        Ok(())
    }

    async fn republish_stale_cache_iteration_completions(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        if self.inner.publisher.is_none() {
            warn!(
                "Skipping synthetic cache iteration completion recovery because MQ publisher is unavailable"
            );
            return Ok(());
        }

        let candidates = WorkflowCacheIterationRepository::find_stale_synthetic_completions(
            &self.inner.pool,
            maintenance.execution_remediation_seconds,
            maintenance.execution_remediation_batch_size,
        )
        .await?;
        if candidates.is_empty() {
            return Ok(());
        }

        let mut execution_ids = Vec::with_capacity(candidates.len());
        for candidate in &candidates {
            self.publish_synthetic_cache_iteration_completion(candidate)
                .await?;
            execution_ids.push(candidate.execution_id);
        }
        self.audit_corrective_action(
            "workflow_cache_iteration",
            "synthetic_completion_messages_republished",
            json!({
                "messages_republished": execution_ids.len(),
                "execution_ids": execution_ids,
                "grace_seconds": maintenance.execution_remediation_seconds,
            }),
        )
        .await?;
        Ok(())
    }

    async fn republish_stale_requested_executions(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        if self.inner.publisher.is_none() {
            warn!(
                "Skipping requested execution reschedule recovery because MQ publisher is unavailable"
            );
            return Ok(());
        }

        let candidates = MaintenanceRepository::find_requested_executions_for_reschedule(
            &self.inner.pool,
            maintenance.execution_reschedule_grace_seconds,
            maintenance.execution_reschedule_max_attempts,
            maintenance.alert_limit_per_cycle,
        )
        .await?;

        for candidate in candidates {
            let Some(attempt) = MaintenanceRepository::mark_execution_reschedule_attempt(
                &self.inner.pool,
                candidate.execution_id,
                "attune-supervisor",
                "requested execution remained stale after scheduler message may have been lost",
                maintenance.execution_reschedule_max_attempts,
                maintenance.execution_reschedule_grace_seconds,
                false,
            )
            .await?
            else {
                continue;
            };

            self.publish_execution_requested_attempt(&attempt).await?;
            self.audit_corrective_action(
                "execution",
                "requested_execution_republished",
                json!({
                    "execution_id": attempt.execution_id,
                    "action_ref": attempt.action_ref,
                    "attempt_count": attempt.attempt_count,
                    "last_attempt_at": attempt.last_attempt_at,
                    "previous_attempt_count": candidate.attempt_count,
                    "previous_last_attempt_at": candidate.last_attempt_at,
                    "source": attempt.last_source,
                    "reason": attempt.last_reason,
                }),
            )
            .await?;
        }

        Ok(())
    }

    async fn remediate_stale_executions(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<()> {
        let candidates = MaintenanceRepository::find_stale_execution_candidates(
            &self.inner.pool,
            maintenance.execution_remediation_seconds,
            maintenance.execution_remediation_batch_size,
        )
        .await?;

        for candidate in candidates {
            let Some(expected_status) = parse_execution_status(&candidate.status) else {
                continue;
            };
            let new_status = match expected_status {
                ExecutionStatus::Canceling => ExecutionStatus::Cancelled,
                ExecutionStatus::Requested
                | ExecutionStatus::Scheduling
                | ExecutionStatus::Scheduled
                | ExecutionStatus::Running => ExecutionStatus::Abandoned,
                _ => continue,
            };
            let result = json!({
                "error": "Execution was reconciled by attune-supervisor after remaining non-terminal beyond the remediation threshold",
                "corrected_by": "attune-supervisor",
                "previous_status": candidate.status,
                "new_status": format!("{:?}", new_status).to_lowercase(),
                "stale_since": candidate.updated,
                "worker": candidate.worker,
                "corrected_at": Utc::now(),
            });
            let updated = ExecutionRepository::update_if_status(
                &self.inner.pool,
                candidate.id,
                expected_status,
                UpdateExecutionInput {
                    status: Some(new_status),
                    result: Some(result.clone()),
                    ..Default::default()
                },
            )
            .await?;

            let Some(execution) = updated else {
                continue;
            };
            self.publish_execution_completed(&execution).await?;
            self.emit_execution_remediation_alert(&candidate, &execution, result.clone())
                .await?;
            self.audit_corrective_action(
                "execution",
                "stale_execution_reconciled",
                json!({
                    "execution_id": execution.id,
                    "action_ref": execution.action_ref,
                    "previous_status": candidate.status,
                    "new_status": execution.status,
                    "result": result,
                }),
            )
            .await?;
        }

        Ok(())
    }

    async fn publish_execution_requested_attempt(
        &self,
        attempt: &ExecutionRescheduleAttempt,
    ) -> Result<()> {
        let Some(publisher) = self.inner.publisher.as_ref() else {
            warn!(
                execution_id = attempt.execution_id,
                "Cannot republish requested execution because MQ publisher is unavailable"
            );
            return Ok(());
        };

        let payload = ExecutionRequestedPayload {
            execution_id: attempt.execution_id,
            action_id: attempt.action_id,
            action_ref: attempt.action_ref.clone(),
            parent_id: attempt.parent_id,
            enforcement_id: attempt.enforcement_id,
            config: attempt.config.clone(),
            release_id: attempt.release_id,
            release_digest: attempt.release_digest.clone(),
        };
        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("attune-supervisor");
        publisher.publish_envelope(&envelope).await?;
        Ok(())
    }

    async fn publish_execution_completed(&self, execution: &Execution) -> Result<()> {
        let Some(publisher) = self.inner.publisher.as_ref() else {
            warn!(
                execution_id = execution.id,
                "Cannot publish supervisor execution completion because MQ publisher is unavailable"
            );
            return Ok(());
        };
        let payload = ExecutionCompletedPayload {
            execution_id: execution.id,
            action_id: execution.action.unwrap_or_default(),
            action_ref: execution.action_ref.clone(),
            status: format!("{:?}", execution.status),
            result: execution.result.clone(),
            completed_at: Utc::now(),
        };
        let envelope = MessageEnvelope::new(MessageType::ExecutionCompleted, payload)
            .with_source("attune-supervisor");
        publisher.publish_envelope(&envelope).await?;
        Ok(())
    }

    async fn publish_synthetic_cache_iteration_completion(
        &self,
        candidate: &StaleSyntheticCacheIterationCompletion,
    ) -> Result<()> {
        let Some(publisher) = self.inner.publisher.as_ref() else {
            return Ok(());
        };
        let payload = ExecutionCompletedPayload {
            execution_id: candidate.execution_id,
            action_id: candidate.action_id.unwrap_or_default(),
            action_ref: candidate.action_ref.clone(),
            status: format!("{:?}", candidate.status),
            result: candidate.result.clone(),
            completed_at: candidate.completed_at,
        };
        let envelope = MessageEnvelope::new(MessageType::ExecutionCompleted, payload)
            .with_source("attune-supervisor");
        publisher.publish_envelope(&envelope).await?;
        Ok(())
    }

    async fn publish_execution_requested(&self, execution_id: i64) -> Result<()> {
        let Some(publisher) = self.inner.publisher.as_ref() else {
            warn!(
                execution_id,
                "Cannot republish promoted execution because MQ publisher is unavailable"
            );
            return Ok(());
        };
        let Some(execution) =
            ExecutionRepository::find_by_id(&self.inner.pool, execution_id).await?
        else {
            return Ok(());
        };
        let payload = ExecutionRequestedPayload {
            execution_id: execution.id,
            action_id: execution.action,
            action_ref: execution.action_ref.clone(),
            parent_id: execution.parent,
            enforcement_id: execution.enforcement,
            config: execution.config.clone(),
            release_id: execution.pack_release,
            release_digest: execution.pack_release_digest.clone(),
        };
        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("attune-supervisor");
        publisher.publish_envelope(&envelope).await?;
        Ok(())
    }

    async fn alert_recently_emitted(
        &self,
        correlation_id: &str,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<bool> {
        MaintenanceRepository::alert_recently_emitted(
            &self.inner.pool,
            correlation_id,
            maintenance.alert_cooldown_seconds,
        )
        .await
        .map_err(Into::into)
    }

    async fn emit_supervisor_alert(&self, mut request: SupervisorAlertRequest) -> Result<()> {
        let details = &mut request.details;
        if let Some(details_object) = details.as_object_mut() {
            details_object.insert(
                "service_name".to_string(),
                json!(self.inner.config.service_name),
            );
            details_object.insert(
                "environment".to_string(),
                json!(self.inner.config.environment),
            );
        }

        let alert = SystemAlert {
            severity: request.severity.to_string(),
            category: request.category.to_string(),
            failure_type: request.failure_type.to_string(),
            component_type: request.component_type.to_string(),
            component_id: None,
            component_ref: request.component_ref,
            worker_role: None,
            observed_at: Utc::now(),
            summary: request.summary,
            details: request.details,
            correlation_id: Some(request.correlation_id),
        };
        emit_core_alert(&self.inner.pool, self.inner.publisher.as_deref(), alert).await?;
        Ok(())
    }

    async fn emit_execution_remediation_alert(
        &self,
        candidate: &StaleExecutionCandidate,
        execution: &Execution,
        result: serde_json::Value,
    ) -> Result<()> {
        self.emit_supervisor_alert(SupervisorAlertRequest {
            severity: "warning",
            category: "maintenance",
            failure_type: "supervisor_corrective_action",
            component_type: "execution",
            component_ref: Some(execution.action_ref.clone()),
            summary: format!(
                "Supervisor changed execution {} from {} to {:?}",
                execution.id, candidate.status, execution.status
            ),
            details: json!({
                "execution_id": execution.id,
                "action_ref": execution.action_ref,
                "previous_status": candidate.status,
                "new_status": execution.status,
                "stale_since": candidate.updated,
                "remediation_result": result,
            }),
            correlation_id: format!("supervisor:corrective:execution:{}", execution.id),
        })
        .await
    }

    async fn emit_queue_remediation_alert(&self, result: &QueueRemediationResult) -> Result<()> {
        self.emit_supervisor_alert(SupervisorAlertRequest {
            severity: "warning",
            category: "maintenance",
            failure_type: "supervisor_corrective_action",
            component_type: "work_queue",
            component_ref: Some("stale_leases".to_string()),
            summary: format!(
                "Supervisor corrected {} queue dispatches and {} queue items",
                result.dispatches_corrected, result.items_corrected
            ),
            details: json!({
                "dispatches_corrected": result.dispatches_corrected,
                "items_corrected": result.items_corrected,
            }),
            correlation_id: "supervisor:corrective:work_queue:stale_leases".to_string(),
        })
        .await
    }

    async fn emit_admission_remediation_alert(
        &self,
        result: &AdmissionRemediationResult,
    ) -> Result<()> {
        self.emit_supervisor_alert(SupervisorAlertRequest {
            severity: "warning",
            category: "maintenance",
            failure_type: "supervisor_corrective_action",
            component_type: "execution_admission",
            component_ref: Some("stale_entries".to_string()),
            summary: format!(
                "Supervisor removed {} stale admission entries and promoted {} queued executions",
                result.entries_removed,
                result.promoted_execution_ids.len()
            ),
            details: json!({
                "entries_removed": result.entries_removed,
                "active_entries_removed": result.active_entries_removed,
                "promoted_execution_ids": result.promoted_execution_ids,
            }),
            correlation_id: "supervisor:corrective:execution_admission:stale_entries".to_string(),
        })
        .await
    }

    async fn emit_workflow_remediation_alert(
        &self,
        result: &WorkflowRemediationResult,
    ) -> Result<()> {
        self.emit_supervisor_alert(SupervisorAlertRequest {
            severity: "warning",
            category: "maintenance",
            failure_type: "supervisor_corrective_action",
            component_type: "workflow_execution",
            component_ref: Some("stale_state".to_string()),
            summary: format!(
                "Supervisor corrected {} stale workflow executions",
                result.workflow_executions_corrected
            ),
            details: json!({
                "workflow_executions_corrected": result.workflow_executions_corrected,
                "parent_executions_corrected": result.parent_executions_corrected,
            }),
            correlation_id: "supervisor:corrective:workflow_execution:stale_state".to_string(),
        })
        .await
    }

    async fn audit_corrective_action(
        &self,
        resource_type: &str,
        action: &str,
        details: serde_json::Value,
    ) -> Result<()> {
        self.audit_corrective_action_with_outcome(
            resource_type,
            action,
            details,
            AuditOutcome::Success,
        )
        .await
    }

    async fn audit_corrective_action_with_outcome(
        &self,
        resource_type: &str,
        action: &str,
        details: serde_json::Value,
        outcome: AuditOutcome,
    ) -> Result<()> {
        let event = AuditEventBuilder::new(
            AuditCategory::Admin,
            event_type::maintenance::CORRECTIVE_ACTION_APPLIED,
            outcome,
        )
        .actor_login("attune-supervisor")
        .actor_token_type("system")
        .resource(resource_type)
        .resource_ref(action)
        .with_details(json!({
            "action": action,
            "service_name": self.inner.config.service_name,
            "environment": self.inner.config.environment,
            "details": details,
        }))
        .build();

        AuditRepository::insert(&self.inner.pool, event).await?;
        Ok(())
    }

    async fn audit_artifact_cleanup_completed(&self, result: &ArtifactCleanupResult) -> Result<()> {
        let details = json!({
            "candidates": result.candidates,
            "deleted_versions": result.deleted_versions,
            "deleted_files": result.deleted_files,
            "deleted_artifacts": result.deleted_artifacts,
            "artifact_transport": self.inner.artifact_transport.transport_mode(),
            "service_name": self.inner.config.service_name,
            "environment": self.inner.config.environment,
        });

        let event = AuditEventBuilder::new(
            AuditCategory::Admin,
            event_type::maintenance::ARTIFACT_CLEANUP_COMPLETED,
            AuditOutcome::Success,
        )
        .actor_login("attune-supervisor")
        .actor_token_type("system")
        .resource("artifact")
        .resource_ref("time_based_retention")
        .with_details(details)
        .build();

        AuditRepository::insert(&self.inner.pool, event).await?;
        Ok(())
    }
}

fn log_target_result(result: &RetentionTargetResult) {
    info!(
        target = result.target.name(),
        cutoff = ?result.cutoff,
        candidates = result.candidates,
        deleted = result.deleted,
        rows_deleted = result.deleted,
        partitions_dropped = result.partitions_dropped,
        partition_candidates = result.partition_candidates,
        candidates_exact = result.candidates_exact,
        dry_run = result.dry_run,
        "Retention target completed"
    );
}

fn parse_execution_status(status: &str) -> Option<ExecutionStatus> {
    match status {
        "requested" => Some(ExecutionStatus::Requested),
        "scheduling" => Some(ExecutionStatus::Scheduling),
        "scheduled" => Some(ExecutionStatus::Scheduled),
        "running" => Some(ExecutionStatus::Running),
        "completed" => Some(ExecutionStatus::Completed),
        "failed" => Some(ExecutionStatus::Failed),
        "canceling" => Some(ExecutionStatus::Canceling),
        "cancelled" => Some(ExecutionStatus::Cancelled),
        "timeout" => Some(ExecutionStatus::Timeout),
        "abandoned" => Some(ExecutionStatus::Abandoned),
        _ => None,
    }
}

fn supervisor_instance_id(service_name: &str) -> String {
    format!("{}:pid:{}", service_name, process::id())
}

fn supervisor_run_id(service_name: &str) -> String {
    let timestamp = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros() * 1_000);
    format!("{}:{}:{}", service_name, process::id(), timestamp)
}

#[tokio::main]
async fn main() -> Result<()> {
    attune_common::auth::install_crypto_provider();

    let args = Args::parse();

    let config = if let Some(ref config_path) = args.config {
        Config::load_from_file(config_path)?
    } else {
        Config::load()?
    };
    config.validate()?;
    let tracing_init = observability::init_tracing_from_config(&config, args.log_level.as_deref())?;

    info!(
        level = %tracing_init.resolved.level_directive,
        level_source = tracing_init.resolved.level_source.as_str(),
        format = tracing_init.resolved.format.as_str(),
        initialized = tracing_init.initialized,
        "Tracing initialized"
    );
    info!("Starting Attune Supervisor Service");

    info!("Configuration loaded successfully");
    info!("Environment: {}", config.environment);
    info!("Database: {}", mask_connection_url(&config.database.url));

    if let Some(Command::MigrateStorage {
        rollback_snapshot_seconds,
    }) = args.command
    {
        let db = Database::new(&config.database).await?;
        let period = rollback_snapshot_seconds
            .unwrap_or(config.maintenance.storage_rollback_snapshot_seconds);
        if period == 0 {
            anyhow::bail!("rollback snapshot period must be greater than zero");
        }
        let report = storage_migration::migrate(
            db.pool(),
            blob_store_from_config(&config.storage)?,
            std::path::Path::new(&config.artifacts_dir),
            period,
        )
        .await?;
        info!(
            source_count = report.source_count,
            source_bytes = report.source_bytes,
            target_count = report.target_count,
            target_bytes = report.target_bytes,
            switched = report.switched,
            rollback_snapshot_seconds = period,
            "Storage migration completed"
        );
        return Ok(());
    }

    let service = SupervisorService::new(config).await?;
    let service_for_shutdown = service.clone();

    tokio::spawn(async move {
        let signal_name = match wait_for_shutdown_signal().await {
            Ok(signal_name) => signal_name,
            Err(err) => {
                error!("Failed to listen for shutdown signal: {}", err);
                return;
            }
        };

        info!(signal = signal_name, "Received shutdown signal");
        if let Err(err) = service_for_shutdown.shutdown().await {
            error!("Error during shutdown: {}", err);
        }
    });

    if let Err(err) = service.start().await {
        error!("Supervisor service error: {}", err);
        return Err(err);
    }

    info!("Attune Supervisor Service stopped");
    Ok(())
}

async fn wait_for_shutdown_signal() -> std::io::Result<&'static str> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result?;
                Ok("interrupt")
            }
            _ = terminate.recv() => Ok("terminate"),
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        Ok("interrupt")
    }
}

fn hex_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn mask_connection_url(url: &str) -> String {
    let Some((scheme, remainder)) = url.split_once("://") else {
        return "<redacted connection URL>".to_string();
    };
    if scheme.is_empty()
        || !scheme.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_alphabetic() || index > 0 && "+-.0123456789".contains(ch)
        })
    {
        return "<redacted connection URL>".to_string();
    }

    let suffix_start = remainder.find(['?', '#']).unwrap_or(remainder.len());
    let without_suffix = &remainder[..suffix_start];
    let authority_end = without_suffix.find('/').unwrap_or(without_suffix.len());
    let authority = &without_suffix[..authority_end];
    let path = &without_suffix[authority_end..];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);

    if host.is_empty()
        || host.contains('@')
        || host.chars().any(char::is_whitespace)
        || path.chars().any(char::is_whitespace)
    {
        return "<redacted connection URL>".to_string();
    }

    format!("{scheme}://{host}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        blob_store::{body_from_bytes, sha256, FilesystemBlobStore, ObjectKey},
        models::enums::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, ExecutionStatus,
            LogStreamBackend, OwnerType, RetentionPolicyType,
        },
        repositories::{
            artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
            execution::{CreateExecutionInput, ExecutionRepository},
            log_stream::LogStreamRepository,
            object_maintenance::ObjectMaintenanceRepository,
            retention::RetentionTarget,
            Create,
        },
        test_database::TestDatabase,
    };

    #[test]
    fn connection_url_log_value_removes_credentials_query_and_fragment() {
        let url = "postgresql://user:password@localhost:5432/db?sslmode=require#secret";
        assert_eq!(mask_connection_url(url), "postgresql://localhost:5432/db");
    }

    #[test]
    fn connection_url_log_value_keeps_safe_host_and_path() {
        let url = "postgresql://localhost:5432/db";
        assert_eq!(mask_connection_url(url), url);
    }

    #[test]
    fn connection_url_log_value_fails_closed() {
        assert_eq!(
            mask_connection_url("not a connection URL?password=secret"),
            "<redacted connection URL>"
        );
    }

    #[test]
    fn supervisor_cycle_reason_labels_boot_recovery() {
        assert_eq!(
            SupervisorCycleReason::StartupRecovery.log_label(),
            "startup_recovery"
        );
        assert_eq!(
            SupervisorCycleReason::DirtyShutdownRecovery.log_label(),
            "dirty_shutdown_recovery"
        );
        assert_eq!(SupervisorCycleReason::Scheduled.log_label(), "scheduled");
    }

    #[test]
    fn supervisor_run_identifiers_include_service_name() {
        assert!(supervisor_instance_id("attune-supervisor").contains("attune-supervisor"));
        assert!(supervisor_run_id("attune-supervisor").contains("attune-supervisor"));
    }

    #[tokio::test]
    async fn cache_partial_failure_audit_keeps_committed_progress_and_failure_outcome() {
        use attune_common::repositories::cache::{
            CacheGenerationRepository, CacheNamespacePolicy, CacheNamespaceRepository,
            CacheOwnerScope, CreateCacheGenerationInput, CreateCacheGenerationResult,
            CreateCacheNamespaceInput,
        };
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let artifacts = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        config.artifacts_dir = artifacts.path().to_string_lossy().into_owned();
        let namespace = CacheNamespaceRepository::create(
            database.pool(),
            CreateCacheNamespaceInput {
                owner: CacheOwnerScope::system(),
                namespace: "owned_failure_audit".to_string(),
                policy: CacheNamespacePolicy::default(),
            },
        )
        .await
        .unwrap();
        let generation = CacheGenerationRepository::create_or_get(
            database.pool(),
            &CreateCacheGenerationInput {
                namespace: namespace.id,
                client_refresh_id: "owned_failure_audit".to_string(),
                expected_active_generation: None,
                expected_chunk_count: 0,
                expected_count: Some(0),
                expected_bytes: None,
                checksum_algorithm: None,
                checksum: None,
                source_revision: None,
                created_by: None,
                created_by_execution: None,
            },
        )
        .await
        .unwrap();
        let CreateCacheGenerationResult::Created(generation) = generation else {
            panic!("expected fresh generation");
        };
        CacheGenerationRepository::fail(database.pool(), generation.id, "owned cleanup fixture")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE FUNCTION test_reject_cache_stats() RETURNS TRIGGER LANGUAGE plpgsql AS $$
             BEGIN
               IF NEW.completed_revision IS DISTINCT FROM OLD.completed_revision THEN
                 RAISE EXCEPTION 'owned statistics failure';
               END IF;
               RETURN NEW;
             END $$;
             CREATE TRIGGER test_reject_cache_stats BEFORE UPDATE ON cache_entry_statistics_state
               FOR EACH ROW EXECUTE FUNCTION test_reject_cache_stats();",
        )
        .execute(database.pool())
        .await
        .unwrap();
        let (shutdown_tx, _) = broadcast::channel(1);
        let service = SupervisorService {
            inner: Arc::new(SupervisorServiceInner {
                pool: database.pool().clone(),
                config,
                artifact_transport: Arc::new(VolumeTransport::new(
                    artifacts.path().to_str().unwrap(),
                )),
                blob_store: Arc::new(FilesystemBlobStore::new(objects.path()).unwrap()),
                publisher: None,
                _mq_connection: None,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        };
        service
            .run_cache_retention_step(&CacheRetentionConfig::default())
            .await;
        let audit: serde_json::Value = sqlx::query_scalar(
            "SELECT jsonb_build_object('outcome',outcome,'details',details) FROM audit_event
             WHERE resource_ref='cache_cleanup_cycle_partial_failure'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(audit["outcome"], json!("failure"));
        assert_eq!(audit["details"]["details"]["generations_deleted"], json!(1));
        assert_eq!(audit["details"]["details"]["statistics_failures"], json!(1));
        assert_eq!(
            audit["details"]["details"]["partitions_dropped_total"],
            json!(1)
        );
        assert_eq!(
            audit["details"]["details"]["statistics_pending"],
            json!(true)
        );
        service.inner.pool.close().await;
        drop(service);
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn native_scheduler_keeps_hourly_retention_independent_and_resumes_bounded_jobs() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let artifacts = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        config.artifacts_dir = artifacts.path().to_string_lossy().into_owned();
        config.maintenance.enabled = false;
        config.maintenance.monitoring_enabled = true;
        config.maintenance.alert_limit_per_cycle = 1;
        let mut retention = RetentionConfig {
            enabled: false,
            check_interval_seconds: 3600,
            ..Default::default()
        };
        retention.cache_retention.enabled = false;
        retention
            .native_maintenance
            .max_partition_operations_per_cycle = 1;
        retention.native_maintenance.max_summary_buckets_per_cycle = 1;
        retention.native_maintenance.summary_bootstrap_hours = 1;
        retention.native_maintenance.partition_lookahead_days = 1;
        retention.native_maintenance.default_repair_row_limit = 2;
        // This checks cadence and operation caps, not elapsed-time deadlines.
        // Concurrent physical clones can stall DDL at shared checkpoints.
        retention.native_maintenance.operation_timeout_milliseconds = 10_000;
        retention
            .native_maintenance
            .max_partition_cycle_milliseconds = 30_000;
        RetentionRepository::update_config(database.pool(), &retention)
            .await
            .unwrap();
        let now = DateTime::parse_from_rfc3339("2040-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        sqlx::query("UPDATE native_maintenance_schedule SET next_due = $1, last_success = NULL")
            .bind(now)
            .execute(database.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO notification (channel, entity_type, entity, activity, content, created)
                     VALUES ('scheduler', 'execution', '1', 'completed', '{}', '2020-01-01T00:00:00Z')",
        )
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO trigger (ref, label) VALUES ('core.alert', 'Core Alert') ON CONFLICT (ref) DO NOTHING")
            .execute(database.pool()).await.unwrap();
        sqlx::query("INSERT INTO event (trigger_ref, payload, created)
                     SELECT 'scheduler.default', '{\"body\":\"private-native-source-body\"}', '2039-12-31T00:00:00Z'
                     FROM generate_series(1, 5)")
            .execute(database.pool()).await.unwrap();
        let (shutdown_tx, _) = broadcast::channel(1);
        let service = SupervisorService {
            inner: Arc::new(SupervisorServiceInner {
                pool: database.pool().clone(),
                config,
                artifact_transport: Arc::new(VolumeTransport::new(
                    artifacts.path().to_str().unwrap(),
                )),
                blob_store: Arc::new(FilesystemBlobStore::new(objects.path()).unwrap()),
                publisher: None,
                _mq_connection: None,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        };
        service
            .run_retention_cycle_at(SupervisorCycleReason::StartupRecovery, now)
            .await
            .unwrap();
        let first = ScheduleRepository::status(database.pool()).await.unwrap();
        let native_audits: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object('job', resource_ref, 'outcome', outcome, 'details', details)
             FROM audit_event WHERE event_type = 'maintenance.native.job_completed'",
        )
        .fetch_all(database.pool())
        .await
        .unwrap();
        for row in &first {
            assert_eq!(
                row.last_success,
                Some(now),
                "job {:?} did not get a bounded turn: {native_audits:?}",
                row.job
            );
        }
        let native_attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE event_type = 'maintenance.native.job_completed'")
            .fetch_one(database.pool()).await.unwrap();
        assert_eq!(native_attempts, 2);
        let backlog_alert: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM event WHERE trigger_ref = 'core.alert'
            AND payload->>'correlation_id' = 'supervisor:native:partition_backlog:event'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(backlog_alert["details"]["default_rows_at_least"], json!(3));
        assert_eq!(
            backlog_alert["details"]["default_count_exact"],
            json!(false)
        );
        assert!(
            backlog_alert["details"]["missing_future_partitions"]
                .as_i64()
                .unwrap()
                > 0
        );
        assert!(!backlog_alert
            .to_string()
            .contains("private-native-source-body"));
        // A five-minute summary tick must leave both hourly due times alone.
        let next = now + ChronoDuration::minutes(5);
        service
            .run_retention_cycle_at(SupervisorCycleReason::Scheduled, next)
            .await
            .unwrap();
        let second = ScheduleRepository::status(database.pool()).await.unwrap();
        let row = |job| second.iter().find(|row| row.job == job).unwrap();
        assert_eq!(row(MaintenanceJob::Retention).last_success, Some(now));
        assert_eq!(row(MaintenanceJob::Partition).last_success, Some(now));
        assert_eq!(row(MaintenanceJob::Summary).last_success, Some(next));
        assert_eq!(
            row(MaintenanceJob::Retention).next_due,
            now + ChronoDuration::hours(1)
        );
        let notifications: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notification WHERE channel = 'scheduler'")
                .fetch_one(database.pool())
                .await
                .unwrap();
        assert_eq!(
            notifications, 1,
            "disabled retention must preserve expired rows while native jobs run"
        );
        // Enabling retention between ticks must still respect its hourly due
        // time. Native bootstrap must not accidentally run its deletion loop.
        retention.enabled = true;
        retention.targets.notifications.max_age_seconds = Some(1);
        RetentionRepository::update_config(database.pool(), &retention)
            .await
            .unwrap();
        // A replacement leader detects the dirty prior run. It reconciles
        // partitions at startup, but still must not replay retention early.
        *service.inner.run_id.lock().await = None;
        service
            .run_retention_cycle_at(
                SupervisorCycleReason::Scheduled,
                next + ChronoDuration::minutes(5),
            )
            .await
            .unwrap();
        let notifications: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notification WHERE channel = 'scheduler'")
                .fetch_one(database.pool())
                .await
                .unwrap();
        assert_eq!(notifications, 1);
        let replacement = ScheduleRepository::status(database.pool()).await.unwrap();
        assert_eq!(
            replacement
                .iter()
                .find(|row| row.job == MaintenanceJob::Retention)
                .unwrap()
                .last_success,
            Some(now)
        );
        assert_eq!(
            replacement
                .iter()
                .find(|row| row.job == MaintenanceJob::Partition)
                .unwrap()
                .last_success,
            Some(next + ChronoDuration::minutes(5))
        );
        // A whole expired leaf must produce partition units, never a fabricated
        // deleted-row count. Its three source rows disappear atomically.
        let expired_day = attune_common::repositories::native_maintenance::partitions::utc_day(
            Utc::now() - ChronoDuration::days(60),
        );
        attune_common::repositories::native_maintenance::partitions::PartitionRepository::ensure_day(
            database.pool(),
            attune_common::repositories::native_maintenance::ManagedTable::Event,
            expired_day,
            &retention.native_maintenance,
        ).await.unwrap();
        sqlx::query("INSERT INTO event (trigger_ref, created) SELECT 'scheduler.drop', $1 FROM generate_series(1, 3)")
            .bind(expired_day).execute(database.pool()).await.unwrap();
        service
            .mark_supervisor_run_clean("test_leader_handoff")
            .await;
        *service.inner.run_id.lock().await = None;
        service
            .run_retention_cycle_at(
                SupervisorCycleReason::Scheduled,
                now + ChronoDuration::hours(1),
            )
            .await
            .unwrap();
        let notifications: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notification WHERE channel = 'scheduler'")
                .fetch_one(database.pool())
                .await
                .unwrap();
        assert_eq!(notifications, 0);
        let clean_replacement = ScheduleRepository::status(database.pool()).await.unwrap();
        assert_eq!(clean_replacement.iter().find(|row| row.job == MaintenanceJob::Partition).unwrap().last_success,
            Some(now + ChronoDuration::hours(1)), "the first owned turn after a clean handoff must reconcile partitions even before their due time");
        let expiry_audit: serde_json::Value = sqlx::query_scalar("SELECT details FROM audit_event
            WHERE event_type = 'maintenance.retention.target_completed' AND resource_ref = 'events'")
            .fetch_one(database.pool()).await.unwrap();
        assert_eq!(expiry_audit["rows_deleted"], json!(0));
        assert_eq!(expiry_audit["deleted"], json!(0));
        assert_eq!(expiry_audit["partitions_dropped"], json!(1));
        assert_eq!(expiry_audit["partition_candidates"], json!(1));
        assert_eq!(expiry_audit["candidates_exact"], json!(true));
        let backlog_alerts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM event WHERE trigger_ref = 'core.alert'
            AND payload->>'correlation_id' = 'supervisor:native:partition_backlog:event'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            backlog_alerts, 1,
            "native alerts must respect the existing cooldown"
        );
        let default_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM event WHERE trigger_ref = 'scheduler.default'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            default_rows, 5,
            "native catch-up must preserve over-budget DEFAULT data"
        );
        drop(service);
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn native_scheduler_retries_builder_failures_without_leaking_source_details() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let artifacts = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        config.artifacts_dir = artifacts.path().to_string_lossy().into_owned();
        config.maintenance.enabled = false;
        config.maintenance.monitoring_enabled = false;
        let mut retention = RetentionConfig {
            enabled: false,
            ..Default::default()
        };
        retention.cache_retention.enabled = false;
        retention
            .native_maintenance
            .max_partition_operations_per_cycle = 1;
        retention.native_maintenance.max_summary_buckets_per_cycle = 4;
        retention.native_maintenance.summary_bootstrap_hours = 1;
        retention.native_maintenance.partition_lookahead_days = 1;
        // Leave time for shared checkpoints without changing the one-operation cap.
        retention.native_maintenance.operation_timeout_milliseconds = 10_000;
        retention
            .native_maintenance
            .max_partition_cycle_milliseconds = 30_000;
        RetentionRepository::update_config(database.pool(), &retention)
            .await
            .unwrap();
        let now = DateTime::parse_from_rfc3339("2040-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        sqlx::query("UPDATE native_maintenance_schedule SET next_due = $1, last_success = NULL")
            .bind(now)
            .execute(database.pool())
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE FUNCTION reject_native_coverage() RETURNS trigger LANGUAGE plpgsql AS $$
                         BEGIN RAISE EXCEPTION 'private source contents'; END; $$;
                       CREATE TRIGGER reject_native_coverage BEFORE INSERT ON native_summary_hour
                         FOR EACH ROW EXECUTE FUNCTION reject_native_coverage();",
        )
        .execute(database.pool())
        .await
        .unwrap();
        let (shutdown_tx, _) = broadcast::channel(1);
        let service = SupervisorService {
            inner: Arc::new(SupervisorServiceInner {
                pool: database.pool().clone(),
                artifact_transport: Arc::new(VolumeTransport::new(&config.artifacts_dir)),
                blob_store: Arc::new(FilesystemBlobStore::new(objects.path()).unwrap()),
                config,
                publisher: None,
                _mq_connection: None,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        };
        service
            .run_retention_cycle_at(SupervisorCycleReason::StartupRecovery, now)
            .await
            .unwrap();
        let statuses = ScheduleRepository::status(database.pool()).await.unwrap();
        let summary = statuses
            .iter()
            .find(|row| row.job == MaintenanceJob::Summary)
            .unwrap();
        assert_eq!(summary.last_success, None);
        assert_eq!(summary.next_due, now + ChronoDuration::minutes(1));
        let partition_audit: serde_json::Value = sqlx::query_scalar(
            "SELECT details FROM audit_event
             WHERE event_type = 'maintenance.native.job_completed' AND resource_ref = 'partition'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(
            statuses
                .iter()
                .find(|row| row.job == MaintenanceJob::Partition)
                .unwrap()
                .last_success,
            Some(now),
            "partition attempt: {partition_audit}"
        );
        let details: serde_json::Value = sqlx::query_scalar("SELECT details FROM audit_event
            WHERE event_type = 'maintenance.native.job_completed' AND resource_ref = 'summary' AND outcome = 'failure'")
            .fetch_one(database.pool()).await.unwrap();
        assert!(details["failure_count"].as_u64().unwrap() > 0);
        assert_eq!(details["buckets_processed"], json!(0));
        assert!(!details.to_string().contains("private source contents"));
        service
            .run_retention_cycle_at(
                SupervisorCycleReason::Scheduled,
                now + ChronoDuration::seconds(30),
            )
            .await
            .unwrap();
        let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE event_type = 'maintenance.native.job_completed' AND resource_ref = 'summary'")
            .fetch_one(database.pool()).await.unwrap();
        assert_eq!(
            attempts, 1,
            "failed jobs must wait for the bounded retry deadline"
        );
        service
            .run_retention_cycle_at(
                SupervisorCycleReason::Scheduled,
                now + ChronoDuration::minutes(1),
            )
            .await
            .unwrap();
        let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE event_type = 'maintenance.native.job_completed' AND resource_ref = 'summary'")
            .fetch_one(database.pool()).await.unwrap();
        assert_eq!(attempts, 2);
        // An invalid persisted native budget must stop native calls at the cycle
        // boundary rather than reaching an unchecked builder/cadence division.
        retention.native_maintenance.summary_interval_seconds = 0;
        RetentionRepository::update_config(database.pool(), &retention)
            .await
            .unwrap();
        service
            .run_retention_cycle_at(
                SupervisorCycleReason::Scheduled,
                now + ChronoDuration::minutes(2),
            )
            .await
            .unwrap();
        let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE event_type = 'maintenance.native.job_completed' AND resource_ref = 'summary'")
            .fetch_one(database.pool()).await.unwrap();
        assert_eq!(attempts, 2);
        retention.native_maintenance.summary_interval_seconds = 300;
        RetentionRepository::update_config(database.pool(), &retention)
            .await
            .unwrap();
        service.shutdown().await.unwrap();
        service
            .run_retention_cycle_at(
                SupervisorCycleReason::Scheduled,
                now + ChronoDuration::minutes(3),
            )
            .await
            .unwrap();
        let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE event_type = 'maintenance.native.job_completed' AND resource_ref = 'summary'")
            .fetch_one(database.pool()).await.unwrap();
        assert_eq!(
            attempts, 2,
            "a late subscription must not miss an already requested shutdown"
        );
        drop(service);
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn retention_budget_allows_later_targets_and_lag_monitoring_under_leadership() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let artifacts = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        config.artifacts_dir = artifacts.path().to_string_lossy().into_owned();
        config.maintenance.artifact_cleanup_enabled = false;
        config.maintenance.pack_release_retention_enabled = false;
        config.maintenance.corrective_actions_enabled = false;
        config.maintenance.retention_lag_alert_seconds = 1;
        let mut retention = RetentionConfig {
            batch_size: 1,
            max_batches_per_target: 2,
            ..RetentionConfig::default()
        };
        retention.cache_retention.enabled = false;
        let targets: serde_json::Map<String, serde_json::Value> = RetentionTarget::all()
            .into_iter()
            .map(|target| (target.name().to_string(), json!({"max_age_seconds": null})))
            .collect();
        retention.targets = serde_json::from_value(json!(targets)).unwrap();
        // Keep the old event cohort in a partially expired daily partition.
        // Whole-leaf expiry would bypass the row-batch budget by design.
        retention.targets.events.max_age_seconds =
            Some(Utc::now().timestamp().rem_euclid(86400) as u64 + 43200);
        retention.targets.notifications.max_age_seconds = Some(86400);
        RetentionRepository::update_config(database.pool(), &retention)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO trigger (ref, label) VALUES ('core.alert', 'Core Alert')
             ON CONFLICT (ref) DO NOTHING",
        )
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO event (trigger_ref, created)
             SELECT 'retention.backlog', date_trunc('day', NOW(), 'UTC') - INTERVAL '1 day' FROM generate_series(1, 5)",
        )
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO notification (channel, entity_type, entity, activity, content, created)
             VALUES ('test', 'execution', '1', 'completed', '{}', NOW() - INTERVAL '2 days')",
        )
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO execution (action_ref, status, config, created, updated)
             VALUES ('retention.forever', 'completed', '{}',
                     NOW() - INTERVAL '2 days', NOW() - INTERVAL '2 days')",
        )
        .execute(database.pool())
        .await
        .unwrap();
        let (shutdown_tx, _) = broadcast::channel(1);
        let service = SupervisorService {
            inner: Arc::new(SupervisorServiceInner {
                pool: database.pool().clone(),
                artifact_transport: Arc::new(VolumeTransport::new(&config.artifacts_dir)),
                blob_store: Arc::new(FilesystemBlobStore::new(objects.path()).unwrap()),
                config,
                publisher: None,
                _mq_connection: None,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        };

        let mut leader = database.pool().acquire().await.unwrap();
        assert!(
            RetentionRepository::try_advisory_lock(&mut leader, retention.advisory_lock_key)
                .await
                .unwrap()
        );
        service
            .run_retention_cycle(SupervisorCycleReason::Scheduled)
            .await
            .unwrap();
        let unchanged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification")
            .fetch_one(database.pool())
            .await
            .unwrap();
        assert_eq!(unchanged, 1, "another leader must prevent cleanup");
        RetentionRepository::advisory_unlock(&mut leader, retention.advisory_lock_key)
            .await
            .unwrap();

        service
            .run_retention_cycle(SupervisorCycleReason::Scheduled)
            .await
            .unwrap();
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM event WHERE trigger_ref = 'retention.backlog'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(remaining, 3);
        let notifications: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification")
            .fetch_one(database.pool())
            .await
            .unwrap();
        assert_eq!(notifications, 0, "later targets must run after the budget");
        let executions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution")
            .fetch_one(database.pool())
            .await
            .unwrap();
        assert_eq!(
            executions, 1,
            "unlimited retention must preserve expired rows"
        );
        let alerts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM event WHERE trigger_ref = 'core.alert'
               AND payload->>'correlation_id' = 'supervisor:retention-lag:events'
               AND (payload->'details'->>'count')::BIGINT = 3",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(alerts, 1, "non-retention maintenance must get a turn");
        let details: serde_json::Value = sqlx::query_scalar(
            "SELECT details FROM audit_event
             WHERE event_type = 'maintenance.retention.target_completed' AND resource_ref = 'events'",
        ).fetch_one(database.pool()).await.unwrap();
        assert_eq!(details["candidates"], json!(3));
        assert_eq!(details["candidates_exact"], json!(false));
        assert_eq!(details["deleted"], json!(2));
        assert_eq!(details["rows_deleted"], json!(2));
        assert_eq!(details["partitions_dropped"], json!(0));
        assert_eq!(details["max_batches_per_target"], json!(2));
        assert!(
            RetentionRepository::try_advisory_lock(&mut leader, retention.advisory_lock_key)
                .await
                .unwrap(),
            "the cycle must release leadership"
        );
        RetentionRepository::advisory_unlock(&mut leader, retention.advisory_lock_key)
            .await
            .unwrap();
        drop(leader);
        drop(service);
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn retention_failure_audit_preserves_prior_committed_batches() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let artifacts = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        config.artifacts_dir = artifacts.path().to_string_lossy().into_owned();
        config.maintenance.enabled = false;
        let mut retention = RetentionConfig {
            batch_size: 2,
            max_batches_per_target: 100,
            ..RetentionConfig::default()
        };
        retention.cache_retention.enabled = false;
        let targets: serde_json::Map<String, serde_json::Value> = RetentionTarget::all()
            .into_iter()
            .map(|target| (target.name().to_string(), json!({"max_age_seconds": null})))
            .collect();
        retention.targets = serde_json::from_value(json!(targets)).unwrap();
        retention.targets.worker_history.max_age_seconds = Some(86400);
        RetentionRepository::update_config(database.pool(), &retention)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO worker_history (time, operation, entity_id, entity_ref)
             SELECT NOW() - INTERVAL '2 days' + n * INTERVAL '1 second',
                    'UPDATE', n, 'retention.failure' FROM generate_series(1, 5) n",
        )
        .execute(database.pool())
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE FUNCTION retention_reject_later_batch() RETURNS trigger
             LANGUAGE plpgsql AS $$
             BEGIN
                 IF OLD.entity_id = 3 THEN
                     RAISE EXCEPTION 'forced second retention batch failure';
                 END IF;
                 RETURN OLD;
             END;
             $$;
             CREATE TRIGGER retention_reject_later_batch
             BEFORE DELETE ON worker_history FOR EACH ROW
             EXECUTE FUNCTION retention_reject_later_batch();",
        )
        .execute(database.pool())
        .await
        .unwrap();
        let (shutdown_tx, _) = broadcast::channel(1);
        let service = SupervisorService {
            inner: Arc::new(SupervisorServiceInner {
                pool: database.pool().clone(),
                artifact_transport: Arc::new(VolumeTransport::new(&config.artifacts_dir)),
                blob_store: Arc::new(FilesystemBlobStore::new(objects.path()).unwrap()),
                config,
                publisher: None,
                _mq_connection: None,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        };
        let earliest_cutoff = Utc::now() - ChronoDuration::days(1);
        service
            .run_retention_cycle(SupervisorCycleReason::Scheduled)
            .await
            .unwrap();
        let latest_cutoff = Utc::now() - ChronoDuration::days(1);
        let remaining: Vec<i64> =
            sqlx::query_scalar("SELECT entity_id FROM worker_history ORDER BY entity_id")
                .fetch_all(database.pool())
                .await
                .unwrap();
        let details: serde_json::Value = sqlx::query_scalar(
            "SELECT details FROM audit_event
             WHERE event_type = 'maintenance.retention.target_failed'
               AND resource_ref = 'worker_history' AND outcome = 'failure'",
        )
        .fetch_one(database.pool())
        .await
        .unwrap();
        drop(service);
        database.cleanup().await.unwrap();

        assert_eq!(remaining, vec![3, 4, 5]);
        assert_eq!(details["deleted"], json!(2));
        assert_eq!(details["candidates"], json!(5));
        assert_eq!(details["deleted_count_complete"], json!(false));
        assert_eq!(details["batch_size"], json!(2));
        assert_eq!(details["max_batches_per_target"], json!(100));
        let cutoff = chrono::DateTime::parse_from_rfc3339(details["cutoff"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        assert!(cutoff >= earliest_cutoff && cutoff <= latest_cutoff);
        assert!(details["error"]
            .as_str()
            .unwrap()
            .contains("forced second retention batch failure"));
    }

    #[tokio::test]
    async fn supervisor_reconciles_abandoned_and_missing_objects_with_a_delete_delay() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let artifacts = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        config.artifacts_dir = artifacts.path().to_string_lossy().into_owned();
        config.maintenance.object_upload_abandon_seconds = 3600;
        config.maintenance.object_delete_grace_seconds = 3600;
        let blob_store: Arc<dyn BlobStore> =
            Arc::new(FilesystemBlobStore::new(objects.path()).unwrap());
        let (shutdown_tx, _) = broadcast::channel(1);
        let service = SupervisorService {
            inner: Arc::new(SupervisorServiceInner {
                pool: database.pool().clone(),
                config: config.clone(),
                artifact_transport: Arc::new(VolumeTransport::new(&config.artifacts_dir)),
                blob_store: blob_store.clone(),
                publisher: None,
                _mq_connection: None,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        };

        let create_artifact = |suffix: &str| CreateArtifactInput {
            r#ref: format!("reconcile_{}_{}", suffix, uuid::Uuid::new_v4().simple()),
            scope: OwnerType::System,
            owner: "supervisor-test".to_string(),
            r#type: ArtifactType::FileBinary,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: None,
            description: None,
            content_type: None,
            data: None,
        };
        let abandoned_artifact =
            ArtifactRepository::create(&*database, create_artifact("abandoned"))
                .await
                .unwrap();
        let abandoned = ArtifactVersionRepository::create_object_pending(
            &database,
            abandoned_artifact.id,
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
        .bind(abandoned.id)
        .execute(&*database)
        .await
        .unwrap();

        let missing_artifact = ArtifactRepository::create(&*database, create_artifact("missing"))
            .await
            .unwrap();
        let missing = ArtifactVersionRepository::create_object_pending(
            &*database,
            missing_artifact.id,
            None,
            "application/octet-stream".to_string(),
            None,
            None,
        )
        .await
        .unwrap();
        ArtifactVersionRepository::mark_body_ready(
            &database,
            missing.id,
            "e:missing-version",
            4,
            &"a".repeat(64),
        )
        .await
        .unwrap()
        .unwrap();

        let log_artifact = ArtifactRepository::create(&*database, create_artifact("log"))
            .await
            .unwrap();
        let terminal_execution = ExecutionRepository::create(
            &database,
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
        let log_version = ArtifactVersionRepository::create_log_pending(
            &*database,
            log_artifact.id,
            &log_artifact.r#ref,
            LogStreamBackend::ObjectSegments,
            "text/plain".to_string(),
            Some(terminal_execution.id),
            None,
            None,
        )
        .await
        .unwrap();
        let stream = LogStreamRepository::create(&database, log_version.id, 1024, 1000)
            .await
            .unwrap();
        let segment_bytes = bytes::Bytes::from_static(b"log bytes");
        let segment_digest = sha256(&segment_bytes);
        let segment_key = ObjectKey::new(format!("logs/{}/segments/0", stream.id)).unwrap();
        ObjectMaintenanceRepository::reserve_upload(&database, segment_key.as_str(), "log")
            .await
            .unwrap();
        let stored_segment = blob_store
            .put(
                &segment_key,
                body_from_bytes(segment_bytes.clone()),
                segment_digest,
            )
            .await
            .unwrap();
        ObjectMaintenanceRepository::record_uploaded(
            &database,
            segment_key.as_str(),
            stored_segment.provider_version.as_stored(),
            stored_segment.size as i64,
        )
        .await
        .unwrap();
        assert!(!stored_segment.provider_version.as_stored().is_empty());
        sqlx::query(
            "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
        )
        .bind(log_version.id)
        .execute(&*database)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE object_maintenance_ledger SET updated = NOW() - INTERVAL '2 hours' WHERE object_key = $1",
        )
        .bind(segment_key.as_str())
        .execute(&*database)
        .await
        .unwrap();

        service
            .run_object_maintenance(&config.maintenance)
            .await
            .unwrap();
        assert!(
            ArtifactVersionRepository::find_by_id(&*database, abandoned.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            ArtifactVersionRepository::find_by_id(&*database, missing.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            ArtifactVersionRepository::find_by_id(&*database, log_version.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(blob_store.head(&segment_key).await.unwrap().is_some());

        sqlx::query(
            "UPDATE object_maintenance_ledger SET eligible_at = NOW() - INTERVAL '2 hours' WHERE object_key = $1",
        )
        .bind(segment_key.as_str())
        .execute(&*database)
        .await
        .unwrap();
        service
            .run_object_maintenance(&config.maintenance)
            .await
            .unwrap();
        assert!(blob_store.head(&segment_key).await.unwrap().is_none());
    }
}
