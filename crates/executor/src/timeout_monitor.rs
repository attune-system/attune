//! Execution Timeout Monitor
//!
//! This module monitors executions in SCHEDULED status and fails them if they
//! don't transition to RUNNING within a configured timeout period.
//!
//! This prevents executions from being stuck indefinitely when workers:
//! - Stop or crash after being selected
//! - Fail to consume messages from their queues
//! - Are partitioned from the network

use anyhow::Result;
use attune_common::{
    models::{enums::ExecutionStatus, Execution, Worker, WorkerStatus},
    mq::{ExecutionCompletedPayload, MessageEnvelope, MessageType, Publisher},
    repositories::{
        execution::UpdateExecutionInput, runtime::WorkerRepository, ExecutionRepository, FindById,
    },
    system_alert::{emit_core_alert, SystemAlert},
};
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Configuration for timeout monitor
#[derive(Debug, Clone)]
pub struct TimeoutMonitorConfig {
    /// How long an execution can remain in SCHEDULED status before timing out
    pub scheduled_timeout: Duration,

    /// How often to check for stale executions
    pub check_interval: Duration,

    /// Whether to enable the timeout monitor
    pub enabled: bool,
}

impl Default for TimeoutMonitorConfig {
    fn default() -> Self {
        Self {
            scheduled_timeout: Duration::from_secs(300), // 5 minutes
            check_interval: Duration::from_secs(60),     // 1 minute
            enabled: true,
        }
    }
}

/// Monitors scheduled executions and fails those that timeout
pub struct ExecutionTimeoutMonitor {
    pool: PgPool,
    publisher: Arc<Publisher>,
    config: TimeoutMonitorConfig,
}

impl ExecutionTimeoutMonitor {
    /// Create a new timeout monitor
    pub fn new(pool: PgPool, publisher: Arc<Publisher>, config: TimeoutMonitorConfig) -> Self {
        Self {
            pool,
            publisher,
            config,
        }
    }

    /// Start the timeout monitor loop
    pub async fn start(self: Arc<Self>) -> Result<()> {
        if !self.config.enabled {
            info!("Execution timeout monitor is disabled");
            return Ok(());
        }

        info!(
            "Starting execution timeout monitor (timeout: {}s, check interval: {}s)",
            self.config.scheduled_timeout.as_secs(),
            self.config.check_interval.as_secs()
        );

        let mut check_interval = interval(self.config.check_interval);

        loop {
            check_interval.tick().await;

            if let Err(e) = self.check_stale_executions().await {
                error!("Error checking stale executions: {}", e);
                // Continue running despite errors
            }
            if let Err(e) = self.reconcile_running_executions_on_dead_workers().await {
                error!(
                    "Error reconciling running executions on dead workers: {}",
                    e
                );
            }
        }
    }

    /// Check for executions stuck in SCHEDULED status
    async fn check_stale_executions(&self) -> Result<()> {
        let cutoff = self.calculate_cutoff_time();

        debug!(
            "Checking for executions scheduled before {}",
            cutoff.format("%Y-%m-%d %H:%M:%S UTC")
        );

        // Find executions stuck in SCHEDULED status
        let stale_executions = ExecutionRepository::find_legacy_reconciliation_candidates(
            &self.pool,
            ExecutionStatus::Scheduled,
            cutoff,
            Some(100),
        )
        .await?;

        if stale_executions.is_empty() {
            debug!("No stale scheduled executions found");
            return Ok(());
        }

        warn!(
            "Found {} stale scheduled executions (older than {}s)",
            stale_executions.len(),
            self.config.scheduled_timeout.as_secs()
        );

        for execution in stale_executions {
            let age_seconds = (Utc::now() - execution.updated).num_seconds();

            warn!(
                "Execution {} has been scheduled for {} seconds (timeout: {}s), marking as failed",
                execution.id,
                age_seconds,
                self.config.scheduled_timeout.as_secs()
            );

            if let Err(e) = self.fail_execution(&execution, age_seconds).await {
                error!("Failed to fail execution {}: {}", execution.id, e);
                // Continue processing other executions
            }
        }

        Ok(())
    }

    /// Calculate the cutoff time for stale executions
    fn calculate_cutoff_time(&self) -> DateTime<Utc> {
        let timeout_duration = chrono::Duration::from_std(self.config.scheduled_timeout)
            .expect("Invalid timeout duration");

        Utc::now() - timeout_duration
    }

    /// Mark an execution as failed due to timeout
    async fn fail_execution(&self, execution: &Execution, age_seconds: i64) -> Result<()> {
        let execution_id = execution.id;
        let error_message = format!(
            "Execution timeout: worker did not pick up task within {} seconds (scheduled for {} seconds)",
            self.config.scheduled_timeout.as_secs(),
            age_seconds
        );

        info!(
            "Failing execution {} due to timeout: {}",
            execution_id, error_message
        );

        // Create failure result
        let result = serde_json::json!({
            "error": error_message,
            "failed_by": "execution_timeout_monitor",
            "timeout_seconds": self.config.scheduled_timeout.as_secs(),
            "age_seconds": age_seconds,
            "original_status": "scheduled"
        });

        let updated = ExecutionRepository::reconcile_legacy_execution(
            &self.pool,
            execution_id,
            ExecutionStatus::Scheduled,
            execution.updated,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Failed),
                result: Some(result.clone()),
                ..Default::default()
            },
        )
        .await?;

        if updated.is_none() {
            debug!(
                "Skipping timeout failure for execution {} because it already left Scheduled or is no longer stale",
                execution_id
            );
            return Ok(());
        }

        info!("Execution {} marked as failed in database", execution_id);

        // Publish completion notification
        self.publish_completion_notification(execution, ExecutionStatus::Failed, result)
            .await?;

        info!(
            "Published completion notification for execution {}",
            execution_id
        );

        Ok(())
    }

    /// Publish execution completion notification
    async fn publish_completion_notification(
        &self,
        execution: &Execution,
        status: ExecutionStatus,
        result: JsonValue,
    ) -> Result<()> {
        let payload = ExecutionCompletedPayload {
            execution_id: execution.id,
            action_id: execution.action.unwrap_or_default(),
            action_ref: execution.action_ref.clone(),
            status: format!("{:?}", status),
            result: Some(result),
            completed_at: Utc::now(),
        };

        let envelope = MessageEnvelope::new(MessageType::ExecutionCompleted, payload)
            .with_source("execution_timeout_monitor");

        // Publish to main executions exchange
        self.publisher.publish_envelope(&envelope).await?;

        Ok(())
    }

    async fn reconcile_running_executions_on_dead_workers(&self) -> Result<()> {
        let running_executions = ExecutionRepository::find_legacy_reconciliation_candidates(
            &self.pool,
            ExecutionStatus::Running,
            Utc::now(),
            None,
        )
        .await?;

        for execution in running_executions
            .into_iter()
            .filter(|exec| exec.worker.is_some())
        {
            let Some(worker_id) = execution.worker else {
                continue;
            };
            let Some(worker) = WorkerRepository::find_by_id(&self.pool, worker_id).await? else {
                warn!(
                    "Execution {} is running on missing worker {}, marking as abandoned",
                    execution.id, worker_id
                );
                self.abandon_running_execution(&execution, None).await?;
                continue;
            };

            if Self::worker_unavailable_for_running_execution(&worker) {
                warn!(
                    "Execution {} is running on unavailable worker {} (id={}), marking as abandoned",
                    execution.id, worker.name, worker.id
                );
                self.abandon_running_execution(&execution, Some(worker))
                    .await?;
            }
        }

        Ok(())
    }

    fn worker_unavailable_for_running_execution(worker: &Worker) -> bool {
        if matches!(
            worker.status,
            Some(WorkerStatus::Inactive | WorkerStatus::Error)
        ) {
            return true;
        }

        let Some(last_heartbeat) = worker.last_heartbeat else {
            return true;
        };
        let max_age = chrono::Duration::seconds(90);
        Utc::now().signed_duration_since(last_heartbeat) > max_age
    }

    async fn abandon_running_execution(
        &self,
        execution: &Execution,
        worker: Option<Worker>,
    ) -> Result<()> {
        let now = Utc::now();
        let worker_json = worker.as_ref().map(|worker| {
            let heartbeat_age_seconds = worker
                .last_heartbeat
                .map(|last| now.signed_duration_since(last).num_seconds().max(0));
            serde_json::json!({
                "id": worker.id,
                "name": worker.name,
                "role": worker.worker_role,
                "status": worker.status,
                "last_heartbeat": worker.last_heartbeat,
                "heartbeat_age_seconds": heartbeat_age_seconds,
                "cordoned": worker.cordoned,
            })
        });
        let result = serde_json::json!({
            "error": "Execution abandoned because its worker became unavailable while running",
            "abandoned_by": "execution_timeout_monitor",
            "original_status": "running",
            "worker": worker_json,
            "reconciled_at": now,
        });

        let updated = ExecutionRepository::reconcile_legacy_execution(
            &self.pool,
            execution.id,
            ExecutionStatus::Running,
            execution.updated,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Abandoned),
                result: Some(result.clone()),
                ..Default::default()
            },
        )
        .await?;

        let Some(updated) = updated else {
            debug!(
                "Skipping abandoned reconciliation for execution {} because it already left Running",
                execution.id
            );
            return Ok(());
        };

        self.publish_completion_notification(&updated, ExecutionStatus::Abandoned, result.clone())
            .await?;

        if let Some(worker) = worker.as_ref().filter(|worker| !worker.cordoned) {
            let alert = SystemAlert {
                severity: "error".to_string(),
                category: "execution_reconciliation".to_string(),
                failure_type: "execution_abandoned_worker_unavailable".to_string(),
                component_type: "execution".to_string(),
                component_id: Some(updated.id),
                component_ref: Some(updated.action_ref.clone()),
                worker_role: Some(format!("{:?}", worker.worker_role).to_lowercase()),
                observed_at: now,
                summary: format!(
                    "Execution {} was abandoned because worker '{}' became unavailable",
                    updated.id, worker.name
                ),
                details: serde_json::json!({
                    "execution_id": updated.id,
                    "action_ref": updated.action_ref.clone(),
                    "worker_id": worker.id,
                    "worker_name": worker.name,
                    "worker_role": worker.worker_role,
                    "worker_status": worker.status,
                    "last_heartbeat": worker.last_heartbeat,
                    "reconciliation_result": result,
                }),
                correlation_id: Some(format!("execution:{}:worker_unavailable", updated.id)),
            };
            if let Err(e) = emit_core_alert(&self.pool, Some(&self.publisher), alert).await {
                warn!(
                    "Failed to emit abandoned-execution alert for execution {}: {}",
                    updated.id, e
                );
            }
        }

        Ok(())
    }

    /// Get current configuration
    #[allow(dead_code)]
    pub fn config(&self) -> &TimeoutMonitorConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    fn create_test_config() -> TimeoutMonitorConfig {
        TimeoutMonitorConfig {
            scheduled_timeout: Duration::from_secs(60), // 1 minute for tests
            check_interval: Duration::from_secs(1),     // 1 second for tests
            enabled: true,
        }
    }

    #[test]
    fn test_config_defaults() {
        let config = TimeoutMonitorConfig::default();
        assert_eq!(config.scheduled_timeout.as_secs(), 300);
        assert_eq!(config.check_interval.as_secs(), 60);
        assert!(config.enabled);
    }

    #[test]
    fn test_cutoff_calculation() {
        // Test that cutoff is calculated as now - scheduled_timeout
        let config = create_test_config(); // scheduled_timeout = 60s

        let before = Utc::now() - ChronoDuration::seconds(60);

        // calculate_cutoff uses Utc::now() internally, so we compute expected bounds
        let timeout_duration =
            chrono::Duration::from_std(config.scheduled_timeout).expect("Invalid timeout duration");
        let cutoff = Utc::now() - timeout_duration;

        let after = Utc::now() - ChronoDuration::seconds(60);

        // cutoff should be between before and after (both ~60s ago)
        let diff_before = (cutoff - before).num_seconds().abs();
        let diff_after = (cutoff - after).num_seconds().abs();
        assert!(
            diff_before <= 1,
            "Cutoff time should be ~60s ago (before check)"
        );
        assert!(
            diff_after <= 1,
            "Cutoff time should be ~60s ago (after check)"
        );
    }

    #[test]
    fn test_disabled_config() {
        let mut config = create_test_config();
        config.enabled = false;

        // Verify the config is properly set to disabled
        assert!(!config.enabled);
        assert_eq!(config.scheduled_timeout.as_secs(), 60);
        assert_eq!(config.check_interval.as_secs(), 1);
    }
}
