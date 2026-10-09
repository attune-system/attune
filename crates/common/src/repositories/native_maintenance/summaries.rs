//! Bounded, restartable hourly materialization. Coverage, not a watermark, is progress.
//!
//! Durability policy: refresh transactions use SET LOCAL synchronous_commit = off.
//! These transactions change only derived summary/state/coverage and acknowledge
//! captured notifications atomically. Crash recovery retains that whole commit or
//! discards it, restoring acknowledgements alongside prior cache contents. Durable
//! source rows and their producer notifications remain the rebuild authority.
//! Source writes, retention, DDL, scheduling, and read/planning transactions keep
//! their normal commit policy. A later synchronous commit flushes the earlier WAL
//! prefix. This changes durability latency, not the operation deadline or isolation.

use std::{collections::VecDeque, time::Duration as StdDuration};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::{Connection, Executor, FromRow, PgPool, Postgres, Transaction};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;

use super::SummaryKind;
use crate::{
    config::{NativeMaintenanceConfig, RetentionTargetsConfig},
    Error, Result,
};

const KINDS: [SummaryKind; 4] = [
    SummaryKind::ExecutionStatus,
    SummaryKind::ExecutionCreation,
    SummaryKind::EventVolume,
    SummaryKind::WorkerStatus,
];
const MAX_SERIALIZATION_RETRIES: u32 = 2;

/// A checked UTC half-open hour. Never accepts a rounded or truncated input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryHour {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

impl SummaryHour {
    pub fn new(start: DateTime<Utc>) -> Result<Self> {
        if start.timestamp().rem_euclid(3600) != 0 || start.timestamp_subsec_nanos() != 0 {
            return Err(Error::validation(
                "summary bucket must be an exact UTC hour",
            ));
        }
        let end = start
            .checked_add_signed(Duration::hours(1))
            .ok_or_else(|| Error::validation("summary hour end is outside chrono's range"))?;
        Ok(Self { start, end })
    }

    pub fn start(self) -> DateTime<Utc> {
        self.start
    }

    pub fn end(self) -> DateTime<Utc> {
        self.end
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SummaryRefreshResult {
    pub kind: SummaryKind,
    pub bucket: DateTime<Utc>,
    pub groups_written: u64,
    pub notifications_processed: u64,
    pub serialization_retries: u32,
    /// Includes connection acquisition, lock waits, commit and awaited rollback on retries.
    pub elapsed_milliseconds: u64,
    /// The successful attempt's phases. Total elapsed also includes serialization retries.
    pub timings: SummaryRefreshTimings,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct SummaryRefreshTimings {
    pub setup_milliseconds: u64,
    pub state_capture_milliseconds: u64,
    pub delete_milliseconds: u64,
    pub replace_publish_ack_milliseconds: u64,
    pub commit_milliseconds: u64,
}

/// COMMIT was acknowledged, but the operation missed its deadline. Keep confirmed
/// progress while reporting a failed budget; never retry this committed operation.
#[derive(Debug, thiserror::Error)]
#[error("summary COMMIT acknowledged after the operation deadline: {elapsed_milliseconds} ms; committed bucket {bucket}")]
pub struct SummaryCommitDeadlineExceeded {
    pub committed: SummaryRefreshResult,
    elapsed_milliseconds: u64,
    bucket: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SummaryBucketFailure {
    pub kind: SummaryKind,
    pub bucket: Option<DateTime<Utc>>,
    pub sqlstate: Option<String>,
    pub message: String,
    pub serialization_retries: u32,
}

/// Operational failures preserve committed progress here. Validation errors return Err.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SummaryCycleResult {
    pub buckets_processed: u64,
    pub notifications_processed: u64,
    pub groups_written: u64,
    pub serialization_retries: u32,
    pub lock_failures: u64,
    pub deadline_failures: u64,
    pub cancelled: bool,
    pub budget_exhausted: bool,
    pub elapsed_milliseconds: u64,
    pub failures: Vec<SummaryBucketFailure>,
}

#[derive(Debug, Clone, FromRow, Serialize, utoipa::ToSchema)]
pub struct SummaryStatus {
    pub kind: SummaryKind,
    pub coverage_hours: i64,
    /// Extrema only. They do not assert continuous coverage.
    pub covered_since: Option<DateTime<Utc>>,
    pub covered_until: Option<DateTime<Utc>>,
    pub dirty_notifications: i64,
    pub dirty_hours: i64,
    pub oldest_dirty_bucket: Option<DateTime<Utc>>,
    pub oldest_notification: Option<DateTime<Utc>>,
    pub latest_success: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SummaryProgress {
    pub kind: SummaryKind,
    /// Oldest eligible hour start, including the partial raw-retention boundary hour.
    pub retained_since: DateTime<Utc>,
    pub bootstrap_since: DateTime<Utc>,
    pub complete_before: DateTime<Utc>,
    pub bootstrap_missing_hours: i64,
    pub next_backfill_bucket: Option<DateTime<Utc>>,
}

/// Only acknowledged cleanup commits count. These are cache units, not deleted raw rows.
#[derive(Debug, Default)]
pub struct SummaryExpiryResult {
    /// Committed hour operations, including repeated passes over capped notifications.
    pub hours_processed: i64,
    pub notifications_processed: u64,
    pub cancelled: bool,
    pub budget_exhausted: bool,
}

pub struct SummaryRepository;

impl SummaryRepository {
    /// Remove fully expired hours after raw retention, even when no raw candidates
    /// remain or native jobs are disabled. The partial cutoff hour stays dirty for
    /// normal rebuilding. Removing coverage forces raw fallback for any old rows
    /// still awaiting deletion. Large notification backlogs drain over later calls.
    pub async fn expire_before(
        pool: &PgPool,
        kinds: &[SummaryKind],
        cutoff: DateTime<Utc>,
        config: &NativeMaintenanceConfig,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<SummaryExpiryResult> {
        validate(config)?;
        let end = floor_hour(cutoff)?;
        let deadline = deadline_after(config.max_summary_cycle_milliseconds)?;
        let mut result = SummaryExpiryResult::default();
        loop {
            let mut any = false;
            for &kind in kinds {
                if is_cancelled() {
                    result.cancelled = true;
                    return Ok(result);
                }
                if result.hours_processed >= config.max_summary_buckets_per_cycle
                    || Instant::now() >= deadline
                {
                    result.budget_exhausted = true;
                    return Ok(result);
                }
                let operation_deadline =
                    deadline.min(deadline_after(config.operation_timeout_milliseconds)?);
                let mut tx = begin(pool, kind, config, operation_deadline).await?;
                let work = async {
                    statement_budget(&mut tx, operation_deadline).await?;
                    // Updating state makes a waiting old-snapshot builder retry;
                    // merely locking it would allow stale groups to be republished.
                    let locked = sqlx::query("UPDATE native_summary_state SET updated = clock_timestamp() WHERE kind = $1::text::native_summary_kind")
                        .bind(kind_parameter(kind)).execute(&mut *tx).await?;
                    if locked.rows_affected() != 1 {
                        return Err(Error::invalid_state(
                            "summary expiry requires its builder state row",
                        ));
                    }
                    statement_budget(&mut tx, operation_deadline).await?;
                    let bucket: Option<DateTime<Utc>> = sqlx::query_scalar(&format!(
                        "SELECT min(bucket) FROM (
                            (SELECT bucket FROM {} WHERE bucket < $2 ORDER BY bucket LIMIT 1)
                            UNION ALL (SELECT bucket FROM native_summary_hour WHERE kind = $1::text::native_summary_kind AND bucket < $2 ORDER BY bucket LIMIT 1)
                            UNION ALL (SELECT bucket FROM native_summary_invalidation WHERE kind = $1::text::native_summary_kind AND bucket < $2 ORDER BY bucket LIMIT 1)
                         ) heads", kind.summary_table()))
                        .bind(kind_parameter(kind)).bind(end).fetch_one(&mut *tx).await?;
                    let Some(bucket) = bucket else {
                        return Ok::<_, Error>(None);
                    };
                    statement_budget(&mut tx, operation_deadline).await?;
                    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM native_summary_invalidation WHERE kind = $1::text::native_summary_kind AND bucket = $2 ORDER BY id LIMIT $3")
                        .bind(kind_parameter(kind)).bind(bucket).bind(config.max_summary_invalidations_per_bucket)
                        .fetch_all(&mut *tx).await?;
                    statement_budget(&mut tx, operation_deadline).await?;
                    sqlx::query(&format!(
                        "DELETE FROM {} WHERE bucket = $1",
                        kind.summary_table()
                    ))
                    .bind(bucket)
                    .execute(&mut *tx)
                    .await?;
                    statement_budget(&mut tx, operation_deadline).await?;
                    sqlx::query("DELETE FROM native_summary_hour WHERE kind = $1::text::native_summary_kind AND bucket = $2")
                        .bind(kind_parameter(kind)).bind(bucket).execute(&mut *tx).await?;
                    statement_budget(&mut tx, operation_deadline).await?;
                    let acknowledged = sqlx::query("DELETE FROM native_summary_invalidation WHERE kind = $1::text::native_summary_kind AND bucket = $2 AND id = ANY($3::bigint[])")
                        .bind(kind_parameter(kind)).bind(bucket).bind(ids).execute(&mut *tx).await?.rows_affected();
                    Ok(Some(acknowledged))
                };
                let outcome = timeout_at(operation_deadline, work)
                    .await
                    .unwrap_or_else(|_| Err(Error::timeout("summary expiry deadline")));
                match outcome {
                    Ok(Some(acknowledged)) if Instant::now() < operation_deadline => {
                        if is_cancelled() {
                            rollback(tx).await?;
                            result.cancelled = true;
                            return Ok(result);
                        }
                        // Await COMMIT, including an acknowledgement after the deadline.
                        tx.commit().await?;
                        result.hours_processed += 1;
                        result.notifications_processed += acknowledged;
                        any = true;
                    }
                    outcome => {
                        rollback(tx).await?;
                        match outcome {
                            Err(error) => return Err(error),
                            Ok(Some(_)) => {
                                return Err(Error::timeout("summary expiry deadline before commit"))
                            }
                            Ok(None) => {}
                        }
                    }
                }
            }
            if !any {
                return Ok(result);
            }
        }
    }

    /// Exact raw retention boundary at the caller's clock. None means keep forever.
    pub fn source_cutoff(
        kind: SummaryKind,
        targets: &RetentionTargetsConfig,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>> {
        let age = match kind {
            SummaryKind::ExecutionStatus | SummaryKind::ExecutionCreation => {
                targets.execution_history.max_age_seconds
            }
            SummaryKind::EventVolume => targets.events.max_age_seconds,
            SummaryKind::WorkerStatus => targets.worker_history.max_age_seconds,
        };
        age.map(|seconds| {
            let duration = i64::try_from(seconds)
                .ok()
                .and_then(Duration::try_seconds)
                .ok_or_else(|| Error::validation("retention age overflow"))?;
            now.checked_sub_signed(duration)
                .ok_or_else(|| Error::validation("retention cutoff overflow"))
        })
        .transpose()
    }

    pub async fn refresh_bucket(
        pool: &PgPool,
        kind: SummaryKind,
        bucket: DateTime<Utc>,
        config: &NativeMaintenanceConfig,
    ) -> Result<SummaryRefreshResult> {
        Self::refresh_bucket_at(pool, kind, bucket, config, Utc::now()).await
    }

    /// Explicit clock entry point for deterministic callers and tests.
    pub async fn refresh_bucket_at(
        pool: &PgPool,
        kind: SummaryKind,
        bucket: DateTime<Utc>,
        config: &NativeMaintenanceConfig,
        now: DateTime<Utc>,
    ) -> Result<SummaryRefreshResult> {
        validate(config)?;
        let hour = completed_hour(bucket, now)?;
        let deadline = deadline_after(config.operation_timeout_milliseconds)?;
        refresh_with_retry(
            pool,
            kind,
            hour,
            config,
            deadline,
            &CancellationToken::new(),
        )
        .await
        .map_err(|(error, retries)| {
            tracing::warn!(?kind, %bucket, retries, %error, "hourly summary refresh failed");
            error
        })
    }

    /// Fair round-robin kinds and dirty/bootstrap/backfill lanes, with no cycle-wide tx.
    /// None retention ages mean keep forever. Expired notifications are left for the
    /// owning retention transaction; this builder never resurrects expired coverage.
    pub async fn refresh_cycle(
        pool: &PgPool,
        config: &NativeMaintenanceConfig,
        retention_targets: &RetentionTargetsConfig,
        now: DateTime<Utc>,
        cancellation: &CancellationToken,
    ) -> Result<SummaryCycleResult> {
        validate(config)?;
        let started = Instant::now();
        let deadline = deadline_after(config.max_summary_cycle_milliseconds)?;
        let mut result = SummaryCycleResult::default();
        if !config.enabled {
            return Ok(result);
        }
        // Rotate even when the cap is less than four. Restart does not reset fairness.
        let offset = (now
            .timestamp()
            .div_euclid(config.summary_interval_seconds as i64))
        .rem_euclid(4) as usize;
        let per_kind = config.max_summary_buckets_per_cycle.div_euclid(4) + 1;
        let mut queues = Vec::new();
        for index in 0..4 {
            let kind = KINDS[(index + offset) % 4];
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                break;
            }
            let planning_deadline =
                deadline.min(deadline_after(config.operation_timeout_milliseconds)?);
            match plan(
                pool,
                kind,
                config,
                retention_targets,
                now,
                SummaryPlanningBudget {
                    cap: per_kind,
                    deadline: planning_deadline,
                },
                cancellation,
            )
            .await
            {
                Ok(queue) => queues.push((kind, queue)),
                Err(error) => record_failure(&mut result, kind, None, error, 0),
            }
        }
        let mut attempted = 0;
        while attempted < config.max_summary_buckets_per_cycle {
            let mut any = false;
            for (kind, queue) in &mut queues {
                if cancellation.is_cancelled()
                    || Instant::now() >= deadline
                    || attempted >= config.max_summary_buckets_per_cycle
                {
                    break;
                }
                let Some(bucket) = queue.pop_front() else {
                    continue;
                };
                any = true;
                attempted += 1;
                let operation_deadline =
                    deadline.min(deadline_after(config.operation_timeout_milliseconds)?);
                let hour = completed_hour(bucket, now)?;
                match refresh_with_retry(
                    pool,
                    *kind,
                    hour,
                    config,
                    operation_deadline,
                    cancellation,
                )
                .await
                {
                    Ok(refresh) => {
                        result.buckets_processed += 1;
                        result.notifications_processed += refresh.notifications_processed;
                        result.groups_written += refresh.groups_written;
                        result.serialization_retries += refresh.serialization_retries;
                    }
                    Err((error, retries)) => {
                        record_failure(&mut result, *kind, Some(bucket), error, retries)
                    }
                }
            }
            if !any || cancellation.is_cancelled() || Instant::now() >= deadline {
                break;
            }
        }
        result.cancelled = cancellation.is_cancelled();
        result.budget_exhausted =
            Instant::now() >= deadline || attempted >= config.max_summary_buckets_per_cycle;
        result.elapsed_milliseconds = elapsed_ms(started);
        Ok(result)
    }

    /// One metadata query, including kinds with no coverage or no pending records.
    pub async fn status(pool: &PgPool) -> Result<Vec<SummaryStatus>> {
        Ok(sqlx::query_as(
            "WITH coverage AS (
                SELECT kind, count(*)::bigint AS coverage_hours, min(bucket) AS covered_since,
                    max(bucket) + interval '1 hour' AS covered_until, max(refreshed_at) AS latest_success
                FROM native_summary_hour GROUP BY kind
             ), dirty AS (
                SELECT kind, count(*)::bigint AS dirty_notifications,
                    count(DISTINCT bucket)::bigint AS dirty_hours,
                    min(bucket) AS oldest_dirty_bucket, min(created) AS oldest_notification
                FROM native_summary_invalidation GROUP BY kind
             )
             SELECT k.kind, coalesce(c.coverage_hours, 0)::bigint AS coverage_hours,
                 c.covered_since, c.covered_until,
                 coalesce(d.dirty_notifications, 0)::bigint AS dirty_notifications,
                 coalesce(d.dirty_hours, 0)::bigint AS dirty_hours,
                 d.oldest_dirty_bucket, d.oldest_notification, c.latest_success
             FROM unnest(enum_range(NULL::native_summary_kind)) AS k(kind)
             LEFT JOIN coverage c USING (kind) LEFT JOIN dirty d USING (kind) ORDER BY k.kind",
        ).fetch_all(pool).await?)
    }

    pub async fn progress(
        pool: &PgPool,
        kind: SummaryKind,
        config: &NativeMaintenanceConfig,
        targets: &RetentionTargetsConfig,
        now: DateTime<Utc>,
    ) -> Result<SummaryProgress> {
        validate(config)?;
        let deadline = deadline_after(config.operation_timeout_milliseconds)?;
        let mut tx = begin(pool, kind, config, deadline).await?;
        let work = async {
            let (lower, recent, end) = window(&mut tx, kind, config, targets, now).await?;
            let missing: i64 = sqlx::query_scalar(
                "SELECT count(*)::bigint FROM generate_series($2::timestamptz, $3::timestamptz - interval '1 hour', interval '1 hour') AS b(bucket)
                 WHERE NOT EXISTS (SELECT 1 FROM native_summary_hour h WHERE h.kind = $1::text::native_summary_kind AND h.bucket = b.bucket)",
            ).bind(kind_parameter(kind)).bind(recent).bind(end).fetch_one(&mut *tx).await?;
            let next = backfill_head(&mut tx, kind, lower, recent).await?;
            Ok(SummaryProgress {
                kind,
                retained_since: lower,
                bootstrap_since: recent,
                complete_before: end,
                bootstrap_missing_hours: missing,
                next_backfill_bucket: next,
            })
        };
        let outcome = timeout_at(deadline, work)
            .await
            .unwrap_or_else(|_| Err(Error::timeout("summary progress deadline")));
        rollback(tx).await?;
        outcome
    }
}

fn validate(config: &NativeMaintenanceConfig) -> Result<()> {
    config.validate().map_err(Error::validation)
}

// Bind built-in TEXT, then cast on the server. A cold pooled connection must not
// spend the hour's deadline discovering enum OIDs and labels in separate queries.
// The public/repository domain type remains SummaryKind.
fn kind_parameter(kind: SummaryKind) -> &'static str {
    match kind {
        SummaryKind::ExecutionStatus => "execution_status",
        SummaryKind::ExecutionCreation => "execution_creation",
        SummaryKind::EventVolume => "event_volume",
        SummaryKind::WorkerStatus => "worker_status",
    }
}

fn floor_hour(at: DateTime<Utc>) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(at.timestamp().div_euclid(3600) * 3600, 0)
        .ok_or_else(|| Error::validation("UTC hour is outside chrono's range"))
}

fn completed_hour(bucket: DateTime<Utc>, now: DateTime<Utc>) -> Result<SummaryHour> {
    let hour = SummaryHour::new(bucket)?;
    if hour.end > now {
        return Err(Error::validation(
            "only completed hours can be materialized",
        ));
    }
    Ok(hour)
}

fn deadline_after(milliseconds: u64) -> Result<Instant> {
    Instant::now()
        .checked_add(StdDuration::from_millis(milliseconds))
        .ok_or_else(|| Error::validation("maintenance deadline is outside the clock's range"))
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

async fn begin(
    pool: &PgPool,
    kind: SummaryKind,
    config: &NativeMaintenanceConfig,
    deadline: Instant,
) -> Result<Transaction<'static, Postgres>> {
    let mut tx = timeout_at(deadline, pool.begin())
        .await
        .map_err(|_| Error::timeout("summary connection acquisition"))??;
    let setup = async {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1);
        // Simple protocol: one round trip, no prepared plans for one-off timeout
        // values, and no SELECT/set_config snapshot before the parent lock.
        let setup_sql = format!(
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ; \
             SET LOCAL lock_timeout = '{}ms'; \
             SET LOCAL statement_timeout = '{remaining}ms'; \
             SET LOCAL plan_cache_mode = 'force_custom_plan'; \
             LOCK TABLE ONLY {} IN ACCESS SHARE MODE",
            config.lock_timeout_milliseconds,
            kind.source_table()
        );
        // The sixth execution must not build a generic Append plan across every
        // retained partition. Bucket bounds are known here; custom planning prunes
        // unrelated leaves before consulting their indexes/statistics.
        (&mut *tx).execute(setup_sql.as_str()).await?;
        Ok::<_, Error>(())
    };
    match timeout_at(deadline, setup).await {
        Ok(Ok(())) => Ok(tx),
        outcome => {
            rollback(tx).await?;
            Err(match outcome {
                Ok(Err(error)) => error,
                _ => Error::timeout("summary setup"),
            })
        }
    }
}

/// Each server statement gets the remaining transaction budget, not a renewed
/// full timeout. A cancelled SQLx future still has to await server rollback.
async fn statement_budget(tx: &mut Transaction<'_, Postgres>, deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(Error::timeout("summary transaction deadline"));
    }
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .max(1);
    let sql = format!("SET LOCAL statement_timeout = '{remaining}ms'");
    (&mut **tx).execute(sql.as_str()).await?;
    Ok(())
}

/// A dropped SQL future can leave its ErrorResponse ahead of ReadyForQuery.
/// Drain it before ROLLBACK: otherwise SQLx may return that pending error without
/// sending ROLLBACK, and Drop only queues cleanup for a later pool operation.
async fn rollback(mut tx: Transaction<'_, Postgres>) -> Result<()> {
    if let Err(error) = (*tx).flush().await {
        match error {
            sqlx::Error::Database(_) => (*tx).flush().await?,
            other => {
                // Still attempt explicit rollback on a transport/protocol failure.
                tx.rollback().await?;
                return Err(other.into());
            }
        }
    }
    tx.rollback().await?;
    Ok(())
}

async fn refresh_with_retry(
    pool: &PgPool,
    kind: SummaryKind,
    hour: SummaryHour,
    config: &NativeMaintenanceConfig,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> std::result::Result<SummaryRefreshResult, (Error, u32)> {
    let started = Instant::now();
    let mut retries = 0;
    loop {
        match refresh_once(pool, kind, hour, config, deadline, cancellation).await {
            Ok((groups_written, notifications_processed, timings)) => {
                let result = SummaryRefreshResult {
                    kind,
                    bucket: hour.start,
                    groups_written,
                    notifications_processed,
                    serialization_retries: retries,
                    elapsed_milliseconds: elapsed_ms(started),
                    timings,
                };
                if Instant::now() >= deadline {
                    return Err((
                        Error::Other(
                            SummaryCommitDeadlineExceeded {
                                elapsed_milliseconds: result.elapsed_milliseconds,
                                bucket: result.bucket,
                                committed: result,
                            }
                            .into(),
                        ),
                        retries,
                    ));
                }
                return Ok(result);
            }
            Err(error)
                if sqlstate(&error).as_deref() == Some("40001")
                    && retries < MAX_SERIALIZATION_RETRIES
                    && Instant::now() < deadline
                    && !cancellation.is_cancelled() =>
            {
                retries += 1;
                tracing::debug!(?kind, bucket = %hour.start, retries, "retrying summary serialization conflict");
            }
            Err(error) => return Err((error, retries)),
        }
    }
}

async fn refresh_once(
    pool: &PgPool,
    kind: SummaryKind,
    hour: SummaryHour,
    config: &NativeMaintenanceConfig,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(u64, u64, SummaryRefreshTimings)> {
    let setup_started = Instant::now();
    let mut tx = begin(pool, kind, config, deadline).await?;
    let mut timings = SummaryRefreshTimings {
        setup_milliseconds: elapsed_ms(setup_started),
        ..Default::default()
    };
    let mut phase = "state_capture";
    let mut phase_started = Instant::now();
    let work = async {
        // Fixed policy for this derived-cache transaction only. SET LOCAL resets
        // on commit/rollback; neither the pool session nor source producers inherit it.
        (&mut *tx)
            .execute("SET LOCAL synchronous_commit = off")
            .await?;
        statement_budget(&mut tx, deadline).await?;
        // The explicit row lock precedes the state update and capture. Updating
        // state still rejects waiting builders with stale snapshots via 40001.
        let ids: Vec<i64> = sqlx::query_scalar(
            "WITH locked AS MATERIALIZED (
                SELECT kind FROM native_summary_state WHERE kind = $1::text::native_summary_kind FOR UPDATE
             ), updated AS (
                UPDATE native_summary_state s SET updated = clock_timestamp()
                FROM locked l WHERE s.kind = l.kind RETURNING s.kind
             ) SELECT ARRAY(
                SELECT i.id FROM native_summary_invalidation i
                WHERE u.kind = $1::text::native_summary_kind
                    AND i.kind = $1::text::native_summary_kind AND i.bucket = $2
                ORDER BY i.id LIMIT $3
             ) FROM updated u",
        ).bind(kind_parameter(kind)).bind(hour.start).bind(config.max_summary_invalidations_per_bucket)
            .fetch_one(&mut *tx).await?;
        timings.state_capture_milliseconds = elapsed_ms(phase_started);
        phase = "delete";
        phase_started = Instant::now();
        statement_budget(&mut tx, deadline).await?;
        sqlx::query(&format!(
            "DELETE FROM {} WHERE bucket = $1",
            kind.summary_table()
        ))
        .bind(hour.start)
        .execute(&mut *tx)
        .await?;
        timings.delete_milliseconds = elapsed_ms(phase_started);
        phase = "replace_publish_ack";
        phase_started = Instant::now();
        statement_budget(&mut tx, deadline).await?;
        // Separate deletion avoids two modifying CTEs targeting the same summary
        // table. The remaining dependencies enforce replacement -> coverage ->
        // exact-ID acknowledgement in one bounded server statement, including zero rows.
        let (groups, acknowledged): (i64, i64) = sqlx::query_as(&format!(
            "WITH refreshed AS (
                {} RETURNING 1
             ), coverage AS (
                INSERT INTO native_summary_hour(kind,bucket,refreshed_at)
                SELECT $3::text::native_summary_kind,$1,clock_timestamp() WHERE (SELECT count(*) FROM refreshed) >= 0
                ON CONFLICT(kind,bucket) DO UPDATE SET refreshed_at = excluded.refreshed_at
                RETURNING kind
             ), acknowledged AS (
                DELETE FROM native_summary_invalidation
                WHERE kind = $3::text::native_summary_kind AND bucket = $1 AND id = ANY($4::bigint[])
                    AND EXISTS (SELECT 1 FROM coverage)
                RETURNING id
             ) SELECT (SELECT count(*) FROM refreshed)::bigint,
                      (SELECT count(*) FROM acknowledged)::bigint",
            aggregation_sql(kind),
        ))
            .bind(hour.start)
            .bind(hour.end)
            .bind(kind_parameter(kind))
            .bind(ids)
            .fetch_one(&mut *tx).await?;
        timings.replace_publish_ack_milliseconds = elapsed_ms(phase_started);
        statement_budget(&mut tx, deadline).await?;
        Ok::<_, Error>((groups as u64, acknowledged as u64))
    };
    let outcome = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(Error::invalid_state("summary refresh cancelled")),
        result = timeout_at(deadline, work) => result.unwrap_or_else(|_| Err(Error::timeout("summary transaction deadline"))),
    };
    match outcome {
        Ok(value) if Instant::now() < deadline && !cancellation.is_cancelled() => {
            // Do not cancel COMMIT and then report an unconfirmed operation as progress.
            let commit_started = Instant::now();
            tx.commit().await?;
            timings.commit_milliseconds = elapsed_ms(commit_started);
            Ok((value.0, value.1, timings))
        }
        outcome => {
            tracing::warn!(?kind, bucket = %hour.start, phase, phase_milliseconds = elapsed_ms(phase_started),
                elapsed_milliseconds = elapsed_ms(setup_started), "hourly summary transaction rolled back");
            rollback(tx).await?;
            match outcome {
                Err(error) => Err(error),
                Ok(_) if cancellation.is_cancelled() => Err(Error::invalid_state(
                    "summary refresh cancelled before commit",
                )),
                Ok(_) => Err(Error::timeout("summary deadline before commit")),
            }
        }
    }
}

fn aggregation_sql(kind: SummaryKind) -> &'static str {
    match kind {
        SummaryKind::ExecutionStatus =>
            "INSERT INTO execution_status_hourly_summary (bucket, action_ref, new_status, transition_count)
             SELECT $1, entity_ref, new_values->>'status', count(*)::bigint FROM execution_history
             WHERE time >= $1 AND time < $2 AND 'status' = ANY(changed_fields)
             GROUP BY entity_ref, new_values->>'status'",
        SummaryKind::ExecutionCreation =>
            "INSERT INTO execution_creation_hourly_summary (bucket, action_ref, execution_count)
             SELECT $1, entity_ref, count(*)::bigint FROM execution_history
             WHERE time >= $1 AND time < $2 AND operation = 'INSERT' GROUP BY entity_ref",
        SummaryKind::EventVolume =>
            "INSERT INTO event_volume_hourly_summary (bucket, trigger_ref, event_count)
             SELECT $1, trigger_ref, count(*)::bigint FROM event
             WHERE created >= $1 AND created < $2 GROUP BY trigger_ref",
        SummaryKind::WorkerStatus =>
            "INSERT INTO worker_status_hourly_summary (bucket, worker_name, new_status, transition_count)
             SELECT $1, entity_ref, new_values->>'status', count(*)::bigint FROM worker_history
             WHERE time >= $1 AND time < $2 AND 'status' = ANY(changed_fields)
             GROUP BY entity_ref, new_values->>'status'",
    }
}

fn source_predicate(kind: SummaryKind) -> &'static str {
    match kind {
        SummaryKind::ExecutionStatus | SummaryKind::WorkerStatus => {
            "'status' = ANY(changed_fields)"
        }
        SummaryKind::ExecutionCreation => "operation = 'INSERT'",
        SummaryKind::EventVolume => "TRUE",
    }
}

async fn window(
    tx: &mut Transaction<'_, Postgres>,
    kind: SummaryKind,
    config: &NativeMaintenanceConfig,
    targets: &RetentionTargetsConfig,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>, DateTime<Utc>)> {
    let end = floor_hour(now)?;
    let recent = end
        .checked_sub_signed(
            Duration::try_hours(config.summary_bootstrap_hours)
                .ok_or_else(|| Error::validation("bootstrap duration overflow"))?,
        )
        .ok_or_else(|| Error::validation("bootstrap boundary overflow"))?;
    let lower = match SummaryRepository::source_cutoff(kind, targets, now)? {
        Some(cutoff) => floor_hour(cutoff)?,
        None => {
            // An indexed first-row probe, not count/min across all historical source rows.
            let first: Option<DateTime<Utc>> = sqlx::query_scalar(&format!(
                "SELECT {} FROM {} WHERE {} AND {} < $1 ORDER BY {} LIMIT 1",
                kind.time_column(),
                kind.source_table(),
                source_predicate(kind),
                kind.time_column(),
                kind.time_column(),
            ))
            .bind(end)
            .fetch_optional(&mut **tx)
            .await?;
            // A DELETE can remove the last old source row. Its invalidation and
            // existing coverage still need rebuilding even without a raw minimum.
            let metadata_first: Option<DateTime<Utc>> = sqlx::query_scalar(
                "SELECT least(
                    (SELECT min(bucket) FROM native_summary_hour WHERE kind = $1::text::native_summary_kind AND bucket < $2),
                    (SELECT min(bucket) FROM native_summary_invalidation WHERE kind = $1::text::native_summary_kind AND bucket < $2)
                 )",
            ).bind(kind_parameter(kind)).bind(end).fetch_one(&mut **tx).await?;
            first
                .map(floor_hour)
                .transpose()?
                .into_iter()
                .chain(metadata_first)
                .fold(recent, DateTime::min)
        }
    };
    Ok((lower, recent.max(lower).min(end), end))
}

async fn backfill_head(
    tx: &mut Transaction<'_, Postgres>,
    kind: SummaryKind,
    lower: DateTime<Utc>,
    recent: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar(
        "WITH heads AS (
            SELECT $3::timestamptz - interval '1 hour' AS bucket
            UNION SELECT bucket - interval '1 hour' FROM native_summary_hour
                WHERE kind = $1::text::native_summary_kind AND bucket > $2 AND bucket <= $3
         ) SELECT bucket FROM heads b WHERE bucket >= $2 AND bucket < $3
           AND NOT EXISTS (SELECT 1 FROM native_summary_hour h WHERE h.kind = $1::text::native_summary_kind AND h.bucket = b.bucket)
         ORDER BY bucket DESC LIMIT 1",
    ).bind(kind_parameter(kind)).bind(lower).bind(recent).fetch_optional(&mut **tx).await?)
}

struct SummaryPlanningBudget {
    cap: i64,
    deadline: Instant,
}

async fn plan(
    pool: &PgPool,
    kind: SummaryKind,
    config: &NativeMaintenanceConfig,
    targets: &RetentionTargetsConfig,
    now: DateTime<Utc>,
    budget: SummaryPlanningBudget,
    cancellation: &CancellationToken,
) -> Result<VecDeque<DateTime<Utc>>> {
    let SummaryPlanningBudget { cap, deadline } = budget;
    let mut tx = begin(pool, kind, config, deadline).await?;
    let work = async {
        let (lower, recent, end) = window(&mut tx, kind, config, targets, now).await?;
        let dirty: Vec<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT bucket FROM native_summary_invalidation WHERE kind = $1::text::native_summary_kind AND bucket >= $2 AND bucket < $3
             GROUP BY bucket ORDER BY bucket LIMIT $4",
        ).bind(kind_parameter(kind)).bind(lower).bind(end).bind(cap).fetch_all(&mut *tx).await?;
        let bootstrap: Vec<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT b.bucket FROM generate_series($2::timestamptz, $3::timestamptz - interval '1 hour', interval '1 hour') AS b(bucket)
             WHERE NOT EXISTS (SELECT 1 FROM native_summary_hour h WHERE h.kind = $1::text::native_summary_kind AND h.bucket = b.bucket)
             ORDER BY b.bucket DESC LIMIT $4",
        ).bind(kind_parameter(kind)).bind(recent).bind(end).bind(cap).fetch_all(&mut *tx).await?;
        // Backfill starts after recent bootstrap, except dirty historical hours, which
        // are always eligible. Empty hours are persisted too, including ledger holes.
        let head = if bootstrap.is_empty() {
            backfill_head(&mut tx, kind, lower, recent).await?
        } else {
            None
        };
        let historical: Vec<DateTime<Utc>> = if let Some(head) = head {
            sqlx::query_scalar(
                "SELECT b.bucket FROM generate_series($2::timestamptz, greatest($3::timestamptz, $2::timestamptz - ($4::bigint - 1) * interval '1 hour'), -interval '1 hour') AS b(bucket)
                 WHERE NOT EXISTS (SELECT 1 FROM native_summary_hour h WHERE h.kind = $1::text::native_summary_kind AND h.bucket = b.bucket)",
            ).bind(kind_parameter(kind)).bind(head).bind(lower).bind(cap).fetch_all(&mut *tx).await?
        } else {
            Vec::new()
        };
        let mut lanes = [
            VecDeque::from(dirty),
            VecDeque::from(bootstrap),
            VecDeque::from(historical),
        ];
        let mut queue = VecDeque::new();
        let lane_offset = now
            .timestamp()
            .div_euclid(config.summary_interval_seconds as i64)
            .rem_euclid(3) as usize;
        while queue.len() < cap as usize {
            let mut any = false;
            for index in 0..3 {
                if let Some(bucket) = lanes[(index + lane_offset) % 3].pop_front() {
                    any = true;
                    if !queue.contains(&bucket) && queue.len() < cap as usize {
                        queue.push_back(bucket);
                    }
                }
            }
            if !any {
                break;
            }
        }
        Ok::<_, Error>(queue)
    };
    let outcome = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(Error::invalid_state("summary planning cancelled")),
        result = timeout_at(deadline, work) => result.unwrap_or_else(|_| Err(Error::timeout("summary planning deadline"))),
    };
    rollback(tx).await?;
    outcome
}

fn sqlstate(error: &Error) -> Option<String> {
    match error {
        Error::Database(sqlx::Error::Database(database)) => {
            database.code().map(|code| code.into_owned())
        }
        _ => None,
    }
}

fn record_failure(
    result: &mut SummaryCycleResult,
    kind: SummaryKind,
    bucket: Option<DateTime<Utc>>,
    error: Error,
    retries: u32,
) {
    if let Error::Other(ref other) = error {
        if let Some(overrun) = other.downcast_ref::<SummaryCommitDeadlineExceeded>() {
            result.buckets_processed += 1;
            result.notifications_processed += overrun.committed.notifications_processed;
            result.groups_written += overrun.committed.groups_written;
            result.deadline_failures += 1;
        }
    }
    let code = sqlstate(&error);
    result.serialization_retries += retries;
    result.lock_failures += u64::from(code.as_deref() == Some("55P03"));
    result.deadline_failures +=
        u64::from(code.as_deref() == Some("57014") || matches!(error, Error::Timeout(_)));
    result.failures.push(SummaryBucketFailure {
        kind,
        bucket,
        sqlstate: code,
        message: error.to_string(),
        serialization_retries: retries,
    });
}
