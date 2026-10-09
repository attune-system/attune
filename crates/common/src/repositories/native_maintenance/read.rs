//! Source-locked, snapshot-consistent hourly analytics. The ledger, not an
//! envelope or notification sequence watermark, proves each clean full hour.

use chrono::{DateTime, Duration, Timelike, Utc};
use serde::Serialize;
use sqlx::{Acquire, Connection, Executor, FromRow, PgConnection, Postgres};

use super::SummaryKind;
use crate::Result;

const MAX_COVERED_HOURS: i64 = 4096;
const MAX_RANGES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    RawOnly,
    SummaryOnly,
    SummaryPlusRaw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadMetadata {
    pub mode: ReadMode,
    /// Exact clean full-hour ranges used by this request. Gaps are not covered.
    pub summary_ranges: Vec<ReadRange>,
    pub raw_ranges: Vec<ReadRange>,
    /// Oldest refresh among the hours actually used. Not a global watermark.
    pub oldest_refresh: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct AnalyticsRead<T> {
    pub data: T,
    pub metadata: ReadMetadata,
}

impl<T> AnalyticsRead<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> AnalyticsRead<U> {
        AnalyticsRead {
            data: f(self.data),
            metadata: self.metadata,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct ReadRow {
    pub bucket: DateTime<Utc>,
    pub reference: Option<String>,
    pub status: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub struct ReadPlan {
    pub metadata: ReadMetadata,
}

fn hour(value: DateTime<Utc>) -> DateTime<Utc> {
    value
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
}

impl ReadPlan {
    fn raw(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            metadata: ReadMetadata {
                mode: ReadMode::RawOnly,
                summary_ranges: Vec::new(),
                raw_ranges: if start < end {
                    vec![ReadRange { start, end }]
                } else {
                    Vec::new()
                },
                oldest_refresh: None,
            },
        }
    }

    /// Input is ordered clean ledger hours from this transaction's snapshot.
    /// Work is bounded by ledger rows, never by the requested range's duration.
    fn build(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        now: DateTime<Utc>,
        clean: &[(DateTime<Utc>, DateTime<Utc>)],
    ) -> Self {
        if clean.len() > MAX_COVERED_HOURS as usize || start >= end {
            return Self::raw(start, end);
        }
        let mut summary_ranges: Vec<ReadRange> = Vec::new();
        let mut oldest_refresh = None;
        for &(bucket, refreshed) in clean {
            let upper = bucket + Duration::hours(1);
            if bucket < start || upper > end || upper > hour(now) {
                continue;
            }
            oldest_refresh =
                Some(oldest_refresh.map_or(refreshed, |old: DateTime<Utc>| old.min(refreshed)));
            if let Some(last) = summary_ranges.last_mut().filter(|last| last.end == bucket) {
                last.end = upper;
            } else {
                summary_ranges.push(ReadRange {
                    start: bucket,
                    end: upper,
                });
            }
            if summary_ranges.len() > MAX_RANGES {
                return Self::raw(start, end);
            }
        }
        let mut raw_ranges = Vec::new();
        let mut cursor = start;
        for range in &summary_ranges {
            if cursor < range.start {
                raw_ranges.push(ReadRange {
                    start: cursor,
                    end: range.start,
                });
            }
            cursor = range.end;
        }
        if cursor < end {
            raw_ranges.push(ReadRange { start: cursor, end });
        }
        if raw_ranges.len() > MAX_RANGES {
            return Self::raw(start, end);
        }
        let mode = if summary_ranges.is_empty() {
            ReadMode::RawOnly
        } else if raw_ranges.is_empty() {
            ReadMode::SummaryOnly
        } else {
            ReadMode::SummaryPlusRaw
        };
        Self {
            metadata: ReadMetadata {
                mode,
                summary_ranges,
                raw_ranges,
                oldest_refresh,
            },
        }
    }
}

// Only missing maintenance relations or insufficient maintenance privileges are
// availability failures. Undefined columns, bad SQL, and decoding errors escape.
fn unavailable(error: &sqlx::Error) -> bool {
    if matches!(error, sqlx::Error::TypeNotFound { type_name } if type_name == "native_summary_kind")
    {
        return true;
    }
    matches!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42P01" | "42501")
    )
}

fn columns(kind: SummaryKind) -> (&'static str, &'static str, &'static str, &'static str) {
    match kind {
        SummaryKind::ExecutionStatus => (
            "entity_ref",
            "new_values->>'status'",
            "'status' = ANY(changed_fields)",
            "transition_count",
        ),
        SummaryKind::ExecutionCreation => (
            "entity_ref",
            "NULL::text",
            "operation = 'INSERT'",
            "execution_count",
        ),
        SummaryKind::EventVolume => ("trigger_ref", "NULL::text", "TRUE", "event_count"),
        SummaryKind::WorkerStatus => (
            "entity_ref",
            "new_values->>'status'",
            "'status' = ANY(changed_fields)",
            "transition_count",
        ),
    }
}

fn summary_ref(kind: SummaryKind) -> &'static str {
    match kind {
        SummaryKind::ExecutionStatus | SummaryKind::ExecutionCreation => "action_ref",
        SummaryKind::EventVolume => "trigger_ref",
        SummaryKind::WorkerStatus => "worker_name",
    }
}

/// One set-based query for all coalesced ranges. Each UNION arm has source-time
/// index bounds; no application path filters an hourly view's computed bucket.
async fn rows(
    connection: &mut PgConnection,
    kind: SummaryKind,
    ranges: &[ReadRange],
    summary: bool,
    refs: Option<&[String]>,
    statuses: Option<&[String]>,
    group_refs: bool,
) -> std::result::Result<Vec<ReadRow>, sqlx::Error> {
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    row_query(kind, ranges, summary, refs, statuses, group_refs)
        .build_query_as::<ReadRow>()
        .fetch_all(connection)
        .await
}

fn row_query<'a>(
    kind: SummaryKind,
    ranges: &[ReadRange],
    summary: bool,
    refs: Option<&'a [String]>,
    statuses: Option<&'a [String]>,
    group_refs: bool,
) -> sqlx::QueryBuilder<'a, Postgres> {
    let (raw_ref, raw_status, predicate, count) = columns(kind);
    let reference = if summary { summary_ref(kind) } else { raw_ref };
    let status = if summary
        && matches!(
            kind,
            SummaryKind::ExecutionStatus | SummaryKind::WorkerStatus
        ) {
        "new_status"
    } else {
        raw_status
    };
    let time = if summary {
        "bucket"
    } else {
        kind.time_column()
    };
    let bucket = if summary {
        "bucket".to_string()
    } else {
        format!("date_trunc('hour', {time}, 'UTC')")
    };
    let table = if summary {
        kind.summary_table()
    } else {
        kind.source_table()
    };
    let aggregate = if summary {
        format!("SUM({count})::bigint")
    } else {
        "COUNT(*)::bigint".to_string()
    };
    let mut query = sqlx::QueryBuilder::<Postgres>::new("");
    for (index, range) in ranges.iter().enumerate() {
        if index > 0 {
            query.push(" UNION ALL ");
        }
        query.push(format!("SELECT {bucket} AS bucket, "));
        if group_refs {
            query.push(reference);
        } else {
            query.push("NULL::text");
        }
        query.push(format!(
            " AS reference, {status} AS status, {aggregate} AS count FROM {table} WHERE {time} >= "
        ));
        query
            .push_bind(range.start)
            .push(format!(" AND {time} < "))
            .push_bind(range.end);
        if !summary {
            query.push(format!(" AND ({predicate})"));
        }
        if let Some(refs) = refs {
            query
                .push(format!(" AND {reference} = ANY("))
                .push_bind(refs)
                .push("::text[])");
        }
        if let Some(statuses) = statuses {
            query
                .push(format!(" AND {status} = ANY("))
                .push_bind(statuses)
                .push("::text[])");
        }
        query.push(" GROUP BY 1, 2, 3");
    }
    query
}

#[derive(FromRow)]
struct CoveredRead {
    read_time: DateTime<Utc>,
    covered_buckets: Vec<DateTime<Utc>>,
    refreshed_at: Vec<DateTime<Utc>>,
    summary_ready: bool,
    buckets: Vec<DateTime<Utc>>,
    references: Vec<Option<String>>,
    statuses: Vec<Option<String>>,
    counts: Vec<i64>,
}

fn covered_query<'a>(
    kind: SummaryKind,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    refs: Option<&'a [String]>,
    statuses: Option<&'a [String]>,
    group_refs: bool,
) -> sqlx::QueryBuilder<'a, Postgres> {
    let mut query = sqlx::QueryBuilder::<Postgres>::new("WITH request AS (SELECT ");
    query
        .push_bind(kind)
        .push("::native_summary_kind AS kind, ")
        .push_bind(start)
        .push("::timestamptz AS since, ")
        .push_bind(end)
        .push("::timestamptz AS until), ");
    query.push("clean AS MATERIALIZED (SELECT h.bucket, h.refreshed_at FROM native_summary_hour h CROSS JOIN request r \
        WHERE h.kind = r.kind AND h.bucket >= r.since \
        AND h.bucket < LEAST(r.until, date_trunc('hour', CURRENT_TIMESTAMP, 'UTC')) \
        AND EXISTS (SELECT 1 FROM native_summary_state s WHERE s.kind = h.kind) \
        AND NOT EXISTS (SELECT 1 FROM native_summary_invalidation i WHERE i.kind = h.kind AND i.bucket = h.bucket) \
        ORDER BY h.bucket LIMIT ");
    query.push((MAX_COVERED_HOURS + 1).to_string());
    query.push(
        "), coverage AS (SELECT CURRENT_TIMESTAMP AS read_time, \
        COALESCE(array_agg(bucket ORDER BY bucket), '{}'::timestamptz[]) AS covered_buckets, \
        COALESCE(array_agg(refreshed_at ORDER BY bucket), '{}'::timestamptz[]) AS refreshed_at, \
        COUNT(*)::bigint AS hours FROM clean), eligible AS (SELECT c.*, \
        c.hours > 0 AND c.hours <= ",
    );
    query.push(MAX_COVERED_HOURS.to_string());
    // Unique whole-hour ledger keys inside these exact bounds plus the exact
    // slot count prove full coverage. Dirty hours were removed before counting.
    query.push(
        " AND r.since = date_trunc('hour', r.since, 'UTC') \
        AND r.until = date_trunc('hour', r.until, 'UTC') \
        AND r.until <= date_trunc('hour', c.read_time, 'UTC') \
        AND c.hours = EXTRACT(EPOCH FROM (r.until - r.since)) / 3600 \
        AS summary_ready FROM coverage c CROSS JOIN request r), summary_rows AS (SELECT s.bucket, ",
    );
    let reference = summary_ref(kind);
    query.push(if group_refs { reference } else { "NULL::text" });
    let status = if matches!(
        kind,
        SummaryKind::ExecutionStatus | SummaryKind::WorkerStatus
    ) {
        "new_status"
    } else {
        "NULL::text"
    };
    let (_, _, _, count) = columns(kind);
    query.push(format!(" AS reference, {status} AS status, SUM({count})::bigint AS count FROM {} s CROSS JOIN request r ", kind.summary_table()));
    query.push(
        "WHERE s.bucket >= r.since AND s.bucket < r.until AND (SELECT summary_ready FROM eligible)",
    );
    if let Some(refs) = refs {
        query
            .push(format!(" AND {reference} = ANY("))
            .push_bind(refs)
            .push("::text[])");
    }
    if let Some(statuses) = statuses {
        query
            .push(format!(" AND {status} = ANY("))
            .push_bind(statuses)
            .push("::text[])");
    }
    query.push(" GROUP BY 1, 2, 3), data AS (SELECT \
        COALESCE(array_agg(bucket ORDER BY bucket, reference NULLS LAST, status NULLS LAST), '{}'::timestamptz[]) AS buckets, \
        COALESCE(array_agg(reference ORDER BY bucket, reference NULLS LAST, status NULLS LAST), '{}'::text[]) AS references, \
        COALESCE(array_agg(status ORDER BY bucket, reference NULLS LAST, status NULLS LAST), '{}'::text[]) AS statuses, \
        COALESCE(array_agg(count ORDER BY bucket, reference NULLS LAST, status NULLS LAST), '{}'::bigint[]) AS counts FROM summary_rows) \
        SELECT e.read_time, e.covered_buckets, e.refreshed_at, e.summary_ready, d.buckets, d.references, d.statuses, d.counts \
        FROM eligible e CROSS JOIN data d");
    query
}

/// Acquire preserves caller-owned connections and their timezone/search_path.
/// This method owns the transaction; call it outside an existing transaction so
/// the source lock precedes the first REPEATABLE READ data snapshot.
pub async fn read<'a, A>(
    acquire: A,
    kind: SummaryKind,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    refs: Option<&[String]>,
    statuses: Option<&[String]>,
    group_refs: bool,
) -> Result<AnalyticsRead<Vec<ReadRow>>>
where
    A: Acquire<'a, Database = Postgres>,
{
    let mut connection = acquire.acquire().await?;
    let mut transaction =
        Connection::begin_with(&mut *connection, "BEGIN ISOLATION LEVEL REPEATABLE READ").await?;
    // These utility commands share one packet and take no data snapshot. Keep
    // BEGIN separate: SQLx must track the transaction before a LOCK can fail.
    // The parent lock survives rollback to the subsequent maintenance savepoint.
    let setup = format!(
        "LOCK TABLE ONLY {} IN ACCESS SHARE MODE; SAVEPOINT analytics_maintenance",
        kind.source_table()
    );
    // Execute an argument-free string through SQLx's boxed Send executor. It
    // uses the same simple-query packet as raw_sql, while keeping this reader
    // usable by spawned tasks and Axum handlers.
    if let Err(error) = (&mut *transaction).execute(setup.as_str()).await {
        transaction.rollback().await?;
        return Err(error.into());
    }
    // Fully covered reads return metadata and counts in the same bound query.
    // Otherwise the bounded ledger rows feed the ordinary mixed/raw planner.
    let clean = covered_query(kind, start, end, refs, statuses, group_refs)
        .build_query_as::<CoveredRead>()
        .fetch_one(&mut *transaction)
        .await;
    let (plan, mut data) = match clean {
        Ok(covered) => {
            let clean: Vec<_> = covered
                .covered_buckets
                .into_iter()
                .zip(covered.refreshed_at)
                .collect();
            let plan = ReadPlan::build(start, end, covered.read_time, &clean);
            if covered.summary_ready {
                let data = covered
                    .buckets
                    .into_iter()
                    .zip(covered.references)
                    .zip(covered.statuses)
                    .zip(covered.counts)
                    .map(|(((bucket, reference), status), count)| ReadRow {
                        bucket,
                        reference,
                        status,
                        count,
                    })
                    .collect();
                (plan, data)
            } else {
                match rows(
                    &mut transaction,
                    kind,
                    &plan.metadata.summary_ranges,
                    true,
                    refs,
                    statuses,
                    group_refs,
                )
                .await
                {
                    Ok(data) => (plan, data),
                    Err(error) if unavailable(&error) => {
                        sqlx::query("ROLLBACK TO SAVEPOINT analytics_maintenance")
                            .execute(&mut *transaction)
                            .await?;
                        (ReadPlan::raw(start, end), Vec::new())
                    }
                    Err(error) => {
                        transaction.rollback().await?;
                        return Err(error.into());
                    }
                }
            }
        }
        Err(error) if unavailable(&error) => {
            sqlx::query("ROLLBACK TO SAVEPOINT analytics_maintenance")
                .execute(&mut *transaction)
                .await?;
            (ReadPlan::raw(start, end), Vec::new())
        }
        Err(error) => {
            transaction.rollback().await?;
            return Err(error.into());
        }
    };
    // COMMIT releases the savepoint. A separate RELEASE would add a round trip
    // without changing fallback, snapshot, or parent-lock lifetime.
    data.extend(
        rows(
            &mut transaction,
            kind,
            &plan.metadata.raw_ranges,
            false,
            refs,
            statuses,
            group_refs,
        )
        .await?,
    );
    // Each SQL arm already groups its dimensions. The plan assigns each hour to
    // exactly one arm, so no two rows need an additional application-side sum.
    data.sort_by(|left, right| {
        left.bucket
            .cmp(&right.bucket)
            .then_with(|| left.reference.is_none().cmp(&right.reference.is_none()))
            .then_with(|| left.reference.cmp(&right.reference))
            .then_with(|| left.status.is_none().cmp(&right.status.is_none()))
            .then_with(|| left.status.cmp(&right.status))
    });
    transaction.commit().await?;
    Ok(AnalyticsRead {
        data,
        metadata: plan.metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn holes_boundaries_and_current_hour_are_disjoint_and_coalesced() {
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let at = |h| base + Duration::hours(h);
        let clean = [0, 1, 3, 4, 5].map(|h| (at(h), at(9)));
        let plan = ReadPlan::build(
            at(0) + Duration::minutes(5),
            at(6),
            at(5) + Duration::minutes(30),
            &clean,
        );
        assert_eq!(plan.metadata.mode, ReadMode::SummaryPlusRaw);
        assert_eq!(
            plan.metadata.summary_ranges,
            vec![
                ReadRange {
                    start: at(1),
                    end: at(2)
                },
                ReadRange {
                    start: at(3),
                    end: at(5)
                }
            ]
        );
        assert_eq!(
            plan.metadata.raw_ranges,
            vec![
                ReadRange {
                    start: at(0) + Duration::minutes(5),
                    end: at(1)
                },
                ReadRange {
                    start: at(2),
                    end: at(3)
                },
                ReadRange {
                    start: at(5),
                    end: at(6)
                }
            ]
        );
    }

    #[test]
    fn excessive_fragmentation_falls_back_truthfully() {
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let end = base + Duration::hours(1000);
        let clean: Vec<_> = (0..300)
            .map(|i| (base + Duration::hours(i * 2), end))
            .collect();
        let plan = ReadPlan::build(base, end, end, &clean);
        assert_eq!(plan.metadata.mode, ReadMode::RawOnly);
        assert!(plan.metadata.summary_ranges.is_empty());
        assert_eq!(
            plan.metadata.raw_ranges,
            vec![ReadRange { start: base, end }]
        );
    }

    #[tokio::test]
    async fn raw_query_has_timestamp_index_bounds_on_the_actual_reader_sql() -> Result<()> {
        let config = crate::config::Config::load_from_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config.test.yaml"
        ))?;
        let database = crate::test_database::TestDatabase::create(&config.database)
            .await?
            .with_cleanup_on_drop();
        let mut connection = database.pool().acquire().await?;
        sqlx::query("SET enable_seqscan = off")
            .execute(&mut *connection)
            .await?;
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let end = start + Duration::hours(1);
        for kind in SummaryKind::ALL {
            let query = row_query(kind, &[ReadRange { start, end }], false, None, None, true);
            let explain: serde_json::Value =
                sqlx::query_scalar(&format!("EXPLAIN (FORMAT JSON) {}", query.sql()))
                    .bind(start)
                    .bind(end)
                    .fetch_one(&mut *connection)
                    .await?;
            let serialized = explain.to_string().replace("\\\"", "");
            assert!(serialized.contains("Index Cond"), "{kind:?}: {serialized}");
            assert!(
                serialized.contains(&format!("{} >=", kind.time_column())),
                "{kind:?}: {serialized}"
            );
            assert!(
                serialized.contains(&format!("{} <", kind.time_column())),
                "{kind:?}: {serialized}"
            );
        }
        drop(connection);
        database.cleanup().await?;
        Ok(())
    }
}
