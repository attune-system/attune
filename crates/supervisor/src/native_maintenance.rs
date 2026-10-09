//! Native jobs share the existing supervisor leader session, but never share a
//! catch-up loop or a retention cadence. Each repository call owns its budget.

use std::time::Duration;

use attune_common::{
    audit::{AuditCategory, AuditEventBuilder, AuditOutcome, AuditRepository},
    config::RetentionConfig,
    repositories::native_maintenance::{
        partitions::{PartitionCycleFailure, PartitionRepository},
        schedule::{MaintenanceJob, ScheduleRepository},
        summaries::SummaryRepository,
    },
};
use chrono::{DateTime, Utc};
use serde_json::json;
use tracing::{info, warn};

use crate::{SupervisorAlertRequest, SupervisorCycleReason, SupervisorService};

/// Poll persisted settings at most once a minute, including when all work is
/// disabled. A short native cadence never changes the retention job's due time.
pub(super) fn poll_interval(config: &RetentionConfig) -> Duration {
    let mut seconds = config.check_interval_seconds.clamp(1, 60);
    if config.native_maintenance.enabled {
        seconds = seconds
            .min(config.native_maintenance.partition_interval_seconds.max(1))
            .min(config.native_maintenance.summary_interval_seconds.max(1));
    }
    Duration::from_secs(seconds)
}

impl SupervisorService {
    pub(super) async fn run_native_maintenance(
        &self,
        retention: &RetentionConfig,
        reason: SupervisorCycleReason,
        now: DateTime<Utc>,
        jobs: &[MaintenanceJob],
    ) {
        let config = &retention.native_maintenance;
        if !config.enabled {
            return;
        }
        let mut shutdown_rx = self.inner.shutdown_tx.subscribe();
        for &job in jobs {
            if self
                .inner
                .shutdown_requested
                .load(std::sync::atomic::Ordering::Acquire)
                || !matches!(
                    shutdown_rx.try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                )
            {
                return;
            }
            let interval = match job {
                MaintenanceJob::Partition => config.partition_interval_seconds,
                MaintenanceJob::Summary => config.summary_interval_seconds,
                MaintenanceJob::Retention => unreachable!(),
            };
            let force = job == MaintenanceJob::Partition
                && !matches!(reason, SupervisorCycleReason::Scheduled);
            match ScheduleRepository::begin_attempt(&self.inner.pool, job, now, interval, force)
                .await
            {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    warn!(job = job.name(), error = %error, "Failed to claim native maintenance schedule");
                    continue;
                }
            }
            let outcome = match job {
                MaintenanceJob::Partition => {
                    match PartitionRepository::reconcile(&self.inner.pool, config, now).await {
                        Ok(result) => {
                            let success = result.lock_retries == 0 && result.deferred_deadline == 0;
                            let details = json!({
                                "attempted": result.attempted, "created": result.created,
                                "rows_moved": result.rows_moved,
                                "deferred_over_budget": result.deferred_over_budget,
                                "lock_retries": result.lock_retries,
                                "deferred_deadline": result.deferred_deadline,
                                "budget_exhausted": result.budget_exhausted,
                            });
                            if !success || result.deferred_over_budget > 0 {
                                self.report_native_problem("partition_deferred", details.clone())
                                    .await;
                            }
                            Ok((success, details))
                        }
                        Err(failure) => Err(partition_failure_details(&failure)),
                    }
                }
                MaintenanceJob::Summary => {
                    // The repository's argument fixes this to CancellationToken.
                    // Infer the type here so the supervisor need not depend on
                    // the repository's cancellation implementation crate.
                    let cancellation = Default::default();
                    let refresh = SummaryRepository::refresh_cycle(
                        &self.inner.pool,
                        config,
                        &retention.targets,
                        now,
                        &cancellation,
                    );
                    tokio::pin!(refresh);
                    let result = tokio::select! {
                        result = &mut refresh => result,
                        _ = shutdown_rx.recv() => {
                            cancellation.cancel();
                            // Await rollback/known committed progress, never
                            // abandon a builder transaction on cancellation.
                            refresh.await
                        }
                    };
                    match result {
                        Ok(result) => {
                            let success = result.failures.is_empty() && !result.cancelled;
                            // SQL error messages can contain source values. Only
                            // fixed counters and SQLSTATEs enter audits/alerts.
                            let sqlstates: Vec<_> = result
                                .failures
                                .iter()
                                .take(8)
                                .filter_map(|failure| failure.sqlstate.as_deref())
                                .collect();
                            let details = json!({
                                "buckets_processed": result.buckets_processed,
                                "notifications_processed": result.notifications_processed,
                                "groups_written": result.groups_written,
                                "serialization_retries": result.serialization_retries,
                                "lock_failures": result.lock_failures,
                                "deadline_failures": result.deadline_failures,
                                "failure_count": result.failures.len(), "sqlstates": sqlstates,
                                "cancelled": result.cancelled, "budget_exhausted": result.budget_exhausted,
                                "elapsed_milliseconds": result.elapsed_milliseconds,
                            });
                            if !success && !result.cancelled {
                                self.report_native_problem(
                                    "summary_builder_failed",
                                    details.clone(),
                                )
                                .await;
                            }
                            Ok((success, details))
                        }
                        Err(error) => Err(
                            json!({"job": job.name(), "failed": true, "sqlstate": safe_sqlstate(&error)}),
                        ),
                    }
                }
                MaintenanceJob::Retention => unreachable!(),
            };
            let (success, details) = match outcome {
                Ok(outcome) => outcome,
                Err(details) => {
                    // Do not expose arbitrary SQL errors or record bodies.
                    self.report_native_problem("native_job_failed", details.clone())
                        .await;
                    (false, details)
                }
            };
            info!(job = job.name(), success, progress = %details, "Native maintenance attempt finished");
            if let Err(error) = self.audit_native_attempt(job, success, details).await {
                warn!(job = job.name(), error = %error, "Failed to audit native maintenance progress");
            }
            if let Err(error) =
                ScheduleRepository::finish_attempt(&self.inner.pool, job, now, interval, success)
                    .await
            {
                warn!(job = job.name(), error = %error, "Failed to persist native maintenance cadence");
            }
            // Status is bounded separately. It cannot withhold successful job
            // progress, and a failed status probe does not stop the next job.
            self.report_native_status(job, retention, now).await;
        }
    }

    async fn audit_native_attempt(
        &self,
        job: MaintenanceJob,
        success: bool,
        details: serde_json::Value,
    ) -> anyhow::Result<()> {
        let event = AuditEventBuilder::new(
            AuditCategory::Admin,
            "maintenance.native.job_completed",
            if success {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
        )
        .actor_login("attune-supervisor")
        .actor_token_type("system")
        .resource("native_maintenance")
        .resource_ref(job.name())
        .with_details(details)
        .build();
        AuditRepository::insert(&self.inner.pool, event).await?;
        Ok(())
    }

    async fn report_native_status(
        &self,
        job: MaintenanceJob,
        retention: &RetentionConfig,
        now: DateTime<Utc>,
    ) {
        let config = &retention.native_maintenance;
        let statuses = match job {
            MaintenanceJob::Partition => PartitionRepository::status(&self.inner.pool, config, now)
                .await
                .map(|rows| {
                    rows.into_iter()
                        .map(|row| {
                            let problem =
                                row.missing_future_partitions > 0 || row.default_rows_at_least > 0;
                            (problem, "partition_backlog", json!(row))
                        })
                        .collect::<Vec<_>>()
                }),
            MaintenanceJob::Summary => ScheduleRepository::summary_backlog_status(
                &self.inner.pool,
                config.max_summary_invalidations_per_bucket.min(10_000),
                config.operation_timeout_milliseconds,
            )
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| {
                        let age = row
                            .oldest_observed_notification
                            .map(|time| (now - time).num_seconds())
                            .unwrap_or(0);
                        let problem = !row.count_exact
                            || row.notifications_at_least
                                > config.max_summary_invalidations_per_bucket
                            || age
                                > config
                                    .summary_interval_seconds
                                    .saturating_mul(2)
                                    .min(i64::MAX as u64) as i64;
                        (problem, "summary_backlog", json!(row))
                    })
                    .collect::<Vec<_>>()
            }),
            MaintenanceJob::Retention => return,
        };
        match statuses {
            Ok(statuses) => {
                let mut emitted = 0;
                for (problem, failure_type, details) in statuses {
                    info!(job = job.name(), status = %details, "Native maintenance status");
                    if problem && emitted < self.inner.config.maintenance.alert_limit_per_cycle {
                        self.report_native_problem(failure_type, details).await;
                        emitted += 1;
                    }
                }
            }
            Err(_) => {
                self.report_native_problem("native_status_failed", json!({"job": job.name()}))
                    .await
            }
        }
    }

    pub(super) async fn report_native_problem(
        &self,
        failure_type: &'static str,
        details: serde_json::Value,
    ) {
        let maintenance = &self.inner.config.maintenance;
        if !maintenance.monitoring_enabled || maintenance.alert_limit_per_cycle == 0 {
            return;
        }
        let component = details
            .get("parent")
            .or_else(|| details.get("kind"))
            .or_else(|| details.get("job"))
            .and_then(|value| value.as_str())
            .unwrap_or("native_maintenance");
        let correlation_id = format!("supervisor:native:{failure_type}:{component}");
        match self
            .alert_recently_emitted(&correlation_id, maintenance)
            .await
        {
            Ok(false) => {}
            Ok(true) => return,
            Err(error) => {
                warn!(error = %error, "Failed to check native alert cooldown");
                return;
            }
        }
        let request = SupervisorAlertRequest {
            severity: "warning",
            category: "maintenance",
            failure_type,
            component_type: "native_maintenance",
            component_ref: Some(component.to_owned()),
            summary: format!("Native maintenance requires attention: {failure_type}"),
            details,
            correlation_id,
        };
        if let Err(error) = self.emit_supervisor_alert(request).await {
            warn!(error = %error, "Failed to emit native maintenance alert");
        }
    }
}

fn safe_sqlstate(error: &attune_common::Error) -> Option<String> {
    match error {
        attune_common::Error::Database(sqlx::Error::Database(database)) => {
            database.code().map(|code| code.into_owned())
        }
        _ => None,
    }
}

fn partition_failure_details(failure: &PartitionCycleFailure) -> serde_json::Value {
    json!({
        "job": "partition", "failed": true,
        "sqlstate": safe_sqlstate(&failure.source),
        "partial_counters": failure.partial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_retention_does_not_suppress_native_polling() {
        let mut config = RetentionConfig {
            enabled: false,
            check_interval_seconds: 3600,
            ..Default::default()
        };
        config.native_maintenance.summary_interval_seconds = 5;
        assert_eq!(poll_interval(&config), Duration::from_secs(5));
        config.native_maintenance.enabled = false;
        assert_eq!(poll_interval(&config), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn partition_failure_audit_preserves_committed_progress_without_sql_messages() {
        use attune_common::{
            artifact_transport::VolumeTransport, blob_store::FilesystemBlobStore, config::Config,
            test_database::TestDatabase,
        };
        use std::sync::{
            atomic::{AtomicBool, AtomicI64},
            Arc,
        };
        use tokio::sync::{broadcast, Mutex};
        let mut config = Config::load_from_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config.test.yaml"
        ))
        .unwrap();
        let db = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let artifacts = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        config.maintenance.monitoring_enabled = false;
        let (shutdown_tx, _) = broadcast::channel(1);
        let service = SupervisorService {
            inner: Arc::new(crate::SupervisorServiceInner {
                pool: db.pool().clone(),
                config,
                artifact_transport: Arc::new(VolumeTransport::new(
                    artifacts.path().to_str().unwrap(),
                )),
                blob_store: Arc::new(FilesystemBlobStore::new(objects.path()).unwrap()),
                publisher: None,
                _mq_connection: None,
                run_id: Mutex::new(None),
                cache_retention_state: Arc::new(
                    crate::cache_retention::CacheRetentionState::default(),
                ),
                artifact_reconciliation_cursor: AtomicI64::new(0),
                shutdown_requested: AtomicBool::new(false),
                shutdown_tx,
            }),
        };
        let now = DateTime::parse_from_rfc3339("2045-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        sqlx::query("INSERT INTO event(created,trigger_ref,payload) VALUES('2045-01-01 00:00+00','private.fixture','{\"secret\":\"do-not-audit\"}')")
            .execute(db.pool()).await.unwrap();
        sqlx::query(
            "CREATE TABLE execution_history_p20450101(LIKE execution_history INCLUDING ALL)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let mut retention = RetentionConfig::default();
        retention.native_maintenance.partition_lookahead_days = 1;
        // This is an audit-counter contract, not a latency gate. The fixture
        // needs a committed repair before its injected catalog failure; a valid
        // one-second deferral on the constrained server cannot exercise that.
        retention.native_maintenance.operation_timeout_milliseconds = 10_000;
        retention
            .native_maintenance
            .max_partition_cycle_milliseconds = 20_000;
        service
            .run_native_maintenance(
                &retention,
                SupervisorCycleReason::StartupRecovery,
                now,
                &[MaintenanceJob::Partition],
            )
            .await;
        let (outcome, details): (String, serde_json::Value) = sqlx::query_as(
            "SELECT outcome::text, details FROM audit_event WHERE event_type='maintenance.native.job_completed' AND resource_ref='partition'",
        ).fetch_one(db.pool()).await.unwrap();
        drop(service);
        db.cleanup().await.unwrap();
        assert_eq!(outcome, "failure");
        assert_eq!(details["partial_counters"]["created"], 1, "audit={details}");
        assert_eq!(details["partial_counters"]["rows_moved"], 1);
        assert_eq!(details["sqlstate"], "55000");
        for private in ["do-not-audit", "Refusing", "execution_history_p20450101"] {
            assert!(!details.to_string().contains(private));
        }
    }
}
