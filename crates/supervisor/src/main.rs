//! Attune Supervisor Service
//!
//! Owns platform maintenance loops such as runtime database retention.

mod cache_retention;

use std::{process, sync::Arc, time::Duration};

use anyhow::Result;
use attune_common::{
    artifact_transport::{ArtifactFileTransport, VolumeTransport},
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome, AuditRepository},
    config::{CacheRetentionConfig, Config, RetentionConfig, SupervisorMaintenanceConfig},
    db::Database,
    models::{enums::ExecutionStatus, Execution},
    mq::{
        Connection as MqConnection, ExecutionCompletedPayload, ExecutionRequestedPayload,
        MessageEnvelope, MessageQueueConfig, MessageType, Publisher, PublisherConfig,
    },
    observability,
    repositories::{
        execution::{ExecutionRepository, UpdateExecutionInput},
        maintenance::{
            AdmissionRemediationResult, ArtifactCleanupResult, ExecutionRescheduleAttempt,
            MaintenanceRepository, QueueRemediationResult, StaleExecutionCandidate,
            WorkflowRemediationResult,
        },
        retention::{RetentionRepository, RetentionTarget, RetentionTargetResult},
        workflow_cache_iteration::{
            StaleSyntheticCacheIterationCompletion, WorkflowCacheIterationRepository,
        },
        FindById,
    },
    system_alert::{emit_core_alert, SystemAlert},
};
use chrono::{Duration as ChronoDuration, Utc};
use clap::Parser;
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
}

#[derive(Clone)]
struct SupervisorService {
    inner: Arc<SupervisorServiceInner>,
}

struct SupervisorServiceInner {
    pool: PgPool,
    config: Config,
    artifact_transport: Arc<dyn ArtifactFileTransport>,
    publisher: Option<Arc<Publisher>>,
    _mq_connection: Option<MqConnection>,
    run_id: Mutex<Option<String>>,
    cache_retention_state: Arc<cache_retention::CacheRetentionState>,
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
        let (mq_connection, publisher) = Self::initialize_publisher(&config).await?;
        let (shutdown_tx, _) = broadcast::channel(1);

        Ok(Self {
            inner: Arc::new(SupervisorServiceInner {
                pool: db.pool().clone(),
                artifact_transport: Arc::new(VolumeTransport::new(&config.artifacts_dir)),
                publisher,
                _mq_connection: mq_connection,
                config,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(cache_retention::CacheRetentionState::default()),
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
        let mut interval = Duration::from_secs(self.inner.config.retention.check_interval_seconds);

        info!(
            fallback_check_interval_seconds = self.inner.config.retention.check_interval_seconds,
            "Supervisor retention loop started; runtime settings are loaded from the database each cycle"
        );

        let mut cycle_reason = SupervisorCycleReason::StartupRecovery;
        loop {
            match self.run_retention_cycle(cycle_reason).await {
                Ok(next_interval) => {
                    interval = next_interval;
                }
                Err(err) => {
                    error!("Retention cycle failed: {}", err);
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
        let _ = self.inner.shutdown_tx.send(());
        Ok(())
    }

    async fn run_retention_cycle(&self, cycle_reason: SupervisorCycleReason) -> Result<Duration> {
        let retention = RetentionRepository::load_config(&self.inner.pool).await?;
        let interval = Duration::from_secs(retention.check_interval_seconds);

        let mut conn = self.inner.pool.acquire().await?;

        if !RetentionRepository::try_advisory_lock(&mut conn, retention.advisory_lock_key).await? {
            info!(
                advisory_lock_key = retention.advisory_lock_key,
                "Another supervisor owns the retention lock; skipping cycle"
            );
            return Ok(interval);
        }

        let cycle_result = async {
            let cycle_reason = self.ensure_supervisor_run(cycle_reason).await?;
            info!(
                cycle_reason = cycle_reason.log_label(),
                check_interval_seconds = retention.check_interval_seconds,
                "Starting supervisor maintenance cycle"
            );

            if retention.enabled {
                let targets = RetentionRepository::configured_targets(&retention.targets);
                info!(
                    target_count = targets.len(),
                    batch_size = retention.batch_size,
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

                    match RetentionRepository::run_target(
                        &self.inner.pool,
                        target.target,
                        max_age_seconds,
                        retention.batch_size,
                        retention.dry_run,
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
                            warn!(
                                target = target.target.name(),
                                error = %err,
                                "Retention target failed"
                            );
                            if let Err(audit_err) = self
                                .audit_retention_target_failed(
                                    target.target,
                                    max_age_seconds,
                                    &retention,
                                    err.to_string(),
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
                }
            } else {
                info!(
                    check_interval_seconds = retention.check_interval_seconds,
                    "Runtime retention is disabled in database config; running non-retention maintenance only"
                );
            }

            self.run_cache_retention_step(&retention.cache_retention)
                .await;

            self.run_maintenance_cycle(&retention).await;

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
            Ok(requested_reason)
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
        if result.candidates == 0 && result.deleted == 0 && !result.dry_run {
            return Ok(());
        }

        let details = json!({
            "target": result.target.name(),
            "cutoff": result.cutoff.map(|cutoff| cutoff.to_rfc3339()),
            "max_age_seconds": max_age_seconds,
            "candidates": result.candidates,
            "deleted": result.deleted,
            "dry_run": result.dry_run,
            "retention_enabled": retention.enabled,
            "batch_size": retention.batch_size,
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
        target: RetentionTarget,
        max_age_seconds: u64,
        retention: &attune_common::config::RetentionConfig,
        error: String,
    ) -> Result<()> {
        let details = json!({
            "target": target.name(),
            "max_age_seconds": max_age_seconds,
            "dry_run": retention.dry_run,
            "batch_size": retention.batch_size,
            "advisory_lock_key": retention.advisory_lock_key,
            "service_name": self.inner.config.service_name,
            "environment": self.inner.config.environment,
            "error": error,
        });

        let event = AuditEventBuilder::new(
            AuditCategory::Admin,
            event_type::maintenance::RETENTION_TARGET_FAILED,
            AuditOutcome::Failure,
        )
        .actor_login("attune-supervisor")
        .actor_token_type("system")
        .resource("runtime_retention")
        .resource_ref(target.name())
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
                        .audit_corrective_action(
                            "cache_retention",
                            "cache_cleanup_cycle_completed",
                            json!({
                                "dry_run": summary.dry_run,
                                "namespaces_scanned": summary.namespaces_scanned,
                                "staging_expired": summary.staging_expired,
                                "cleanup_candidates": summary.cleanup_candidates,
                                "entries_deleted": summary.entries_deleted,
                                "generations_deleted": summary.generations_deleted,
                                "namespaces_deleted": summary.namespaces_deleted,
                                "freshness_alerts": summary.freshness_alerts,
                                "staging_failure_alerts": summary.staging_failure_alerts,
                            }),
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
            match self.run_artifact_cleanup(maintenance).await {
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

    async fn run_artifact_cleanup(
        &self,
        maintenance: &SupervisorMaintenanceConfig,
    ) -> Result<ArtifactCleanupResult> {
        let candidates =
            MaintenanceRepository::expired_artifact_version_count(&self.inner.pool).await?;
        let versions = MaintenanceRepository::find_expired_artifact_versions(
            &self.inner.pool,
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
                self.inner.artifact_transport.delete_file(file_path).await?;
                result.deleted_files += 1;
            }

            if MaintenanceRepository::delete_artifact_version(&self.inner.pool, version.id).await? {
                result.deleted_versions += 1;
                if MaintenanceRepository::refresh_or_delete_artifact_metadata(
                    &self.inner.pool,
                    version.artifact,
                )
                .await?
                {
                    result.deleted_artifacts += 1;
                }
            }
        }

        Ok(result)
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
        let event = AuditEventBuilder::new(
            AuditCategory::Admin,
            event_type::maintenance::CORRECTIVE_ACTION_APPLIED,
            AuditOutcome::Success,
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
    info!("Database: {}", mask_password(&config.database.url));

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

fn mask_password(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            let mut masked = url.to_string();
            masked.replace_range(colon_pos + 1..at_pos, "****");
            return masked;
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_database_password() {
        let url = "postgresql://user:password@localhost:5432/db";
        assert_eq!(
            mask_password(url),
            "postgresql://user:****@localhost:5432/db"
        );
    }

    #[test]
    fn leaves_url_without_password_unchanged() {
        let url = "postgresql://localhost:5432/db";
        assert_eq!(mask_password(url), url);
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
}
