//! Analytics over retained PostgreSQL records.
//!
//! Dedicated hourly endpoints include buckets whose UTC start is in the inclusive
//! requested range. Dashboard raw queries use exact half-open source-time ranges.
//! Both filter source timestamps before grouping so time indexes remain usable.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Timelike, Utc};
use serde::Serialize;
use sqlx::{Executor, FromRow, PgPool, Postgres};

use crate::{models::ExecutionStatus, Result};
use serde_json::Value as JsonValue;

/// Read-only analytics. Results reflect retained raw records without refresh jobs.
pub struct AnalyticsRepository;

// ---------------------------------------------------------------------------
// Row types returned by aggregate queries
// ---------------------------------------------------------------------------

/// A single hourly bucket of execution status transitions.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ExecutionStatusBucket {
    /// Start of the 1-hour bucket
    pub bucket: DateTime<Utc>,
    /// Action ref (e.g., "core.http_request"); NULL when grouped across all actions
    pub action_ref: Option<String>,
    /// The status that was transitioned to (e.g., "completed", "failed")
    pub new_status: Option<String>,
    /// Number of transitions in this bucket
    pub transition_count: i64,
}

/// A single hourly bucket of execution throughput (creations).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ExecutionThroughputBucket {
    /// Start of the 1-hour bucket
    pub bucket: DateTime<Utc>,
    /// Action ref; NULL when grouped across all actions
    pub action_ref: Option<String>,
    /// Number of executions created in this bucket
    pub execution_count: i64,
}

/// A single hourly bucket of event volume.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct EventVolumeBucket {
    /// Start of the 1-hour bucket
    pub bucket: DateTime<Utc>,
    /// Trigger ref; NULL when grouped across all triggers
    pub trigger_ref: Option<String>,
    /// Number of events created in this bucket
    pub event_count: i64,
}

/// A single hourly bucket of worker status transitions.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WorkerStatusBucket {
    /// Start of the 1-hour bucket
    pub bucket: DateTime<Utc>,
    /// Worker name; NULL when grouped across all workers
    pub worker_name: Option<String>,
    /// The status transitioned to (e.g., "online", "offline")
    pub new_status: Option<String>,
    /// Number of transitions in this bucket
    pub transition_count: i64,
}

/// A single hourly bucket of enforcement volume.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct EnforcementVolumeBucket {
    /// Start of the 1-hour bucket
    pub bucket: DateTime<Utc>,
    /// Rule ref; NULL when grouped across all rules
    pub rule_ref: Option<String>,
    /// Number of enforcements created in this bucket
    pub enforcement_count: i64,
}

/// A single hourly bucket of execution volume (from the execution table directly).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ExecutionVolumeBucket {
    /// Start of the 1-hour bucket
    pub bucket: DateTime<Utc>,
    /// Action ref; NULL when grouped across all actions
    pub action_ref: Option<String>,
    /// Current status of executions grouped by their creation hour
    pub initial_status: Option<String>,
    /// Number of executions created in this bucket
    pub execution_count: i64,
}

/// Aggregated failure rate over a time range.
#[derive(Debug, Clone, Serialize)]
pub struct FailureRateSummary {
    /// Total status transitions to terminal states in the window
    pub total_terminal: i64,
    /// Number of transitions to "failed" status
    pub failed_count: i64,
    /// Number of transitions to "timeout" status
    pub timeout_count: i64,
    /// Number of transitions to "completed" status
    pub completed_count: i64,
    /// Failure rate as a percentage (0.0 – 100.0)
    pub failure_rate_pct: f64,
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Requested analytics range. Dedicated hourly readers include whole hours by
/// bucket start; dashboard readers use half-open source-time ranges.
#[derive(Debug, Clone)]
pub struct AnalyticsTimeRange {
    /// Range start. Defaults to 24 hours ago.
    pub since: DateTime<Utc>,
    /// Range end. Defaults to now.
    pub until: DateTime<Utc>,
}

impl Default for AnalyticsTimeRange {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            since: now - chrono::Duration::hours(24),
            until: now,
        }
    }
}

impl AnalyticsTimeRange {
    /// Source-time bounds equivalent to `bucket >= since AND bucket <= until`.
    /// A partial first hour is excluded; the last included hour is counted whole.
    fn hourly_source_bounds(&self) -> (DateTime<Utc>, DateTime<Utc>) {
        let first_hour = self
            .since
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();
        let start = if first_hour < self.since {
            first_hour + Duration::hours(1)
        } else {
            first_hour
        };
        let end = self
            .until
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap()
            + Duration::hours(1);
        (start, end)
    }

    /// Create a range covering the last N hours from now.
    pub fn last_hours(hours: i64) -> Self {
        let now = Utc::now();
        Self {
            since: now - chrono::Duration::hours(hours),
            until: now,
        }
    }

    /// Create a range covering the last N days from now.
    pub fn last_days(days: i64) -> Self {
        let now = Utc::now();
        Self {
            since: now - chrono::Duration::days(days),
            until: now,
        }
    }
}

// ---------------------------------------------------------------------------
// Repository implementation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct QueueThroughputSourceRow {
    pub bucket_start: DateTime<Utc>,
    pub queue_ref: String,
    pub completed: i64,
    pub failed: i64,
    pub skipped: i64,
    pub cancelled: i64,
    pub total_processed: i64,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueueDispatchStatsSourceRow {
    pub bucket_start: DateTime<Utc>,
    pub queue_ref: String,
    pub status: String,
    pub dispatch_count: i64,
    pub leased_item_count: i64,
    pub avg_duration_seconds: f64,
    pub max_duration_seconds: f64,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct InquiryBacklogSourceRow {
    pub pack_ref: Option<String>,
    pub assigned_to: Option<i64>,
    pub pending_count: i64,
    pub overdue_count: i64,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct InquirySlaSourceRow {
    pub bucket_start: DateTime<Utc>,
    pub pack_ref: Option<String>,
    pub assigned_to: Option<i64>,
    pub sla_target_seconds: i64,
    pub total_inquiries: i64,
    pub within_sla_count: i64,
    pub breached_count: i64,
    pub open_count: i64,
    pub compliance_rate: f64,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecutionDurationStatsSourceRow {
    pub bucket_start: DateTime<Utc>,
    pub series: String,
    pub execution_count: i64,
    pub avg_duration_seconds: f64,
    pub p50_duration_seconds: f64,
    pub p95_duration_seconds: f64,
    pub max_duration_seconds: f64,
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LatestExecutionQueryRow {
    pub action_ref: String,
    pub execution_id: i64,
    pub status: ExecutionStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub trace_tag: Option<String>,
    pub result: Option<JsonValue>,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct LastEventSourceRow {
    pub trigger_ref: String,
    pub event_id: i64,
    pub created: DateTime<Utc>,
    pub source_ref: Option<String>,
    pub rule_ref: Option<String>,
    pub trace_tag: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct LastEnforcementSourceRow {
    pub rule_ref: String,
    pub enforcement_id: i64,
    pub trigger_ref: String,
    pub status: String,
    pub created: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub event_id: Option<i64>,
}

/// Dashboard terminal-transition/event metric selection.
#[derive(Debug, Clone, Copy)]
pub enum DashboardBucketKind {
    /// Completed, failed, timeout, cancelled, and abandoned transitions by action.
    /// This is not the dedicated endpoint's history-INSERT creation metric.
    ExecutionThroughput,
    /// The same terminal attempts grouped by their destination status.
    ExecutionStatus,
    /// Event creation counts by trigger.
    EventVolume,
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketCountRow {
    pub bucket_start: DateTime<Utc>,
    pub series: String,
    pub count: i64,
}

const TERMINAL_EXECUTION_STATUSES: [&str; 5] =
    ["completed", "failed", "timeout", "cancelled", "abandoned"];
const TERMINAL_QUEUE_ITEM_STATUSES: [&str; 4] = ["completed", "failed", "skipped", "cancelled"];
const TERMINAL_QUEUE_DISPATCH_FALLBACK_STATUSES: [&str; 4] =
    ["completed", "failed", "released", "cancelled"];

impl AnalyticsRepository {
    pub async fn dashboard_queue_throughput_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        queue_refs: Option<&BTreeSet<String>>,
    ) -> Result<Vec<QueueThroughputSourceRow>> {
        let rows = if let Some(queue_refs) = queue_refs {
            let queue_refs: Vec<String> = queue_refs.iter().cloned().collect();
            sqlx::query_as::<_, (DateTime<Utc>, String, i64, i64, i64, i64, i64)>(
                r#"
            SELECT
                date_trunc('hour', updated, 'UTC') AS bucket_start,
                queue_ref,
                COUNT(*) FILTER (WHERE status::text = 'completed')::bigint AS completed,
                COUNT(*) FILTER (WHERE status::text = 'failed')::bigint AS failed,
                COUNT(*) FILTER (WHERE status::text = 'skipped')::bigint AS skipped,
                COUNT(*) FILTER (WHERE status::text = 'cancelled')::bigint AS cancelled,
                COUNT(*)::bigint AS total_processed
            FROM work_queue_item
            WHERE updated >= $1
              AND updated < $2
              AND queue_ref = ANY($3::text[])
              AND status::text = ANY($4::text[])
            GROUP BY bucket_start, queue_ref
            ORDER BY bucket_start ASC, queue_ref ASC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(queue_refs)
            .bind(TERMINAL_QUEUE_ITEM_STATUSES)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (DateTime<Utc>, String, i64, i64, i64, i64, i64)>(
                r#"
            SELECT
                date_trunc('hour', updated, 'UTC') AS bucket_start,
                queue_ref,
                COUNT(*) FILTER (WHERE status::text = 'completed')::bigint AS completed,
                COUNT(*) FILTER (WHERE status::text = 'failed')::bigint AS failed,
                COUNT(*) FILTER (WHERE status::text = 'skipped')::bigint AS skipped,
                COUNT(*) FILTER (WHERE status::text = 'cancelled')::bigint AS cancelled,
                COUNT(*)::bigint AS total_processed
            FROM work_queue_item
            WHERE updated >= $1
              AND updated < $2
              AND status::text = ANY($3::text[])
            GROUP BY bucket_start, queue_ref
            ORDER BY bucket_start ASC, queue_ref ASC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(TERMINAL_QUEUE_ITEM_STATUSES)
            .fetch_all(pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(
                |(
                    bucket_start,
                    queue_ref,
                    completed,
                    failed,
                    skipped,
                    cancelled,
                    total_processed,
                )| {
                    QueueThroughputSourceRow {
                        bucket_start,
                        queue_ref,
                        completed,
                        failed,
                        skipped,
                        cancelled,
                        total_processed,
                    }
                },
            )
            .collect())
    }

    pub async fn dashboard_queue_dispatch_stats_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        queue_refs: Option<&BTreeSet<String>>,
    ) -> Result<Vec<QueueDispatchStatsSourceRow>> {
        let rows = if let Some(queue_refs) = queue_refs {
            let queue_refs: Vec<String> = queue_refs.iter().cloned().collect();
            sqlx::query_as::<_, (DateTime<Utc>, String, String, i64, i64, f64, f64)>(
                r#"
            SELECT
                date_trunc('hour', COALESCE(e.updated, d.updated), 'UTC') AS bucket_start,
                d.queue_ref,
                COALESCE(e.status::text, d.status::text) AS status,
                COUNT(*)::bigint AS dispatch_count,
                COALESCE(SUM(d.leased_item_count), 0)::bigint AS leased_item_count,
                COALESCE(
                    AVG(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    MAX(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS max_duration_seconds
            FROM work_queue_dispatch d
            LEFT JOIN execution e ON e.id = d.execution
            WHERE COALESCE(e.updated, d.updated) >= $1
              AND COALESCE(e.updated, d.updated) < $2
              AND d.queue_ref = ANY($3::text[])
              AND (
                    e.status::text = ANY($4::text[])
                 OR (e.id IS NULL AND d.status::text = ANY($5::text[]))
              )
            GROUP BY bucket_start, d.queue_ref, COALESCE(e.status::text, d.status::text)
            ORDER BY bucket_start ASC, d.queue_ref ASC, status ASC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(queue_refs)
            .bind(TERMINAL_EXECUTION_STATUSES)
            .bind(TERMINAL_QUEUE_DISPATCH_FALLBACK_STATUSES)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (DateTime<Utc>, String, String, i64, i64, f64, f64)>(
                r#"
            SELECT
                date_trunc('hour', COALESCE(e.updated, d.updated), 'UTC') AS bucket_start,
                d.queue_ref,
                COALESCE(e.status::text, d.status::text) AS status,
                COUNT(*)::bigint AS dispatch_count,
                COALESCE(SUM(d.leased_item_count), 0)::bigint AS leased_item_count,
                COALESCE(
                    AVG(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    MAX(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS max_duration_seconds
            FROM work_queue_dispatch d
            LEFT JOIN execution e ON e.id = d.execution
            WHERE COALESCE(e.updated, d.updated) >= $1
              AND COALESCE(e.updated, d.updated) < $2
              AND (
                    e.status::text = ANY($3::text[])
                 OR (e.id IS NULL AND d.status::text = ANY($4::text[]))
              )
            GROUP BY bucket_start, d.queue_ref, COALESCE(e.status::text, d.status::text)
            ORDER BY bucket_start ASC, d.queue_ref ASC, status ASC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(TERMINAL_EXECUTION_STATUSES)
            .bind(TERMINAL_QUEUE_DISPATCH_FALLBACK_STATUSES)
            .fetch_all(pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(
                |(
                    bucket_start,
                    queue_ref,
                    status,
                    dispatch_count,
                    leased_item_count,
                    avg_duration_seconds,
                    max_duration_seconds,
                )| QueueDispatchStatsSourceRow {
                    bucket_start,
                    queue_ref,
                    status,
                    dispatch_count,
                    leased_item_count,
                    avg_duration_seconds,
                    max_duration_seconds,
                },
            )
            .collect())
    }

    pub async fn dashboard_inquiry_backlog_rows(
        pool: &PgPool,
        pack_refs: Option<&BTreeSet<String>>,
        assigned_to: Option<i64>,
    ) -> Result<Vec<InquiryBacklogSourceRow>> {
        let pack_ref_expr = r#"
        CASE
            WHEN e.action_ref IS NOT NULL AND position('.' in e.action_ref) > 0
                THEN split_part(e.action_ref, '.', 1)
            ELSE NULL
        END
    "#;

        let rows = match (pack_refs, assigned_to) {
            (Some(pack_refs), Some(assigned_to)) => {
                let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
                sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                    r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.status::text = 'pending'
                      AND i.assigned_to = $1
                      AND {pack_ref_expr} = ANY($2::text[])
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .bind(assigned_to)
                .bind(pack_refs)
                .fetch_all(pool)
                .await?
            }
            (Some(pack_refs), None) => {
                let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
                sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                    r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.status::text = 'pending'
                      AND {pack_ref_expr} = ANY($1::text[])
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .bind(pack_refs)
                .fetch_all(pool)
                .await?
            }
            (None, Some(assigned_to)) => {
                sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                    r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.status::text = 'pending'
                      AND i.assigned_to = $1
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .bind(assigned_to)
                .fetch_all(pool)
                .await?
            }
            (None, None) => {
                sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                    r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.status::text = 'pending'
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .fetch_all(pool)
                .await?
            }
        };

        Ok(rows
            .into_iter()
            .map(
                |(pack_ref, assigned_to, pending_count, overdue_count)| InquiryBacklogSourceRow {
                    pack_ref,
                    assigned_to,
                    pending_count,
                    overdue_count,
                },
            )
            .collect())
    }

    pub async fn dashboard_inquiry_sla_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        pack_refs: Option<&BTreeSet<String>>,
        assigned_to: Option<i64>,
        sla_target_seconds: i64,
    ) -> Result<Vec<InquirySlaSourceRow>> {
        let pack_ref_expr = r#"
        CASE
            WHEN e.action_ref IS NOT NULL AND position('.' in e.action_ref) > 0
                THEN split_part(e.action_ref, '.', 1)
            ELSE NULL
        END
    "#;
        let elapsed_expr = r#"
        EXTRACT(EPOCH FROM (
            COALESCE(
                i.responded_at,
                CASE WHEN i.status::text = 'timeout' THEN COALESCE(i.updated, i.timeout_at) END,
                NOW()
            ) - i.created
        ))
    "#;

        let rows = match (pack_refs, assigned_to) {
            (Some(pack_refs), Some(assigned_to)) => {
                let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
                sqlx::query_as::<
                    _,
                    (
                        DateTime<Utc>,
                        Option<String>,
                        Option<i64>,
                        i64,
                        i64,
                        i64,
                        i64,
                    ),
                >(&format!(
                    r#"
                    SELECT
                        date_trunc('hour', i.created, 'UTC') AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.created >= $2
                      AND i.created < $3
                      AND i.assigned_to = $4
                      AND {pack_ref_expr} = ANY($5::text[])
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .bind(sla_target_seconds as f64)
                .bind(range.since)
                .bind(range.until)
                .bind(assigned_to)
                .bind(pack_refs)
                .fetch_all(pool)
                .await?
            }
            (Some(pack_refs), None) => {
                let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
                sqlx::query_as::<
                    _,
                    (
                        DateTime<Utc>,
                        Option<String>,
                        Option<i64>,
                        i64,
                        i64,
                        i64,
                        i64,
                    ),
                >(&format!(
                    r#"
                    SELECT
                        date_trunc('hour', i.created, 'UTC') AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.created >= $2
                      AND i.created < $3
                      AND {pack_ref_expr} = ANY($4::text[])
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .bind(sla_target_seconds as f64)
                .bind(range.since)
                .bind(range.until)
                .bind(pack_refs)
                .fetch_all(pool)
                .await?
            }
            (None, Some(assigned_to)) => {
                sqlx::query_as::<
                    _,
                    (
                        DateTime<Utc>,
                        Option<String>,
                        Option<i64>,
                        i64,
                        i64,
                        i64,
                        i64,
                    ),
                >(&format!(
                    r#"
                    SELECT
                        date_trunc('hour', i.created, 'UTC') AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.created >= $2
                      AND i.created < $3
                      AND i.assigned_to = $4
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .bind(sla_target_seconds as f64)
                .bind(range.since)
                .bind(range.until)
                .bind(assigned_to)
                .fetch_all(pool)
                .await?
            }
            (None, None) => {
                sqlx::query_as::<
                    _,
                    (
                        DateTime<Utc>,
                        Option<String>,
                        Option<i64>,
                        i64,
                        i64,
                        i64,
                        i64,
                    ),
                >(&format!(
                    r#"
                    SELECT
                        date_trunc('hour', i.created, 'UTC') AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.created_by_execution
                    WHERE i.created >= $2
                      AND i.created < $3
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
                ))
                .bind(sla_target_seconds as f64)
                .bind(range.since)
                .bind(range.until)
                .fetch_all(pool)
                .await?
            }
        };

        Ok(rows
            .into_iter()
            .map(
                |(
                    bucket_start,
                    pack_ref,
                    assigned_to,
                    total_inquiries,
                    within_sla_count,
                    breached_count,
                    open_count,
                )| InquirySlaSourceRow {
                    bucket_start,
                    pack_ref,
                    assigned_to,
                    sla_target_seconds,
                    total_inquiries,
                    within_sla_count,
                    breached_count,
                    open_count,
                    compliance_rate: if total_inquiries > 0 {
                        within_sla_count as f64 / total_inquiries as f64
                    } else {
                        0.0
                    },
                },
            )
            .collect())
    }

    pub async fn dashboard_execution_duration_stats_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        action_refs: Option<&BTreeSet<String>>,
    ) -> Result<Vec<ExecutionDurationStatsSourceRow>> {
        let rows = if let Some(action_refs) = action_refs {
            let action_refs: Vec<String> = action_refs.iter().cloned().collect();
            sqlx::query_as::<_, (DateTime<Utc>, String, i64, f64, f64, f64, f64)>(
                r#"
            SELECT
                date_trunc('hour', updated, 'UTC') AS bucket_start,
                COALESCE(action_ref, 'unknown') AS series,
                COUNT(*)::bigint AS execution_count,
                COALESCE(
                    AVG(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.5) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p50_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.95) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p95_duration_seconds,
                COALESCE(
                    MAX(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS max_duration_seconds
            FROM execution
            WHERE updated >= $1
              AND updated < $2
              AND started_at IS NOT NULL
              AND status::text = ANY($3::text[])
              AND action_ref = ANY($4::text[])
            GROUP BY 1, 2
            ORDER BY bucket_start ASC, series ASC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(TERMINAL_EXECUTION_STATUSES)
            .bind(action_refs)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (DateTime<Utc>, String, i64, f64, f64, f64, f64)>(
                r#"
            SELECT
                date_trunc('hour', updated, 'UTC') AS bucket_start,
                COALESCE(action_ref, 'unknown') AS series,
                COUNT(*)::bigint AS execution_count,
                COALESCE(
                    AVG(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.5) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p50_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.95) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p95_duration_seconds,
                COALESCE(
                    MAX(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS max_duration_seconds
            FROM execution
            WHERE updated >= $1
              AND updated < $2
              AND started_at IS NOT NULL
              AND status::text = ANY($3::text[])
            GROUP BY 1, 2
            ORDER BY bucket_start ASC, series ASC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(TERMINAL_EXECUTION_STATUSES)
            .fetch_all(pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(
                |(
                    bucket_start,
                    series,
                    execution_count,
                    avg_duration_seconds,
                    p50_duration_seconds,
                    p95_duration_seconds,
                    max_duration_seconds,
                )| ExecutionDurationStatsSourceRow {
                    bucket_start,
                    series,
                    execution_count,
                    avg_duration_seconds,
                    p50_duration_seconds,
                    p95_duration_seconds,
                    max_duration_seconds,
                },
            )
            .collect())
    }

    pub async fn dashboard_latest_execution_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        action_refs: Option<&BTreeSet<String>>,
        statuses: &[&str],
    ) -> Result<Vec<LatestExecutionQueryRow>> {
        let action_refs: Option<Vec<String>> =
            action_refs.map(|refs| refs.iter().cloned().collect::<Vec<_>>());
        let statuses = statuses
            .iter()
            .map(|status| (*status).to_string())
            .collect::<Vec<_>>();
        sqlx::query_as::<_, LatestExecutionQueryRow>(
            r#"
        SELECT DISTINCT ON (e.action_ref)
            e.action_ref AS action_ref,
            e.id AS execution_id,
            e.status AS status,
            e.created AS created_at,
            e.started_at AS started_at,
            e.updated AS updated_at,
            e.trace_tag AS trace_tag,
            e.result AS result
        FROM execution e
        WHERE ($1::text[] IS NULL OR e.action_ref = ANY($1))
          AND e.created >= $2
          AND e.created < $3
          AND e.status::text = ANY($4::text[])
        ORDER BY e.action_ref ASC, e.created DESC, e.id DESC
        "#,
        )
        .bind(action_refs)
        .bind(range.since)
        .bind(range.until)
        .bind(statuses)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn dashboard_last_event_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        trigger_refs: Option<&BTreeSet<String>>,
    ) -> Result<Vec<LastEventSourceRow>> {
        let rows = if let Some(trigger_refs) = trigger_refs {
            let trigger_refs: Vec<String> = trigger_refs.iter().cloned().collect();
            sqlx::query_as::<
                _,
                (
                    String,
                    i64,
                    DateTime<Utc>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ),
            >(
                r#"
            SELECT trigger_ref, event_id, created, source_ref, rule_ref, trace_tag
            FROM (
                SELECT DISTINCT ON (trigger_ref)
                    trigger_ref,
                    id AS event_id,
                    created,
                    source_ref,
                    rule_ref,
                    trace_tag
                FROM event
                WHERE created >= $1
                  AND created < $2
                  AND trigger_ref = ANY($3::text[])
                ORDER BY trigger_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY trigger_ref ASC, event_id DESC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(trigger_refs)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<
                _,
                (
                    String,
                    i64,
                    DateTime<Utc>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ),
            >(
                r#"
            SELECT trigger_ref, event_id, created, source_ref, rule_ref, trace_tag
            FROM (
                SELECT DISTINCT ON (trigger_ref)
                    trigger_ref,
                    id AS event_id,
                    created,
                    source_ref,
                    rule_ref,
                    trace_tag
                FROM event
                WHERE created >= $1
                  AND created < $2
                ORDER BY trigger_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY trigger_ref ASC, event_id DESC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .fetch_all(pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(
                |(trigger_ref, event_id, created, source_ref, rule_ref, trace_tag)| {
                    LastEventSourceRow {
                        trigger_ref,
                        event_id,
                        created,
                        source_ref,
                        rule_ref,
                        trace_tag,
                    }
                },
            )
            .collect())
    }

    pub async fn dashboard_last_enforcement_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        rule_refs: Option<&BTreeSet<String>>,
    ) -> Result<Vec<LastEnforcementSourceRow>> {
        let rows = if let Some(rule_refs) = rule_refs {
            let rule_refs: Vec<String> = rule_refs.iter().cloned().collect();
            sqlx::query_as::<
                _,
                (
                    String,
                    i64,
                    String,
                    String,
                    DateTime<Utc>,
                    Option<DateTime<Utc>>,
                    Option<i64>,
                ),
            >(
                r#"
            SELECT rule_ref, enforcement_id, trigger_ref, status, created, resolved_at, event_id
            FROM (
                SELECT DISTINCT ON (rule_ref)
                    rule_ref,
                    id AS enforcement_id,
                    trigger_ref,
                    status::text AS status,
                    created,
                    resolved_at,
                    event AS event_id
                FROM enforcement
                WHERE created >= $1
                  AND created < $2
                  AND rule_ref = ANY($3::text[])
                ORDER BY rule_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY rule_ref ASC, enforcement_id DESC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .bind(rule_refs)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<
                _,
                (
                    String,
                    i64,
                    String,
                    String,
                    DateTime<Utc>,
                    Option<DateTime<Utc>>,
                    Option<i64>,
                ),
            >(
                r#"
            SELECT rule_ref, enforcement_id, trigger_ref, status, created, resolved_at, event_id
            FROM (
                SELECT DISTINCT ON (rule_ref)
                    rule_ref,
                    id AS enforcement_id,
                    trigger_ref,
                    status::text AS status,
                    created,
                    resolved_at,
                    event AS event_id
                FROM enforcement
                WHERE created >= $1
                  AND created < $2
                ORDER BY rule_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY rule_ref ASC, enforcement_id DESC
            "#,
            )
            .bind(range.since)
            .bind(range.until)
            .fetch_all(pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(
                |(
                    rule_ref,
                    enforcement_id,
                    trigger_ref,
                    status,
                    created,
                    resolved_at,
                    event_id,
                )| {
                    LastEnforcementSourceRow {
                        rule_ref,
                        enforcement_id,
                        trigger_ref,
                        status,
                        created,
                        resolved_at,
                        event_id,
                    }
                },
            )
            .collect())
    }

    pub async fn dashboard_bucket_rows(
        pool: &PgPool,
        range: &AnalyticsTimeRange,
        kind: DashboardBucketKind,
        primary_refs: Option<&BTreeSet<String>>,
    ) -> Result<Vec<BucketCountRow>> {
        // Preserve the former raw-path bucket inclusion: omit a partial first hour,
        // clip the final hour to `until`, and exclude records exactly at `until`.
        let (start, _) = range.hourly_source_bounds();
        let rows = match kind {
            DashboardBucketKind::ExecutionThroughput => {
                if let Some(action_refs) = primary_refs {
                    let action_refs: Vec<String> = action_refs.iter().cloned().collect();
                    let rows = sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                        r#"
                    SELECT
                        date_trunc('hour', time, 'UTC') AS bucket_start,
                        entity_ref AS series,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND entity_ref = ANY($3::text[])
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($4::text[])
                    GROUP BY bucket_start, entity_ref
                    ORDER BY bucket_start ASC, entity_ref ASC
                    "#,
                    )
                    .bind(start)
                    .bind(range.until)
                    .bind(action_refs)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(pool)
                    .await?;
                    rows.into_iter()
                        .map(|(bucket_start, series, count)| BucketCountRow {
                            bucket_start,
                            series,
                            count,
                        })
                        .collect()
                } else {
                    let rows = sqlx::query_as::<_, (DateTime<Utc>, i64)>(
                        r#"
                    SELECT
                        date_trunc('hour', time, 'UTC') AS bucket_start,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($3::text[])
                    GROUP BY bucket_start
                    ORDER BY bucket_start ASC
                    "#,
                    )
                    .bind(start)
                    .bind(range.until)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(pool)
                    .await?;
                    rows.into_iter()
                        .map(|(bucket_start, count)| BucketCountRow {
                            bucket_start,
                            series: "all".to_string(),
                            count,
                        })
                        .collect()
                }
            }
            DashboardBucketKind::ExecutionStatus => {
                let rows = if let Some(action_refs) = primary_refs {
                    let action_refs: Vec<String> = action_refs.iter().cloned().collect();
                    sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                        r#"
                    SELECT
                        date_trunc('hour', time, 'UTC') AS bucket_start,
                        COALESCE(new_values->>'status', 'unknown') AS series,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND entity_ref = ANY($3::text[])
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($4::text[])
                    GROUP BY bucket_start, COALESCE(new_values->>'status', 'unknown')
                    ORDER BY bucket_start ASC, series ASC
                    "#,
                    )
                    .bind(start)
                    .bind(range.until)
                    .bind(action_refs)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                        r#"
                    SELECT
                        date_trunc('hour', time, 'UTC') AS bucket_start,
                        COALESCE(new_values->>'status', 'unknown') AS series,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($3::text[])
                    GROUP BY bucket_start, COALESCE(new_values->>'status', 'unknown')
                    ORDER BY bucket_start ASC, series ASC
                    "#,
                    )
                    .bind(start)
                    .bind(range.until)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(pool)
                    .await?
                };
                rows.into_iter()
                    .map(|(bucket_start, series, count)| BucketCountRow {
                        bucket_start,
                        series,
                        count,
                    })
                    .collect()
            }
            DashboardBucketKind::EventVolume => {
                if let Some(trigger_refs) = primary_refs {
                    let trigger_refs: Vec<String> = trigger_refs.iter().cloned().collect();
                    let rows = sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                        r#"
                    SELECT
                        date_trunc('hour', created, 'UTC') AS bucket_start,
                        trigger_ref AS series,
                        COUNT(*)::bigint AS count
                    FROM event
                    WHERE created >= $1
                      AND created < $2
                      AND trigger_ref = ANY($3::text[])
                    GROUP BY bucket_start, trigger_ref
                    ORDER BY bucket_start ASC, trigger_ref ASC
                    "#,
                    )
                    .bind(start)
                    .bind(range.until)
                    .bind(trigger_refs)
                    .fetch_all(pool)
                    .await?;
                    rows.into_iter()
                        .map(|(bucket_start, series, count)| BucketCountRow {
                            bucket_start,
                            series,
                            count,
                        })
                        .collect()
                } else {
                    let rows = sqlx::query_as::<_, (DateTime<Utc>, i64)>(
                        r#"
                    SELECT
                        date_trunc('hour', created, 'UTC') AS bucket_start,
                        COUNT(*)::bigint AS count
                    FROM event
                    WHERE created >= $1
                      AND created < $2
                    GROUP BY bucket_start
                    ORDER BY bucket_start ASC
                    "#,
                    )
                    .bind(start)
                    .bind(range.until)
                    .fetch_all(pool)
                    .await?;
                    rows.into_iter()
                        .map(|(bucket_start, count)| BucketCountRow {
                            bucket_start,
                            series: "all".to_string(),
                            count,
                        })
                        .collect()
                }
            }
        };
        Ok(rows)
    }

    // =======================================================================
    // Execution status transitions
    // =======================================================================

    /// Get execution status transitions per hour, aggregated across all actions.
    ///
    /// Returns one row per (bucket, new_status) pair, ordered by bucket ascending.
    pub async fn execution_status_hourly<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
    ) -> Result<Vec<ExecutionStatusBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, ExecutionStatusBucket>(
            r#"
            SELECT
                date_trunc('hour', time, 'UTC') AS bucket,
                NULL::text AS action_ref,
                new_values->>'status' AS new_status,
                COUNT(*)::bigint AS transition_count
            FROM execution_history
            WHERE time >= $1 AND time < $2
              AND 'status' = ANY(changed_fields)
            GROUP BY bucket, new_values->>'status'
            ORDER BY bucket ASC, new_status
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Get execution status transitions per hour for a specific action.
    pub async fn execution_status_hourly_by_action<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
        action_ref: &str,
    ) -> Result<Vec<ExecutionStatusBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, ExecutionStatusBucket>(
            r#"
            SELECT
                date_trunc('hour', time, 'UTC') AS bucket,
                entity_ref AS action_ref,
                new_values->>'status' AS new_status,
                COUNT(*)::bigint AS transition_count
            FROM execution_history
            WHERE time >= $1 AND time < $2
              AND 'status' = ANY(changed_fields) AND entity_ref = $3
            GROUP BY bucket, entity_ref, new_values->>'status'
            ORDER BY bucket ASC, new_status
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(action_ref)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    // =======================================================================
    // Execution throughput
    // =======================================================================

    /// Get execution creation throughput per hour, aggregated across all actions.
    pub async fn execution_throughput_hourly<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
    ) -> Result<Vec<ExecutionThroughputBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, ExecutionThroughputBucket>(
            r#"
            SELECT
                date_trunc('hour', time, 'UTC') AS bucket,
                NULL::text AS action_ref,
                COUNT(*)::bigint AS execution_count
            FROM execution_history
            WHERE time >= $1 AND time < $2
              AND operation = 'INSERT'
            GROUP BY bucket
            ORDER BY bucket ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Get execution creation throughput per hour for a specific action.
    pub async fn execution_throughput_hourly_by_action<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
        action_ref: &str,
    ) -> Result<Vec<ExecutionThroughputBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, ExecutionThroughputBucket>(
            r#"
            SELECT
                date_trunc('hour', time, 'UTC') AS bucket,
                entity_ref AS action_ref,
                COUNT(*)::bigint AS execution_count
            FROM execution_history
            WHERE time >= $1 AND time < $2
              AND operation = 'INSERT' AND entity_ref = $3
            GROUP BY bucket, entity_ref
            ORDER BY bucket ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(action_ref)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    // =======================================================================
    // Event volume
    // =======================================================================

    /// Get event creation volume per hour, aggregated across all triggers.
    pub async fn event_volume_hourly<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
    ) -> Result<Vec<EventVolumeBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, EventVolumeBucket>(
            r#"
            SELECT
                date_trunc('hour', created, 'UTC') AS bucket,
                NULL::text AS trigger_ref,
                COUNT(*)::bigint AS event_count
            FROM event
            WHERE created >= $1 AND created < $2
            GROUP BY bucket
            ORDER BY bucket ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Get event creation volume per hour for a specific trigger.
    pub async fn event_volume_hourly_by_trigger<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
        trigger_ref: &str,
    ) -> Result<Vec<EventVolumeBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, EventVolumeBucket>(
            r#"
            SELECT
                date_trunc('hour', created, 'UTC') AS bucket,
                trigger_ref,
                COUNT(*)::bigint AS event_count
            FROM event
            WHERE created >= $1 AND created < $2
              AND trigger_ref = $3
            GROUP BY bucket, trigger_ref
            ORDER BY bucket ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(trigger_ref)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    // =======================================================================
    // Worker health
    // =======================================================================

    /// Get worker status transitions per hour, aggregated across all workers.
    pub async fn worker_status_hourly<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
    ) -> Result<Vec<WorkerStatusBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, WorkerStatusBucket>(
            r#"
            SELECT
                date_trunc('hour', time, 'UTC') AS bucket,
                NULL::text AS worker_name,
                new_values->>'status' AS new_status,
                COUNT(*)::bigint AS transition_count
            FROM worker_history
            WHERE time >= $1 AND time < $2
              AND 'status' = ANY(changed_fields)
            GROUP BY bucket, new_values->>'status'
            ORDER BY bucket ASC, new_status
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Get worker status transitions per hour for a specific worker.
    pub async fn worker_status_hourly_by_name<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
        worker_name: &str,
    ) -> Result<Vec<WorkerStatusBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, WorkerStatusBucket>(
            r#"
            SELECT
                date_trunc('hour', time, 'UTC') AS bucket,
                entity_ref AS worker_name,
                new_values->>'status' AS new_status,
                COUNT(*)::bigint AS transition_count
            FROM worker_history
            WHERE time >= $1 AND time < $2
              AND 'status' = ANY(changed_fields) AND entity_ref = $3
            GROUP BY bucket, entity_ref, new_values->>'status'
            ORDER BY bucket ASC, new_status
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(worker_name)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    // =======================================================================
    // Enforcement volume
    // =======================================================================

    /// Get enforcement creation volume per hour, aggregated across all rules.
    pub async fn enforcement_volume_hourly<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
    ) -> Result<Vec<EnforcementVolumeBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, EnforcementVolumeBucket>(
            r#"
            SELECT
                date_trunc('hour', created, 'UTC') AS bucket,
                NULL::text AS rule_ref,
                COUNT(*)::bigint AS enforcement_count
            FROM enforcement
            WHERE created >= $1 AND created < $2
            GROUP BY bucket
            ORDER BY bucket ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    /// Get enforcement creation volume per hour for a specific rule.
    pub async fn enforcement_volume_hourly_by_rule<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
        rule_ref: &str,
    ) -> Result<Vec<EnforcementVolumeBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        let rows = sqlx::query_as::<_, EnforcementVolumeBucket>(
            r#"
            SELECT
                date_trunc('hour', created, 'UTC') AS bucket,
                rule_ref,
                COUNT(*)::bigint AS enforcement_count
            FROM enforcement
            WHERE created >= $1 AND created < $2
              AND rule_ref = $3
            GROUP BY bucket, rule_ref
            ORDER BY bucket ASC
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(rule_ref)
        .fetch_all(executor)
        .await?;

        Ok(rows)
    }

    // =======================================================================
    // Execution volume (from the execution table directly)
    // =======================================================================

    /// Query retained execution rows for execution
    /// creation volume across all actions.
    pub async fn execution_volume_hourly<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
    ) -> Result<Vec<ExecutionVolumeBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        sqlx::query_as::<_, ExecutionVolumeBucket>(
            r#"
            SELECT
                date_trunc('hour', created, 'UTC') AS bucket,
                NULL::text AS action_ref,
                status::text AS initial_status,
                COUNT(*)::bigint AS execution_count
            FROM execution
            WHERE created >= $1 AND created < $2
            GROUP BY bucket, status::text
            ORDER BY bucket ASC, initial_status
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    /// Query retained execution rows filtered by
    /// a specific action ref.
    pub async fn execution_volume_hourly_by_action<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
        action_ref: &str,
    ) -> Result<Vec<ExecutionVolumeBucket>>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        sqlx::query_as::<_, ExecutionVolumeBucket>(
            r#"
            SELECT
                date_trunc('hour', created, 'UTC') AS bucket,
                action_ref,
                status::text AS initial_status,
                COUNT(*)::bigint AS execution_count
            FROM execution
            WHERE created >= $1 AND created < $2
              AND action_ref = $3
            GROUP BY bucket, action_ref, status::text
            ORDER BY bucket ASC, initial_status
            "#,
        )
        .bind(start)
        .bind(end)
        .bind(action_ref)
        .fetch_all(executor)
        .await
        .map_err(Into::into)
    }

    // =======================================================================
    // Derived analytics
    // =======================================================================

    /// Compute the execution failure rate over a time range.
    ///
    /// Counts retained history transitions to completed, failed, and timeout.
    /// Failed and timed-out attempts form the failure percentage numerator.
    pub async fn execution_failure_rate<'e, E>(
        executor: E,
        range: &AnalyticsTimeRange,
    ) -> Result<FailureRateSummary>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let (start, end) = range.hourly_source_bounds();
        // Count attempts, not distinct executions. Retries can terminate more than once.
        let rows = sqlx::query_as::<_, (Option<String>, i64)>(
            r#"
            SELECT
                new_values->>'status' AS new_status,
                COUNT(*)::bigint AS cnt
            FROM execution_history
            WHERE time >= $1 AND time < $2
              AND 'status' = ANY(changed_fields)
              AND new_values->>'status' IN ('completed', 'failed', 'timeout')
            GROUP BY new_values->>'status'
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(executor)
        .await?;

        let mut completed: i64 = 0;
        let mut failed: i64 = 0;
        let mut timeout: i64 = 0;

        for (status, count) in &rows {
            match status.as_deref() {
                Some("completed") => completed = *count,
                Some("failed") => failed = *count,
                Some("timeout") => timeout = *count,
                _ => {}
            }
        }

        let total_terminal = completed + failed + timeout;
        let failure_rate_pct = if total_terminal > 0 {
            ((failed + timeout) as f64 / total_terminal as f64) * 100.0
        } else {
            0.0
        };

        Ok(FailureRateSummary {
            total_terminal,
            failed_count: failed,
            timeout_count: timeout,
            completed_count: completed,
            failure_rate_pct,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analytics_time_range_default() {
        let range = AnalyticsTimeRange::default();
        let diff = range.until - range.since;
        // Should be approximately 24 hours
        assert!((diff.num_hours() - 24).abs() <= 1);
    }

    #[test]
    fn test_analytics_time_range_last_hours() {
        let range = AnalyticsTimeRange::last_hours(6);
        let diff = range.until - range.since;
        assert!((diff.num_hours() - 6).abs() <= 1);
    }

    #[test]
    fn test_analytics_time_range_last_days() {
        let range = AnalyticsTimeRange::last_days(7);
        let diff = range.until - range.since;
        assert!((diff.num_days() - 7).abs() <= 1);
    }

    #[test]
    fn test_failure_rate_summary_zero_total() {
        let summary = FailureRateSummary {
            total_terminal: 0,
            failed_count: 0,
            timeout_count: 0,
            completed_count: 0,
            failure_rate_pct: 0.0,
        };
        assert_eq!(summary.failure_rate_pct, 0.0);
    }

    #[test]
    fn test_failure_rate_calculation() {
        // 80 completed, 15 failed, 5 timeout → 20% failure rate
        let total = 80 + 15 + 5;
        let rate = ((15 + 5) as f64 / total as f64) * 100.0;
        assert!((rate - 20.0).abs() < 0.01);
    }
}
