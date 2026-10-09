//! Runtime retention repository.
//!
//! This module owns the SQL used by the supervisor to purge runtime metadata.

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, PgConnection, PgPool};

use super::native_maintenance::{
    partitions::{bounded_transaction, PartitionRepository},
    summaries::SummaryRepository,
    ManagedTable, SummaryKind,
};

use crate::{
    config::{
        CacheRetentionConfig, RetentionConfig, RetentionTargetConfig, RetentionTargetsConfig,
    },
    Result,
};

const EXECUTION_RETENTION_PREDICATE: &str =
    "updated < $1 AND status IN ('completed', 'failed', 'cancelled', 'timeout', 'abandoned') \
     AND NOT EXISTS ( \
         SELECT 1 FROM workflow_task_wait wait \
         WHERE wait.target_execution = execution.id AND wait.state = 'waiting' \
     ) \
     AND NOT EXISTS ( \
         SELECT 1 FROM workflow_execution workflow \
         JOIN workflow_log_outbox outbox ON outbox.workflow_execution = workflow.id \
         WHERE workflow.execution = execution.id AND outbox.delivered_at IS NULL \
     )";

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
    pub fn all() -> [Self; 16] {
        [
            Self::Events,
            Self::Enforcements,
            Self::Executions,
            Self::ExecutionHistory,
            Self::WorkerHistory,
            Self::SensorProcessHistory,
            Self::AuditEvents,
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
    max_batches_per_target: i64,
    dry_run: bool,
    advisory_lock_key: i64,
    cache_retention: serde_json::Value,
    native_maintenance: serde_json::Value,
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
    /// Row candidates can be a bounded lower bound on native targets. Whole
    /// partition contents are deliberately never counted for DDL accounting.
    pub candidates_exact: bool,
    /// Eligible daily leaves selected within the partition operation budget.
    pub partition_candidates: i64,
    pub partitions_dropped: i64,
    pub dry_run: bool,
}

/// A failed target run, including progress confirmed before the failure.
#[derive(Debug, thiserror::Error)]
#[error("Retention target {target:?} failed after {deleted} confirmed deleted rows and {partitions_dropped} confirmed partition drops: {source}")]
pub struct RetentionTargetFailure {
    pub target: RetentionTarget,
    pub cutoff: DateTime<Utc>,
    /// Initial candidate count, or `None` if counting did not succeed.
    pub candidates: Option<i64>,
    /// Rows deleted by successful, committed batches only. The failed
    /// statement may have committed without returning a response; its row
    /// count is unknown and is never included here.
    pub deleted: i64,
    pub candidates_exact: bool,
    pub partition_candidates: i64,
    pub partitions_dropped: i64,
    pub dry_run: bool,
    #[source]
    pub source: crate::Error,
}

/// Repository for runtime metadata retention operations.
pub struct RetentionRepository;

impl RetentionRepository {
    /// Ensure the database-backed runtime retention config exists.
    pub async fn ensure_config(pool: &PgPool) -> Result<()> {
        let defaults = RetentionConfig::default();

        sqlx::query(
            "INSERT INTO runtime_retention_config (
                id, enabled, check_interval_seconds, batch_size, max_batches_per_target, dry_run, advisory_lock_key,
                cache_retention, native_maintenance
             )
             VALUES (TRUE, $1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(defaults.enabled)
        .bind(defaults.check_interval_seconds as i64)
        .bind(defaults.batch_size)
        .bind(defaults.max_batches_per_target)
        .bind(defaults.dry_run)
        .bind(defaults.advisory_lock_key)
        .bind(serde_json::to_value(&defaults.cache_retention)?)
        .bind(serde_json::to_value(&defaults.native_maintenance)?)
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

    /// Seed the database cache-retention object from service configuration
    /// only while the migration-created singleton still contains `{}`. Once
    /// persisted, the database remains the source of truth.
    pub async fn seed_cache_config_if_empty(
        pool: &PgPool,
        cache_retention: &CacheRetentionConfig,
    ) -> Result<()> {
        Self::ensure_config(pool).await?;
        sqlx::query(
            "UPDATE runtime_retention_config
             SET cache_retention = $1
             WHERE id = TRUE AND cache_retention = '{}'::JSONB",
        )
        .bind(serde_json::to_value(cache_retention)?)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Initialize native job settings once from service configuration. Operator
    /// changes in the runtime singleton remain authoritative after initialization.
    pub async fn seed_native_config_if_empty(
        pool: &PgPool,
        native_maintenance: &crate::config::NativeMaintenanceConfig,
    ) -> Result<()> {
        native_maintenance
            .validate()
            .map_err(crate::Error::validation)?;
        Self::ensure_config(pool).await?;
        sqlx::query(
            "UPDATE runtime_retention_config SET native_maintenance = $1
             WHERE id = TRUE AND native_maintenance = '{}'::JSONB",
        )
        .bind(serde_json::to_value(native_maintenance)?)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Load the database-backed runtime retention config.
    pub async fn load_config(pool: &PgPool) -> Result<RetentionConfig> {
        Self::ensure_config(pool).await?;

        let row = sqlx::query_as::<_, RuntimeRetentionConfigRow>(
            "SELECT enabled, check_interval_seconds, batch_size, max_batches_per_target, dry_run, advisory_lock_key,
                    cache_retention, native_maintenance
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
            max_batches_per_target: row.max_batches_per_target,
            dry_run: row.dry_run,
            advisory_lock_key: row.advisory_lock_key,
            targets,
            cache_retention: serde_json::from_value(row.cache_retention)?,
            native_maintenance: serde_json::from_value(row.native_maintenance)?,
        })
    }

    /// Persist the full runtime retention config and return the stored value.
    pub async fn update_config(pool: &PgPool, config: &RetentionConfig) -> Result<RetentionConfig> {
        let mut tx = pool.begin().await?;

        sqlx::query(
            "INSERT INTO runtime_retention_config (
                id, enabled, check_interval_seconds, batch_size, max_batches_per_target, dry_run, advisory_lock_key,
                cache_retention, native_maintenance
             )
             VALUES (TRUE, $1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO UPDATE SET
                enabled = EXCLUDED.enabled,
                check_interval_seconds = EXCLUDED.check_interval_seconds,
                batch_size = EXCLUDED.batch_size,
                max_batches_per_target = EXCLUDED.max_batches_per_target,
                dry_run = EXCLUDED.dry_run,
                advisory_lock_key = EXCLUDED.advisory_lock_key,
                cache_retention = EXCLUDED.cache_retention,
                native_maintenance = EXCLUDED.native_maintenance",
        )
        .bind(config.enabled)
        .bind(config.check_interval_seconds as i64)
        .bind(config.batch_size)
        .bind(config.max_batches_per_target)
        .bind(config.dry_run)
        .bind(config.advisory_lock_key)
        .bind(serde_json::to_value(&config.cache_retention)?)
        .bind(serde_json::to_value(&config.native_maintenance)?)
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
    ) -> [(RetentionTarget, &RetentionTargetConfig); 16] {
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

    /// Expire whole daily leaves first, then delete bounded row batches. Native
    /// candidate probes count only bounded boundary/DEFAULT rows, not the contents
    /// of dropped partitions. Cancellation is checked between operations, never by
    /// dropping an in-flight delete whose commit outcome would be unknown.
    /// Failures retain confirmed progress and exclude the failing batch's
    /// unknown row count.
    pub async fn run_target_bounded(
        pool: &PgPool,
        target: RetentionTarget,
        max_age_seconds: u64,
        config: &RetentionConfig,
        is_cancelled: impl FnMut() -> bool,
    ) -> std::result::Result<RetentionTargetResult, RetentionTargetFailure> {
        Self::run_target_before(
            pool,
            target,
            retention_cutoff(max_age_seconds),
            config,
            is_cancelled,
        )
        .await
    }

    async fn run_target_before(
        pool: &PgPool,
        target: RetentionTarget,
        cutoff: DateTime<Utc>,
        config: &RetentionConfig,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> std::result::Result<RetentionTargetResult, RetentionTargetFailure> {
        let failure =
            |source, result: &RetentionTargetResult, counted: bool| RetentionTargetFailure {
                target,
                cutoff,
                candidates: counted.then_some(result.candidates),
                deleted: result.deleted,
                candidates_exact: counted && result.candidates_exact,
                partition_candidates: result.partition_candidates,
                partitions_dropped: result.partitions_dropped,
                dry_run: config.dry_run,
                source,
            };
        let mut result = RetentionTargetResult {
            target,
            cutoff: Some(cutoff),
            candidates: 0,
            deleted: 0,
            candidates_exact: false,
            partition_candidates: 0,
            partitions_dropped: 0,
            dry_run: config.dry_run,
        };
        if config.batch_size <= 0 || config.max_batches_per_target <= 0 {
            return Err(failure(
                crate::Error::Validation(
                    "retention batch_size and max_batches_per_target must be positive".to_string(),
                ),
                &result,
                false,
            ));
        }
        if is_cancelled() {
            return Ok(result);
        }
        let managed = match target {
            RetentionTarget::Events => Some(ManagedTable::Event),
            RetentionTarget::ExecutionHistory => Some(ManagedTable::ExecutionHistory),
            RetentionTarget::AuditEvents => Some(ManagedTable::AuditEvent),
            _ => None,
        }
        .filter(|_| config.native_maintenance.enabled);
        if let Some(table) = managed {
            match PartitionRepository::expire_before(
                pool,
                table,
                cutoff,
                &config.native_maintenance,
                config.dry_run,
                &mut is_cancelled,
            )
            .await
            {
                Ok(expiry) => {
                    result.partition_candidates = expiry.candidates;
                    result.partitions_dropped = expiry.partitions_dropped;
                    if expiry.cancelled {
                        return Ok(result);
                    }
                }
                Err(expiry) => {
                    result.partition_candidates = expiry.candidates.unwrap_or(0);
                    result.partitions_dropped = expiry.partitions_dropped;
                    return Err(failure(expiry.source, &result, false));
                }
            }
        }
        let (table, predicate, order_column) = Self::target_sql(target);
        if let Some(managed) = managed {
            let cap = config
                .batch_size
                .saturating_mul(config.max_batches_per_target)
                .min(i64::MAX - 1);
            let mut tx = bounded_transaction(pool, &config.native_maintenance)
                .await
                .map_err(|source| failure(source, &result, false))?;
            let col = managed.time_column();
            let fallback = managed.default_name();
            // Split DEFAULT from the boundary source range so PostgreSQL can
            // prune ALL complete old leaves, including those left by the DDL cap.
            let count_sql = format!(
                "SELECT COUNT(*)::BIGINT FROM (
                    SELECT 1 FROM ONLY {fallback} WHERE {col} < $1
                    UNION ALL
                    SELECT 1 FROM {table} WHERE {col} >= date_trunc('day', $1::timestamptz, 'UTC')
                        AND {col} < $1 AND tableoid <> to_regclass('{fallback}')::oid
                    LIMIT $2
                ) bounded",
            );
            let count = sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(cutoff)
                .bind(cap + 1)
                .fetch_one(&mut *tx)
                .await;
            result.candidates = match count {
                Ok(count) => count,
                Err(source) => {
                    tx.rollback()
                        .await
                        .map_err(|e| failure(e.into(), &result, false))?;
                    return Err(failure(source.into(), &result, false));
                }
            };
            result.candidates_exact = result.candidates <= cap;
            tx.commit()
                .await
                .map_err(|e| failure(e.into(), &result, false))?;
        } else {
            result.candidates = Self::count_target_candidates(pool, target, cutoff)
                .await
                .map_err(|source| failure(source, &result, false))?;
            result.candidates_exact = true;
        }
        if config.dry_run {
            return Ok(result);
        }

        let summary_kinds: &[SummaryKind] = match target {
            RetentionTarget::Events => &[SummaryKind::EventVolume],
            RetentionTarget::ExecutionHistory => {
                &[SummaryKind::ExecutionStatus, SummaryKind::ExecutionCreation]
            }
            RetentionTarget::WorkerHistory => &[SummaryKind::WorkerStatus],
            _ => &[],
        };

        let delete_sql = if let Some(managed) = managed {
            Self::delete_native_boundary_sql(managed)
        } else if target == RetentionTarget::Executions {
            Self::delete_executions_sql()
        } else {
            let identity = match target {
                RetentionTarget::ExecutionHistory
                | RetentionTarget::WorkerHistory
                | RetentionTarget::SensorProcessHistory => "ctid",
                _ => "id",
            };
            Self::delete_sql(table, predicate, order_column, identity)
        };
        for _ in 0..config.max_batches_per_target {
            if result.candidates == 0 {
                break;
            }
            if is_cancelled() {
                return Ok(result);
            }
            // Parent DML runs the transition-table invalidation trigger in the
            // same transaction. Locations never escape this locked statement.
            let deleted = if managed.is_some() || !summary_kinds.is_empty() {
                let mut tx = bounded_transaction(pool, &config.native_maintenance)
                    .await
                    .map_err(|source| failure(source, &result, true))?;
                let deleted = sqlx::query_scalar::<_, i64>(&delete_sql)
                    .bind(cutoff)
                    .bind(config.batch_size)
                    .fetch_one(&mut *tx)
                    .await;
                let deleted = match deleted {
                    Ok(deleted) => deleted,
                    Err(source) => {
                        tx.rollback()
                            .await
                            .map_err(|e| failure(e.into(), &result, true))?;
                        return Err(failure(source.into(), &result, true));
                    }
                };
                tx.commit()
                    .await
                    .map_err(|e| failure(e.into(), &result, true))?;
                deleted
            } else {
                sqlx::query_scalar::<_, i64>(&delete_sql)
                    .bind(cutoff)
                    .bind(config.batch_size)
                    .fetch_one(pool)
                    .await
                    .map_err(|source| failure(source.into(), &result, true))?
            };
            result.deleted += deleted;
            if deleted == 0 {
                break;
            }
        }
        if !summary_kinds.is_empty() {
            // Independent from candidate counts and the DDL enable flag. A prior
            // attempt may have committed its last raw deletion before cleanup failed.
            SummaryRepository::expire_before(
                pool,
                summary_kinds,
                cutoff,
                &config.native_maintenance,
                &mut is_cancelled,
            )
            .await
            .map_err(|source| failure(source, &result, true))?;
        }
        Ok(result)
    }

    /// Count eligible rows older than a target cutoff, without mutating data.
    pub async fn count_target_candidates(
        pool: &PgPool,
        target: RetentionTarget,
        cutoff: DateTime<Utc>,
    ) -> Result<i64> {
        let (table, predicate, _) = Self::target_sql(target);
        Self::count_predicate(pool, table, predicate, cutoff).await
    }

    // Counts and deletes share eligibility predicates. SQL identifiers are
    // internal constants, never caller-provided names.
    fn target_sql(target: RetentionTarget) -> (&'static str, &'static str, &'static str) {
        match target {
            RetentionTarget::Events => ("event", "created < $1", "created"),
            RetentionTarget::ExecutionHistory => ("execution_history", "time < $1", "time"),
            RetentionTarget::WorkerHistory => ("worker_history", "time < $1", "time"),
            RetentionTarget::SensorProcessHistory => ("sensor_process_history", "time < $1", "time"),
            RetentionTarget::AuditEvents => ("audit_event", "created < $1", "created"),
            RetentionTarget::Enforcements => ("enforcement", "created < $1 AND status <> 'created'", "created"),
            RetentionTarget::Executions => ("execution", EXECUTION_RETENTION_PREDICATE, "updated"),
            RetentionTarget::Notifications => ("notification", "created < $1", "created"),
            RetentionTarget::WebhookEventLogs => ("webhook_event_log", "created < $1", "created"),
            RetentionTarget::Inquiries => ("inquiry", "updated < $1 AND status IN ('responded', 'timeout', 'cancelled')", "updated"),
            RetentionTarget::WorkQueueItems => (
                "work_queue_item",
                "updated < $1 AND status IN ('completed', 'failed', 'skipped', 'cancelled') \
                 AND NOT EXISTS (SELECT 1 FROM workflow_task_wait wait \
                     WHERE wait.work_queue_item = work_queue_item.id AND wait.state = 'waiting')",
                "updated",
            ),
            RetentionTarget::WorkQueueDispatches => ("work_queue_dispatch", "updated < $1 AND status IN ('completed', 'failed', 'released', 'cancelled')", "updated"),
            RetentionTarget::PackTestExecutions => ("pack_test_execution", "execution_time < $1", "execution_time"),
            RetentionTarget::ExecutionAdmission => (
                "execution_admission_state",
                "updated < $1 AND NOT EXISTS (SELECT 1 FROM execution_admission_entry e WHERE e.state_id = execution_admission_state.id)",
                "updated",
            ),
            RetentionTarget::Workers => (
                "worker",
                "updated < $1 AND status IN ('inactive', 'error') AND cordoned = false AND NOT EXISTS (SELECT 1 FROM sensor_process sp WHERE sp.worker = worker.id AND sp.status IN ('starting', 'running', 'backoff'))",
                "updated",
            ),
            RetentionTarget::SensorProcesses => ("sensor_process", "updated < $1 AND status IN ('stopped', 'failed') AND active_rule_count = 0", "updated"),
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

    fn delete_sql(table: &str, predicate: &str, order_column: &str, identity: &str) -> String {
        if identity == "ctid" || table == "event" || table == "audit_event" {
            return format!(
                "WITH doomed AS MATERIALIZED (
                    SELECT tableoid, ctid FROM {table} WHERE {predicate}
                    ORDER BY {order_column} ASC, tableoid ASC, ctid ASC LIMIT $2
                    FOR UPDATE SKIP LOCKED
                 ), deleted AS (
                    DELETE FROM {table} source USING doomed
                    WHERE source.tableoid = doomed.tableoid AND source.ctid = doomed.ctid
                    RETURNING 1
                 ) SELECT COUNT(*)::BIGINT FROM deleted"
            );
        }
        format!(
            "WITH doomed AS MATERIALIZED (
                SELECT {identity} FROM {table}
                WHERE {predicate}
                ORDER BY {order_column} ASC, {identity} ASC
                LIMIT $2
                FOR UPDATE SKIP LOCKED
             ),
             deleted AS (
                DELETE FROM {table}
                WHERE {identity} IN (SELECT {identity} FROM doomed)
                RETURNING 1
             )
             SELECT COUNT(*)::BIGINT FROM deleted"
        )
    }

    fn delete_native_boundary_sql(managed: ManagedTable) -> String {
        let table = managed.table_name();
        let fallback = managed.default_name();
        let col = managed.time_column();
        format!(
            "WITH default_rows AS MATERIALIZED (
                SELECT tableoid, ctid, {col} AS sort_time FROM ONLY {fallback}
                WHERE {col} < $1 ORDER BY {col}, ctid LIMIT $2 FOR UPDATE SKIP LOCKED
             ), boundary_rows AS MATERIALIZED (
                SELECT tableoid, ctid, {col} AS sort_time FROM {table}
                WHERE {col} >= date_trunc('day', $1::timestamptz, 'UTC') AND {col} < $1
                    AND tableoid <> to_regclass('{fallback}')::oid
                ORDER BY {col}, tableoid, ctid LIMIT $2 FOR UPDATE SKIP LOCKED
             ), doomed AS MATERIALIZED (
                SELECT tableoid, ctid FROM (
                    SELECT tableoid, ctid, sort_time FROM default_rows
                    UNION ALL SELECT tableoid, ctid, sort_time FROM boundary_rows
                ) candidates ORDER BY sort_time, tableoid, ctid LIMIT $2
             ), deleted AS (
                DELETE FROM {table} source USING doomed
                WHERE source.tableoid = doomed.tableoid AND source.ctid = doomed.ctid
                RETURNING 1
             ) SELECT COUNT(*)::BIGINT FROM deleted"
        )
    }

    fn delete_executions_sql() -> String {
        format!(
            "WITH doomed AS MATERIALIZED (
                 SELECT execution.id FROM execution
                  WHERE {EXECUTION_RETENTION_PREDICATE}
                 ORDER BY updated ASC, id ASC
                 LIMIT $2
                 FOR UPDATE SKIP LOCKED
             ), deleted_outbox AS (
                 DELETE FROM workflow_log_outbox outbox
                 WHERE outbox.delivered_at IS NOT NULL
                   AND EXISTS (
                       SELECT 1 FROM workflow_execution workflow
                       JOIN doomed ON doomed.id = workflow.execution
                       WHERE workflow.id = outbox.workflow_execution
                   )
                 RETURNING 1
             ), deleted AS (
                 DELETE FROM execution
                 WHERE id IN (SELECT id FROM doomed)
                   AND (SELECT COUNT(*) FROM deleted_outbox) >= 0
                 RETURNING 1
             )
             SELECT COUNT(*)::BIGINT FROM deleted",
        )
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

        // All targets are returned regardless of max_age_seconds.
        assert_eq!(configured.len(), 16);
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

    #[tokio::test]
    async fn cancellation_before_work_never_acquires_a_connection() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let result = RetentionRepository::run_target_bounded(
            &pool,
            RetentionTarget::Events,
            60,
            &RetentionConfig::default(),
            || true,
        )
        .await
        .unwrap();
        assert_eq!((result.candidates, result.deleted), (0, 0));
        pool.close().await;
    }

    #[tokio::test]
    async fn failure_before_counting_reports_unknown_candidates_and_no_confirmed_deletes() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        pool.close().await;
        let failure = RetentionRepository::run_target_bounded(
            &pool,
            RetentionTarget::Events,
            60,
            &RetentionConfig::default(),
            || false,
        )
        .await
        .unwrap_err();
        assert_eq!(failure.target, RetentionTarget::Events);
        assert_eq!(failure.candidates, None);
        assert_eq!(failure.deleted, 0);
        assert!(matches!(
            failure.source,
            crate::Error::Database(sqlx::Error::PoolClosed)
        ));
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

    #[test]
    fn active_waits_are_excluded_from_retention_predicates() {
        assert!(EXECUTION_RETENTION_PREDICATE.contains("wait.target_execution = execution.id"));

        let queue_item_predicate =
            "updated < $1 AND NOT EXISTS (SELECT 1 FROM workflow_task_wait wait \
             WHERE wait.work_queue_item = work_queue_item.id AND wait.state = 'waiting')";
        assert!(
            RetentionRepository::count_sql("work_queue_item", queue_item_predicate)
                .contains("wait.work_queue_item = work_queue_item.id")
        );
        assert!(RetentionRepository::delete_sql(
            "work_queue_item",
            queue_item_predicate,
            "updated",
            "id"
        )
        .contains("wait.state = 'waiting'"));
    }
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod database_tests;
