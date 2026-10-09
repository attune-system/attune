//! Bounded native partition operations. Each DDL operation is one server-side
//! statement and one transaction. A timeout awaits rollback before returning.

use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use super::ManagedTable;
use crate::{config::NativeMaintenanceConfig, Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PartitionRepairOutcome {
    Applied {
        rows_moved: i64,
    },
    AlreadyPresent,
    /// A bounded probe found at least this many rows. All remain in DEFAULT.
    DeferredOverBudget {
        rows_at_least: i64,
    },
    DeferredBusy,
    DeferredDeadline,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PartitionCycleResult {
    pub attempted: i64,
    pub created: i64,
    pub rows_moved: i64,
    pub deferred_over_budget: i64,
    pub lock_retries: i64,
    pub deferred_deadline: i64,
    pub budget_exhausted: bool,
}

/// A mid-cycle failure cannot erase earlier acknowledged repair commits.
#[derive(Debug, thiserror::Error)]
#[error("Partition reconciliation failed after {partial:?}: {source}")]
pub struct PartitionCycleFailure {
    pub partial: PartitionCycleResult,
    #[source]
    pub source: Error,
}

impl From<PartitionCycleFailure> for Error {
    fn from(failure: PartitionCycleFailure) -> Self {
        Self::Other(failure.into())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PartitionExpiryResult {
    /// Metadata count of eligible leaves, never a raw-row count.
    pub candidates: i64,
    pub partitions_dropped: i64,
    pub dry_run: bool,
    pub cancelled: bool,
    pub budget_exhausted: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("Partition expiry failed after {partitions_dropped} confirmed drops: {source}")]
pub struct PartitionExpiryFailure {
    pub candidates: Option<i64>,
    /// Only positively acknowledged commits count. A failed COMMIT is unknown.
    pub partitions_dropped: i64,
    #[source]
    pub source: Error,
}

#[derive(Debug, Clone, FromRow)]
pub struct PartitionDescriptor {
    pub id: i64,
    pub parent: ManagedTable,
    pub partition_name: String,
    pub lower_bound: DateTime<Utc>,
    pub upper_bound: DateTime<Utc>,
    pub created: DateTime<Utc>,
}

/// Catalog-verified status. DEFAULT counts stop at repair_cap + 1; they are
/// lower bounds when default_count_exact is false, never full backlog scans.
#[derive(Debug, Clone, FromRow, Serialize, utoipa::ToSchema)]
pub struct PartitionStatus {
    pub parent: ManagedTable,
    pub registered_partitions: i64,
    pub future_partitions: i64,
    pub missing_future_partitions: i64,
    pub default_rows_at_least: i64,
    pub default_count_exact: bool,
    pub oldest_default_day: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct RepairRow {
    outcome: String,
    rows_moved: i64,
}

pub struct PartitionRepository;

impl PartitionRepository {
    pub async fn status(
        pool: &PgPool,
        config: &NativeMaintenanceConfig,
        now: DateTime<Utc>,
    ) -> Result<Vec<PartitionStatus>> {
        validate(config)?;
        let mut statuses = Vec::with_capacity(3);
        for parent in ManagedTable::ALL {
            let mut tx = bounded_transaction(pool, config).await?;
            let row = sqlx::query_as::<_, PartitionStatus>(
                "SELECT $1::native_partition_parent AS parent, registered_partitions, future_partitions,
                    missing_future_partitions, default_rows_at_least, default_count_exact, oldest_default_day
                 FROM native_partition_status($1, $2, $3, $4)",
            ).bind(parent).bind(utc_day(now)).bind(config.partition_lookahead_days).bind(config.default_repair_row_limit)
                .fetch_one(&mut *tx).await;
            let row = match row {
                Ok(row) => row,
                Err(error) => {
                    tx.rollback().await?;
                    return Err(error.into());
                }
            };
            tx.commit().await?;
            statuses.push(row);
        }
        Ok(statuses)
    }
    /// Persist round-robin parent and circular day reservations before each
    /// attempt, including deferrals. Future and DEFAULT days share that rotation.
    /// The caller holds the supervisor's full-cycle leader lock.
    pub async fn reconcile(
        pool: &PgPool,
        config: &NativeMaintenanceConfig,
        now: DateTime<Utc>,
    ) -> std::result::Result<PartitionCycleResult, PartitionCycleFailure> {
        let mut result = PartitionCycleResult::default();
        let failure = |source, partial: &PartitionCycleResult| PartitionCycleFailure {
            partial: partial.clone(),
            source,
        };
        validate(config).map_err(|e| failure(e, &result))?;
        if !config.enabled {
            return Ok(result);
        }
        let deadline =
            Instant::now() + StdDuration::from_millis(config.max_partition_cycle_milliseconds);
        let today = utc_day(now);
        let days = config
            .partition_lookahead_days
            .checked_add(1)
            .and_then(Duration::try_days)
            .ok_or_else(|| {
                failure(
                    Error::Validation("partition horizon exceeds timestamp range".into()),
                    &result,
                )
            })?;
        let horizon = today.checked_add_signed(days).ok_or_else(|| {
            failure(
                Error::Validation("partition horizon exceeds timestamp range".into()),
                &result,
            )
        })?;
        let mut empty_parents = 0;
        while empty_parents < ManagedTable::ALL.len() {
            if exhausted(&result, config, deadline) {
                result.budget_exhausted = true;
                return Ok(result);
            }
            let remaining = remaining_config(config, deadline);
            let mut tx = bounded_transaction(pool, &remaining)
                .await
                .map_err(|e| failure(e, &result))?;
            // Reserve the parent separately: a failed discovery/lock attempt also
            // rotates on restart rather than trapping every leader on that source.
            let index = sqlx::query_scalar::<_, i16>(
                "UPDATE native_partition_reconcile_state SET next_parent = (next_parent + 1) % 3
                 WHERE id = TRUE RETURNING ((next_parent + 2) % 3)::smallint",
            )
            .fetch_one(&mut *tx)
            .await;
            let index = match index {
                Ok(index) => index,
                Err(error) => {
                    tx.rollback()
                        .await
                        .map_err(|e| failure(e.into(), &result))?;
                    return Err(failure(error.into(), &result));
                }
            };
            tx.commit().await.map_err(|e| failure(e.into(), &result))?;
            let table = ManagedTable::ALL[index as usize];
            let mut tx = bounded_transaction(pool, &remaining_config(config, deadline))
                .await
                .map_err(|e| failure(e, &result))?;
            let day = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
                "SELECT native_partition_next_day($1, $2, $3,
                    (SELECT last_day FROM native_partition_reconcile_cursor WHERE parent = $1 FOR UPDATE))",
            )
            .bind(table)
            .bind(today)
            .bind(horizon)
            .fetch_one(&mut *tx)
            .await;
            let day = match day {
                Ok(day) => day,
                Err(error) => {
                    tx.rollback()
                        .await
                        .map_err(|e| failure(e.into(), &result))?;
                    match deferred(&error) {
                        Some(outcome) => {
                            record(&mut result, outcome);
                            continue;
                        }
                        None => return Err(failure(error.into(), &result)),
                    }
                }
            };
            if let Some(day) = day {
                let saved = sqlx::query(
                    "UPDATE native_partition_reconcile_cursor SET last_day = $2 WHERE parent = $1",
                )
                .bind(table)
                .bind(day)
                .execute(&mut *tx)
                .await;
                if let Err(error) = saved {
                    tx.rollback()
                        .await
                        .map_err(|e| failure(e.into(), &result))?;
                    return Err(failure(error.into(), &result));
                }
                tx.commit().await.map_err(|e| failure(e.into(), &result))?;
                empty_parents = 0;
                if exhausted(&result, config, deadline) {
                    result.budget_exhausted = true;
                    return Ok(result);
                }
                let outcome =
                    Self::ensure_day(pool, table, day, &remaining_config(config, deadline))
                        .await
                        .map_err(|e| failure(e, &result))?;
                record(&mut result, outcome);
            } else {
                tx.commit().await.map_err(|e| failure(e.into(), &result))?;
                empty_parents += 1;
            }
        }
        Ok(result)
    }

    /// Repair a whole UTC day atomically. Oversized/busy days stay parent-visible.
    pub async fn ensure_day(
        pool: &PgPool,
        table: ManagedTable,
        day: DateTime<Utc>,
        config: &NativeMaintenanceConfig,
    ) -> Result<PartitionRepairOutcome> {
        validate(config)?;
        if utc_day(day) != day {
            return Err(Error::Validation(
                "partition day must be UTC midnight".into(),
            ));
        }
        let mut tx = bounded_transaction(pool, config).await?;
        let row = sqlx::query_as::<_, RepairRow>(
            "SELECT outcome, rows_moved FROM native_partition_ensure_day($1, $2, $3)",
        )
        .bind(table)
        .bind(day)
        .bind(config.default_repair_row_limit)
        .fetch_one(&mut *tx)
        .await;
        match row {
            Ok(row) => {
                let outcome = match row.outcome.as_str() {
                    "applied" => PartitionRepairOutcome::Applied {
                        rows_moved: row.rows_moved,
                    },
                    "already_present" => PartitionRepairOutcome::AlreadyPresent,
                    "deferred_over_budget" => PartitionRepairOutcome::DeferredOverBudget {
                        rows_at_least: row.rows_moved,
                    },
                    value => {
                        tx.rollback().await?;
                        return Err(Error::InvalidState(format!(
                            "unknown partition outcome {value}"
                        )));
                    }
                };
                tx.commit().await?;
                Ok(outcome)
            }
            Err(error) => {
                tx.rollback().await?;
                deferred(&error).ok_or_else(|| error.into())
            }
        }
    }

    /// Drop only complete expired daily leaves, with materialization removal in
    /// the very same statement/transaction. Preserves acknowledged prior drops.
    pub async fn expire_before(
        pool: &PgPool,
        table: ManagedTable,
        cutoff: DateTime<Utc>,
        config: &NativeMaintenanceConfig,
        dry_run: bool,
        mut cancellation: impl FnMut() -> bool,
    ) -> std::result::Result<PartitionExpiryResult, PartitionExpiryFailure> {
        let failure = |source, candidates, partitions_dropped| PartitionExpiryFailure {
            source,
            candidates,
            partitions_dropped,
        };
        validate(config).map_err(|e| failure(e, None, 0))?;
        let mut result = PartitionExpiryResult {
            dry_run,
            ..Default::default()
        };
        if cancellation() {
            result.cancelled = true;
            return Ok(result);
        }
        let deadline =
            Instant::now() + StdDuration::from_millis(config.max_partition_cycle_milliseconds);
        let mut tx = bounded_transaction(pool, config)
            .await
            .map_err(|e| failure(e, None, 0))?;
        let ids = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM native_partition_registry WHERE parent = $1 AND upper_bound <= $2
             ORDER BY upper_bound, id LIMIT $3",
        )
        .bind(table)
        .bind(cutoff)
        .bind(config.max_partition_operations_per_cycle)
        .fetch_all(&mut *tx)
        .await;
        let ids = match ids {
            Ok(ids) => ids,
            Err(e) => {
                tx.rollback()
                    .await
                    .map_err(|e| failure(e.into(), None, 0))?;
                return Err(failure(e.into(), None, 0));
            }
        };
        tx.commit().await.map_err(|e| failure(e.into(), None, 0))?;
        result.candidates = ids.len() as i64;
        result.budget_exhausted = result.candidates == config.max_partition_operations_per_cycle;
        if dry_run {
            return Ok(result);
        }
        for id in ids {
            if cancellation() {
                result.cancelled = true;
                break;
            }
            if Instant::now() >= deadline {
                result.budget_exhausted = true;
                break;
            }
            let mut tx = bounded_transaction(pool, &remaining_config(config, deadline))
                .await
                .map_err(|e| failure(e, Some(result.candidates), result.partitions_dropped))?;
            let dropped =
                sqlx::query_scalar::<_, bool>("SELECT native_partition_expire($1, $2, $3)")
                    .bind(table)
                    .bind(id)
                    .bind(cutoff)
                    .fetch_one(&mut *tx)
                    .await;
            let dropped = match dropped {
                Ok(value) => value,
                Err(e) => {
                    tx.rollback().await.map_err(|e| {
                        failure(e.into(), Some(result.candidates), result.partitions_dropped)
                    })?;
                    return Err(failure(
                        e.into(),
                        Some(result.candidates),
                        result.partitions_dropped,
                    ));
                }
            };
            tx.commit().await.map_err(|e| {
                failure(e.into(), Some(result.candidates), result.partitions_dropped)
            })?;
            result.partitions_dropped += i64::from(dropped);
        }
        Ok(result)
    }

    /// Bounded catalog-backed operator inventory. Does not read raw row totals.
    pub async fn inventory(
        pool: &PgPool,
        limit: i64,
        config: &NativeMaintenanceConfig,
    ) -> Result<Vec<PartitionDescriptor>> {
        validate(config)?;
        if limit <= 0 {
            return Err(Error::Validation(
                "partition inventory limit must be positive".into(),
            ));
        }
        let mut tx = bounded_transaction(pool, config).await?;
        let rows = sqlx::query_as::<_, PartitionDescriptor>(
            "SELECT id, parent, partition_name, lower_bound, upper_bound, created
             FROM native_partition_registry ORDER BY parent, lower_bound LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await;
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) => {
                tx.rollback().await?;
                return Err(error.into());
            }
        };
        tx.commit().await?;
        Ok(rows)
    }
}

pub fn utc_day(time: DateTime<Utc>) -> DateTime<Utc> {
    time.date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc()
}

fn validate(config: &NativeMaintenanceConfig) -> Result<()> {
    config.validate().map_err(Error::Validation)?;
    if config.default_repair_row_limit == i64::MAX {
        return Err(Error::Validation(
            "repair row cap must leave room for its overflow probe".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn bounded_transaction<'a>(
    pool: &'a PgPool,
    config: &NativeMaintenanceConfig,
) -> Result<Transaction<'a, Postgres>> {
    let mut tx = tokio::time::timeout(
        StdDuration::from_millis(config.operation_timeout_milliseconds),
        pool.begin(),
    )
    .await
    .map_err(|_| Error::Database(sqlx::Error::PoolTimedOut))??;
    let configured = sqlx::query("SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true), set_config('TimeZone', 'UTC', true)")
        .bind(format!("{}ms", config.lock_timeout_milliseconds))
        .bind(format!("{}ms", config.operation_timeout_milliseconds))
        .execute(&mut *tx).await;
    if let Err(error) = configured {
        tx.rollback().await?;
        return Err(error.into());
    }
    Ok(tx)
}

fn deferred(error: &sqlx::Error) -> Option<PartitionRepairOutcome> {
    match error.as_database_error().and_then(|e| e.code()).as_deref() {
        Some("55P03") => Some(PartitionRepairOutcome::DeferredBusy),
        Some("57014") => Some(PartitionRepairOutcome::DeferredDeadline),
        _ => None,
    }
}

fn remaining_config(
    config: &NativeMaintenanceConfig,
    deadline: Instant,
) -> NativeMaintenanceConfig {
    let mut remaining = config.clone();
    remaining.operation_timeout_milliseconds = config.operation_timeout_milliseconds.min(
        deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1) as u64,
    );
    remaining.lock_timeout_milliseconds = remaining
        .lock_timeout_milliseconds
        .min(remaining.operation_timeout_milliseconds);
    remaining
}

fn exhausted(
    result: &PartitionCycleResult,
    config: &NativeMaintenanceConfig,
    deadline: Instant,
) -> bool {
    result.attempted >= config.max_partition_operations_per_cycle || Instant::now() >= deadline
}

fn record(result: &mut PartitionCycleResult, outcome: PartitionRepairOutcome) {
    // Existing verified days consume elapsed time, but not the DDL operation
    // budget. Otherwise a small cap would forever repeat the same initial days.
    if outcome != PartitionRepairOutcome::AlreadyPresent {
        result.attempted += 1;
    }
    match outcome {
        PartitionRepairOutcome::Applied { rows_moved } => {
            result.created += 1;
            result.rows_moved += rows_moved;
        }
        PartitionRepairOutcome::AlreadyPresent => {}
        PartitionRepairOutcome::DeferredOverBudget { .. } => result.deferred_over_budget += 1,
        PartitionRepairOutcome::DeferredBusy => result.lock_retries += 1,
        PartitionRepairOutcome::DeferredDeadline => result.deferred_deadline += 1,
    }
}
