//! Runtime retention repository.
//!
//! This module owns the SQL used by the supervisor to purge runtime metadata.

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, PgConnection, PgPool, Row};

use crate::{
    config::{RetentionConfig, RetentionTargetConfig, RetentionTargetsConfig},
    Result,
};

/// Runtime retention targets managed by the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionTarget {
    Events,
    Enforcements,
    Executions,
    ExecutionHistory,
    WorkerHistory,
    SensorProcessHistory,
    AuditEvents,
    ContinuousAggregates,
    Notifications,
    WebhookEventLogs,
    Inquiries,
    WorkQueueItems,
    WorkQueueDispatches,
    PackTestExecutions,
    ExecutionAdmission,
    Workers,
    SensorProcesses,
}

impl RetentionTarget {
    pub fn all() -> [Self; 17] {
        [
            Self::Events,
            Self::Enforcements,
            Self::Executions,
            Self::ExecutionHistory,
            Self::WorkerHistory,
            Self::SensorProcessHistory,
            Self::AuditEvents,
            Self::ContinuousAggregates,
            Self::Notifications,
            Self::WebhookEventLogs,
            Self::Inquiries,
            Self::WorkQueueItems,
            Self::WorkQueueDispatches,
            Self::PackTestExecutions,
            Self::ExecutionAdmission,
            Self::Workers,
            Self::SensorProcesses,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Enforcements => "enforcements",
            Self::Executions => "executions",
            Self::ExecutionHistory => "execution_history",
            Self::WorkerHistory => "worker_history",
            Self::SensorProcessHistory => "sensor_process_history",
            Self::AuditEvents => "audit_events",
            Self::ContinuousAggregates => "continuous_aggregates",
            Self::Notifications => "notifications",
            Self::WebhookEventLogs => "webhook_event_logs",
            Self::Inquiries => "inquiries",
            Self::WorkQueueItems => "work_queue_items",
            Self::WorkQueueDispatches => "work_queue_dispatches",
            Self::PackTestExecutions => "pack_test_executions",
            Self::ExecutionAdmission => "execution_admission",
            Self::Workers => "workers",
            Self::SensorProcesses => "sensor_processes",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|target| target.name() == name)
    }
}

#[derive(Debug, Clone, FromRow)]
struct RuntimeRetentionConfigRow {
    enabled: bool,
    check_interval_seconds: i64,
    batch_size: i64,
    dry_run: bool,
    advisory_lock_key: i64,
}

#[derive(Debug, Clone, FromRow)]
struct RuntimeRetentionTargetConfigRow {
    target: String,
    max_age_seconds: Option<i64>,
}

/// Effective retention target selected from config.
#[derive(Debug, Clone, Copy)]
pub struct RetentionTargetRunConfig {
    pub target: RetentionTarget,
    pub max_age_seconds: Option<u64>,
}

/// Per-target retention result.
#[derive(Debug, Clone)]
pub struct RetentionTargetResult {
    pub target: RetentionTarget,
    pub cutoff: Option<DateTime<Utc>>,
    pub candidates: i64,
    pub deleted: i64,
    pub dry_run: bool,
}

/// Repository for runtime metadata retention operations.
pub struct RetentionRepository;

impl RetentionRepository {
    /// Ensure the database-backed runtime retention config exists.
    pub async fn ensure_config(pool: &PgPool) -> Result<()> {
        let defaults = RetentionConfig::default();

        sqlx::query(
            "INSERT INTO runtime_retention_config (
                id, enabled, check_interval_seconds, batch_size, dry_run, advisory_lock_key
             )
             VALUES (TRUE, $1, $2, $3, $4, $5)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(defaults.enabled)
        .bind(defaults.check_interval_seconds as i64)
        .bind(defaults.batch_size)
        .bind(defaults.dry_run)
        .bind(defaults.advisory_lock_key)
        .execute(pool)
        .await?;

        for (target, config) in Self::target_config_pairs(&defaults.targets) {
            sqlx::query(
                "INSERT INTO runtime_retention_target_config (target, max_age_seconds)
                 VALUES ($1, $2)
                 ON CONFLICT (target) DO NOTHING",
            )
            .bind(target.name())
            .bind(config.max_age_seconds.map(|value| value as i64))
            .execute(pool)
            .await?;
        }

        Ok(())
    }

    /// Load the database-backed runtime retention config.
    pub async fn load_config(pool: &PgPool) -> Result<RetentionConfig> {
        Self::ensure_config(pool).await?;

        let row = sqlx::query_as::<_, RuntimeRetentionConfigRow>(
            "SELECT enabled, check_interval_seconds, batch_size, dry_run, advisory_lock_key
             FROM runtime_retention_config
             WHERE id = TRUE",
        )
        .fetch_one(pool)
        .await?;

        let target_rows = sqlx::query_as::<_, RuntimeRetentionTargetConfigRow>(
            "SELECT target, max_age_seconds
             FROM runtime_retention_target_config
             ORDER BY target ASC",
        )
        .fetch_all(pool)
        .await?;

        let mut targets = RetentionTargetsConfig::default();
        for target_row in target_rows {
            let Some(target) = RetentionTarget::from_name(&target_row.target) else {
                continue;
            };
            Self::set_target_config(
                &mut targets,
                target,
                RetentionTargetConfig {
                    max_age_seconds: target_row.max_age_seconds.map(|value| value as u64),
                },
            );
        }

        Ok(RetentionConfig {
            enabled: row.enabled,
            check_interval_seconds: row.check_interval_seconds as u64,
            batch_size: row.batch_size,
            dry_run: row.dry_run,
            advisory_lock_key: row.advisory_lock_key,
            targets,
        })
    }

    /// Persist the full runtime retention config and return the stored value.
    pub async fn update_config(pool: &PgPool, config: &RetentionConfig) -> Result<RetentionConfig> {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO runtime_retention_config (
                id, enabled, check_interval_seconds, batch_size, dry_run, advisory_lock_key
             )
             VALUES (TRUE, $1, $2, $3, $4, $5)
             ON CONFLICT (id) DO UPDATE SET
                enabled = EXCLUDED.enabled,
                check_interval_seconds = EXCLUDED.check_interval_seconds,
                batch_size = EXCLUDED.batch_size,
                dry_run = EXCLUDED.dry_run,
                advisory_lock_key = EXCLUDED.advisory_lock_key",
        )
        .bind(config.enabled)
        .bind(config.check_interval_seconds as i64)
        .bind(config.batch_size)
        .bind(config.dry_run)
        .bind(config.advisory_lock_key)
        .execute(&mut *tx)
        .await?;

        for (target, target_config) in Self::target_config_pairs(&config.targets) {
            sqlx::query(
                "INSERT INTO runtime_retention_target_config (target, max_age_seconds)
                 VALUES ($1, $2)
                 ON CONFLICT (target) DO UPDATE SET
                    max_age_seconds = EXCLUDED.max_age_seconds",
            )
            .bind(target.name())
            .bind(target_config.max_age_seconds.map(|value| value as i64))
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Self::load_config(pool).await
    }

    /// Build the target list from configuration.
    ///
    /// All targets are returned. Targets with `max_age_seconds: None` are skipped
    /// by the supervisor at runtime (keep forever).
    ///
    /// The order is dependency-aware for regular tables. For example, stale
    /// `sensor_process` rows must be purged before `worker` rows so a worker is
    /// not kept around for an extra cycle solely because it still has a
    /// retention-eligible sensor-process child.
    pub fn configured_targets(targets: &RetentionTargetsConfig) -> Vec<RetentionTargetRunConfig> {
        Self::target_config_pairs(targets)
            .into_iter()
            .map(|(target, config)| RetentionTargetRunConfig {
                target,
                max_age_seconds: config.max_age_seconds,
            })
            .collect()
    }

    fn target_config_pairs(
        targets: &RetentionTargetsConfig,
    ) -> [(RetentionTarget, &RetentionTargetConfig); 17] {
        [
            (RetentionTarget::Events, &targets.events),
            (RetentionTarget::Enforcements, &targets.enforcements),
            (RetentionTarget::Executions, &targets.executions),
            (
                RetentionTarget::ExecutionHistory,
                &targets.execution_history,
            ),
            (RetentionTarget::WorkerHistory, &targets.worker_history),
            (
                RetentionTarget::SensorProcessHistory,
                &targets.sensor_process_history,
            ),
            (RetentionTarget::AuditEvents, &targets.audit_events),
            (
                RetentionTarget::ContinuousAggregates,
                &targets.continuous_aggregates,
            ),
            (RetentionTarget::Notifications, &targets.notifications),
            (
                RetentionTarget::WebhookEventLogs,
                &targets.webhook_event_logs,
            ),
            (RetentionTarget::Inquiries, &targets.inquiries),
            (RetentionTarget::WorkQueueItems, &targets.work_queue_items),
            (
                RetentionTarget::WorkQueueDispatches,
                &targets.work_queue_dispatches,
            ),
            (
                RetentionTarget::PackTestExecutions,
                &targets.pack_test_executions,
            ),
            (RetentionTarget::SensorProcesses, &targets.sensor_processes),
            (
                RetentionTarget::ExecutionAdmission,
                &targets.execution_admission,
            ),
            (RetentionTarget::Workers, &targets.workers),
        ]
    }

    fn set_target_config(
        targets: &mut RetentionTargetsConfig,
        target: RetentionTarget,
        config: RetentionTargetConfig,
    ) {
        match target {
            RetentionTarget::Events => targets.events = config,
            RetentionTarget::Enforcements => targets.enforcements = config,
            RetentionTarget::Executions => targets.executions = config,
            RetentionTarget::ExecutionHistory => targets.execution_history = config,
            RetentionTarget::WorkerHistory => targets.worker_history = config,
            RetentionTarget::SensorProcessHistory => targets.sensor_process_history = config,
            RetentionTarget::AuditEvents => targets.audit_events = config,
            RetentionTarget::ContinuousAggregates => targets.continuous_aggregates = config,
            RetentionTarget::Notifications => targets.notifications = config,
            RetentionTarget::WebhookEventLogs => targets.webhook_event_logs = config,
            RetentionTarget::Inquiries => targets.inquiries = config,
            RetentionTarget::WorkQueueItems => targets.work_queue_items = config,
            RetentionTarget::WorkQueueDispatches => targets.work_queue_dispatches = config,
            RetentionTarget::PackTestExecutions => targets.pack_test_executions = config,
            RetentionTarget::ExecutionAdmission => targets.execution_admission = config,
            RetentionTarget::Workers => targets.workers = config,
            RetentionTarget::SensorProcesses => targets.sensor_processes = config,
        }
    }

    /// Try to acquire a session-level advisory lock for one retention cycle.
    pub async fn try_advisory_lock(conn: &mut PgConnection, key: i64) -> Result<bool> {
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await
            .map_err(Into::into)
    }

    /// Release a previously acquired advisory lock.
    pub async fn advisory_unlock(conn: &mut PgConnection, key: i64) -> Result<bool> {
        sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await
            .map_err(Into::into)
    }

    /// Run a single retention target.
    pub async fn run_target(
        pool: &PgPool,
        target: RetentionTarget,
        max_age_seconds: u64,
        batch_size: i64,
        dry_run: bool,
    ) -> Result<RetentionTargetResult> {
        let cutoff = retention_cutoff(max_age_seconds);
        let batch_size = batch_size.max(1);
        let (candidates, deleted) = match target {
            RetentionTarget::Events => {
                Self::drop_hypertable_chunks(pool, "event", "created", cutoff, dry_run).await?
            }
            RetentionTarget::ExecutionHistory => {
                Self::drop_hypertable_chunks(pool, "execution_history", "time", cutoff, dry_run)
                    .await?
            }
            RetentionTarget::WorkerHistory => {
                Self::drop_hypertable_chunks(pool, "worker_history", "time", cutoff, dry_run)
                    .await?
            }
            RetentionTarget::SensorProcessHistory => {
                Self::drop_hypertable_chunks(
                    pool,
                    "sensor_process_history",
                    "time",
                    cutoff,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::AuditEvents => {
                Self::drop_hypertable_chunks(pool, "audit_event", "created", cutoff, dry_run)
                    .await?
            }
            RetentionTarget::ContinuousAggregates => {
                Self::drop_continuous_aggregate_chunks(pool, cutoff, dry_run).await?
            }
            RetentionTarget::Enforcements => {
                Self::delete_limited(
                    pool,
                    "enforcement",
                    "created < $1 AND status <> 'created'",
                    "created",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::Executions => {
                Self::delete_limited(
                    pool,
                    "execution",
                    "updated < $1 AND status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')",
                    "updated",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::Notifications => {
                Self::delete_limited(
                    pool,
                    "notification",
                    "created < $1",
                    "created",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::WebhookEventLogs => {
                Self::delete_limited(
                    pool,
                    "webhook_event_log",
                    "created < $1",
                    "created",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::Inquiries => {
                Self::delete_limited(
                    pool,
                    "inquiry",
                    "updated < $1 AND status IN ('responded', 'timeout', 'cancelled')",
                    "updated",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::WorkQueueItems => {
                Self::delete_limited(
                    pool,
                    "work_queue_item",
                    "updated < $1 AND status IN ('completed', 'failed', 'skipped', 'cancelled')",
                    "updated",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::WorkQueueDispatches => {
                Self::delete_limited(
                    pool,
                    "work_queue_dispatch",
                    "updated < $1 AND status IN ('completed', 'failed', 'released', 'cancelled')",
                    "updated",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::PackTestExecutions => {
                Self::delete_limited(
                    pool,
                    "pack_test_execution",
                    "execution_time < $1",
                    "execution_time",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::ExecutionAdmission => {
                Self::delete_execution_admission(pool, cutoff, batch_size, dry_run).await?
            }
            RetentionTarget::Workers => {
                Self::delete_limited(
                    pool,
                    "worker",
                    "updated < $1 AND status IN ('inactive', 'error') AND cordoned = false AND NOT EXISTS (SELECT 1 FROM sensor_process sp WHERE sp.worker = worker.id AND sp.status IN ('starting', 'running', 'backoff'))",
                    "updated",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
            RetentionTarget::SensorProcesses => {
                Self::delete_limited(
                    pool,
                    "sensor_process",
                    "updated < $1 AND status IN ('stopped', 'failed') AND active_rule_count = 0",
                    "updated",
                    cutoff,
                    batch_size,
                    dry_run,
                )
                .await?
            }
        };

        Ok(RetentionTargetResult {
            target,
            cutoff: Some(cutoff),
            candidates,
            deleted,
            dry_run,
        })
    }

    /// Count rows/chunks still older than a target cutoff, without mutating data.
    pub async fn count_target_candidates(
        pool: &PgPool,
        target: RetentionTarget,
        cutoff: DateTime<Utc>,
    ) -> Result<i64> {
        match target {
            RetentionTarget::Events => Self::count_predicate(pool, "event", "created < $1", cutoff).await,
            RetentionTarget::ExecutionHistory => {
                Self::count_predicate(pool, "execution_history", "time < $1", cutoff).await
            }
            RetentionTarget::WorkerHistory => {
                Self::count_predicate(pool, "worker_history", "time < $1", cutoff).await
            }
            RetentionTarget::SensorProcessHistory => {
                Self::count_predicate(pool, "sensor_process_history", "time < $1", cutoff).await
            }
            RetentionTarget::AuditEvents => {
                Self::count_predicate(pool, "audit_event", "created < $1", cutoff).await
            }
            RetentionTarget::ContinuousAggregates => {
                Self::count_continuous_aggregate_candidates(pool, cutoff).await
            }
            RetentionTarget::Enforcements => {
                Self::count_predicate(pool, "enforcement", "created < $1 AND status <> 'created'", cutoff).await
            }
            RetentionTarget::Executions => {
                Self::count_predicate(
                    pool,
                    "execution",
                    "updated < $1 AND status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned')",
                    cutoff,
                )
                .await
            }
            RetentionTarget::Notifications => {
                Self::count_predicate(pool, "notification", "created < $1", cutoff).await
            }
            RetentionTarget::WebhookEventLogs => {
                Self::count_predicate(pool, "webhook_event_log", "created < $1", cutoff).await
            }
            RetentionTarget::Inquiries => {
                Self::count_predicate(
                    pool,
                    "inquiry",
                    "updated < $1 AND status IN ('responded', 'timeout', 'cancelled')",
                    cutoff,
                )
                .await
            }
            RetentionTarget::WorkQueueItems => {
                Self::count_predicate(
                    pool,
                    "work_queue_item",
                    "updated < $1 AND status IN ('completed', 'failed', 'skipped', 'cancelled')",
                    cutoff,
                )
                .await
            }
            RetentionTarget::WorkQueueDispatches => {
                Self::count_predicate(
                    pool,
                    "work_queue_dispatch",
                    "updated < $1 AND status IN ('completed', 'failed', 'released', 'cancelled')",
                    cutoff,
                )
                .await
            }
            RetentionTarget::PackTestExecutions => {
                Self::count_predicate(pool, "pack_test_execution", "execution_time < $1", cutoff).await
            }
            RetentionTarget::ExecutionAdmission => {
                Self::count_execution_admission_candidates(pool, cutoff).await
            }
            RetentionTarget::Workers => {
                Self::count_predicate(
                    pool,
                    "worker",
                    "updated < $1 AND status IN ('inactive', 'error') AND cordoned = false AND NOT EXISTS (SELECT 1 FROM sensor_process sp WHERE sp.worker = worker.id AND sp.status IN ('starting', 'running', 'backoff'))",
                    cutoff,
                )
                .await
            }
            RetentionTarget::SensorProcesses => {
                Self::count_predicate(
                    pool,
                    "sensor_process",
                    "updated < $1 AND status IN ('stopped', 'failed') AND active_rule_count = 0",
                    cutoff,
                )
                .await
            }
        }
    }

    fn count_sql(table: &str, predicate: &str) -> String {
        format!("SELECT COUNT(*)::BIGINT FROM {table} WHERE {predicate}")
    }

    async fn count_predicate(
        pool: &PgPool,
        table: &str,
        predicate: &str,
        cutoff: DateTime<Utc>,
    ) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(&Self::count_sql(table, predicate))
            .bind(cutoff)
            .fetch_one(pool)
            .await
            .map_err(Into::into)
    }

    fn delete_sql(table: &str, predicate: &str, order_column: &str) -> String {
        format!(
            "WITH doomed AS (
                SELECT id FROM {table}
                WHERE {predicate}
                ORDER BY {order_column} ASC, id ASC
                LIMIT $2
             ),
             deleted AS (
                DELETE FROM {table}
                WHERE id IN (SELECT id FROM doomed)
                RETURNING 1
             )
             SELECT COUNT(*)::BIGINT FROM deleted"
        )
    }

    async fn delete_limited(
        pool: &PgPool,
        table: &str,
        predicate: &str,
        order_column: &str,
        cutoff: DateTime<Utc>,
        batch_size: i64,
        dry_run: bool,
    ) -> Result<(i64, i64)> {
        let candidates = sqlx::query_scalar::<_, i64>(&Self::count_sql(table, predicate))
            .bind(cutoff)
            .fetch_one(pool)
            .await?;

        if dry_run || candidates == 0 {
            return Ok((candidates, 0));
        }

        let deleted =
            sqlx::query_scalar::<_, i64>(&Self::delete_sql(table, predicate, order_column))
                .bind(cutoff)
                .bind(batch_size)
                .fetch_one(pool)
                .await?;

        Ok((candidates, deleted))
    }

    async fn drop_hypertable_chunks(
        pool: &PgPool,
        table: &str,
        time_column: &str,
        cutoff: DateTime<Utc>,
        dry_run: bool,
    ) -> Result<(i64, i64)> {
        let count_sql = format!("SELECT COUNT(*)::BIGINT FROM {table} WHERE {time_column} < $1");
        let candidates = sqlx::query_scalar::<_, i64>(&count_sql)
            .bind(cutoff)
            .fetch_one(pool)
            .await?;

        if dry_run || candidates == 0 {
            return Ok((candidates, 0));
        }

        let drop_sql = format!(
            "SELECT COUNT(*)::BIGINT FROM drop_chunks('{}', older_than => $1::timestamptz)",
            table.replace('\'', "''")
        );
        let dropped_chunks = sqlx::query_scalar::<_, i64>(&drop_sql)
            .bind(cutoff)
            .fetch_one(pool)
            .await?;

        Ok((candidates, dropped_chunks))
    }

    async fn drop_continuous_aggregate_chunks(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
        dry_run: bool,
    ) -> Result<(i64, i64)> {
        let aggregates = [
            "execution_status_hourly",
            "execution_throughput_hourly",
            "event_volume_hourly",
            "worker_status_hourly",
        ];
        let mut candidates = 0;
        let mut dropped_chunks = 0;

        for aggregate in aggregates {
            let count_sql = format!("SELECT COUNT(*)::BIGINT FROM {aggregate} WHERE bucket < $1");
            candidates += sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(cutoff)
                .fetch_one(pool)
                .await?;

            if !dry_run {
                let drop_sql = format!(
                    "SELECT COUNT(*)::BIGINT FROM drop_chunks('{}', older_than => $1::timestamptz)",
                    aggregate
                );
                dropped_chunks += sqlx::query_scalar::<_, i64>(&drop_sql)
                    .bind(cutoff)
                    .fetch_one(pool)
                    .await?;
            }
        }

        Ok((candidates, dropped_chunks))
    }

    async fn count_continuous_aggregate_candidates(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
    ) -> Result<i64> {
        let aggregates = [
            "execution_status_hourly",
            "execution_throughput_hourly",
            "event_volume_hourly",
            "worker_status_hourly",
        ];
        let mut candidates = 0;

        for aggregate in aggregates {
            let count_sql = format!("SELECT COUNT(*)::BIGINT FROM {aggregate} WHERE bucket < $1");
            candidates += sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(cutoff)
                .fetch_one(pool)
                .await?;
        }

        Ok(candidates)
    }

    async fn count_execution_admission_candidates(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
    ) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM execution_admission_state s
             WHERE s.updated < $1
               AND NOT EXISTS (
                   SELECT 1 FROM execution_admission_entry e WHERE e.state_id = s.id
               )",
        )
        .bind(cutoff)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }

    async fn delete_execution_admission(
        pool: &PgPool,
        cutoff: DateTime<Utc>,
        batch_size: i64,
        dry_run: bool,
    ) -> Result<(i64, i64)> {
        let candidates = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT
             FROM execution_admission_state s
             WHERE s.updated < $1
               AND NOT EXISTS (
                   SELECT 1 FROM execution_admission_entry e WHERE e.state_id = s.id
               )",
        )
        .bind(cutoff)
        .fetch_one(pool)
        .await?;

        if dry_run || candidates == 0 {
            return Ok((candidates, 0));
        }

        let row = sqlx::query(
            "WITH doomed AS (
                SELECT s.id
                FROM execution_admission_state s
                WHERE s.updated < $1
                  AND NOT EXISTS (
                      SELECT 1 FROM execution_admission_entry e WHERE e.state_id = s.id
                  )
                ORDER BY s.updated ASC, s.id ASC
                LIMIT $2
             ),
             deleted AS (
                DELETE FROM execution_admission_state
                WHERE id IN (SELECT id FROM doomed)
                RETURNING 1
             )
             SELECT COUNT(*)::BIGINT AS deleted FROM deleted",
        )
        .bind(cutoff)
        .bind(batch_size)
        .fetch_one(pool)
        .await?;

        Ok((candidates, row.get::<i64, _>("deleted")))
    }
}

fn retention_cutoff(max_age_seconds: u64) -> DateTime<Utc> {
    let seconds = max_age_seconds.min(i64::MAX as u64) as i64;
    Utc::now() - Duration::seconds(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RetentionTargetsConfig;

    #[test]
    fn configured_targets_includes_all_targets() {
        let mut targets = RetentionTargetsConfig::default();
        targets.audit_events.max_age_seconds = None;

        let configured = RetentionRepository::configured_targets(&targets);

        // All 17 targets are returned regardless of max_age_seconds
        assert_eq!(configured.len(), 17);
        // Target with max_age_seconds = None is included (supervisor skips it at runtime)
        assert!(configured.iter().any(|target| {
            target.target == RetentionTarget::AuditEvents && target.max_age_seconds.is_none()
        }));
    }

    #[test]
    fn retention_target_names_are_stable() {
        assert_eq!(RetentionTarget::Executions.name(), "executions");
        assert_eq!(RetentionTarget::AuditEvents.name(), "audit_events");
    }

    #[test]
    fn configured_targets_purge_sensor_processes_before_workers() {
        let configured =
            RetentionRepository::configured_targets(&RetentionTargetsConfig::default());
        let sensor_process_index = configured
            .iter()
            .position(|target| target.target == RetentionTarget::SensorProcesses)
            .expect("sensor_processes target should be present");
        let worker_index = configured
            .iter()
            .position(|target| target.target == RetentionTarget::Workers)
            .expect("workers target should be present");

        assert!(
            sensor_process_index < worker_index,
            "sensor_process retention should run before worker retention",
        );
    }
}
