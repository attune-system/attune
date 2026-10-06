//! PostgreSQL boundary contracts for raw analytics, independent of hourly views.

use std::collections::BTreeSet;

use attune_common::{
    config::Config,
    repositories::analytics::{
        AnalyticsRepository as Analytics, AnalyticsTimeRange, DashboardBucketKind,
    },
    test_database::TestDatabase,
};
use chrono::{Duration, TimeZone, Utc};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

async fn fixture() -> Result<TestDatabase> {
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    Ok(TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop())
}

fn range() -> AnalyticsTimeRange {
    let base = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    AnalyticsTimeRange {
        since: base + Duration::minutes(30),
        until: base + Duration::minutes(125),
    }
}

async fn history(
    pool: &PgPool,
    minute: i64,
    operation: &str,
    id: i64,
    action: &str,
    status: &str,
    fields: &[&str],
) -> Result<()> {
    sqlx::query("INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, new_values) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(range().since - Duration::minutes(30) + Duration::minutes(minute))
        .bind(operation).bind(id).bind(action).bind(fields).bind(json!({"status": status}))
        .execute(pool).await?;
    Ok(())
}

#[tokio::test]
async fn hourly_analytics_include_whole_utc_hours_and_count_retry_attempts() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    // The public hourly relations remain a schema contract, but these readers
    // must work without them so computed-bucket filters cannot block indexes.
    sqlx::query("DROP VIEW execution_status_hourly, execution_throughput_hourly, event_volume_hourly, worker_status_hourly, enforcement_volume_hourly, execution_volume_hourly")
        .execute(pool).await?;
    let base = range().since - Duration::minutes(30);
    history(pool, 50, "UPDATE", 1, "fixture.a", "completed", &["status"]).await?;
    history(pool, 60, "INSERT", 1, "fixture.a", "requested", &[]).await?;
    history(pool, 105, "INSERT", 2, "fixture.b", "requested", &[]).await?;
    for (minute, status) in [
        (65, "running"),
        (70, "failed"),
        (71, "running"),
        (72, "failed"),
        (73, "timeout"),
        (74, "cancelled"),
        (75, "abandoned"),
        (90, "completed"),
        (124, "completed"),
        (125, "completed"),
        (130, "completed"),
    ] {
        history(pool, minute, "UPDATE", 1, "fixture.a", status, &["status"]).await?;
    }
    history(pool, 91, "UPDATE", 1, "fixture.a", "completed", &["result"]).await?;
    history(pool, 180, "UPDATE", 1, "fixture.a", "failed", &["status"]).await?;
    for minute in [50, 60, 90, 124, 125, 130, 180] {
        let at = base + Duration::minutes(minute);
        sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ('fixture.trigger', $1)")
            .bind(at)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO enforcement (rule_ref, trigger_ref, payload, created) VALUES ('fixture.rule', 'fixture.trigger', '{}', $1)").bind(at).execute(pool).await?;
        sqlx::query("INSERT INTO worker_history (time, operation, entity_id, entity_ref, changed_fields, new_values) VALUES ($1, 'UPDATE', 1, 'fixture-worker', ARRAY['status'], '{\"status\":\"online\"}')").bind(at).execute(pool).await?;
    }
    sqlx::query("INSERT INTO execution (action_ref, status, created) VALUES ('fixture.live', 'completed', $1)").bind(base + Duration::minutes(60)).execute(pool).await?;

    // Every query runs in a non-hour-offset session. UTC buckets must not shift.
    let mut connection = pool.acquire().await?;
    sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
        .execute(&mut *connection)
        .await?;
    let range = range();
    let status = Analytics::execution_status_hourly(&mut *connection, &range).await?;
    assert!(status
        .iter()
        .all(|row| row.bucket == base + Duration::hours(1)
            || row.bucket == base + Duration::hours(2)));
    assert_eq!(
        status.iter().map(|row| row.transition_count).sum::<i64>(),
        11
    );
    assert_eq!(
        Analytics::execution_status_hourly_by_action(&mut *connection, &range, "fixture.a")
            .await?
            .len(),
        status.len()
    );
    assert!(
        Analytics::execution_status_hourly_by_action(&mut *connection, &range, "missing")
            .await?
            .is_empty()
    );

    let throughput = Analytics::execution_throughput_hourly(&mut *connection, &range).await?;
    assert_eq!(
        throughput
            .iter()
            .map(|row| row.execution_count)
            .sum::<i64>(),
        2
    );
    let filtered =
        Analytics::execution_throughput_hourly_by_action(&mut *connection, &range, "fixture.a")
            .await?;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].execution_count, 1);
    assert_eq!(filtered[0].bucket, base + Duration::hours(1));

    let failure = Analytics::execution_failure_rate(&mut *connection, &range).await?;
    assert_eq!(
        (
            failure.total_terminal,
            failure.failed_count,
            failure.timeout_count,
            failure.completed_count
        ),
        (7, 2, 1, 4)
    );
    assert!((failure.failure_rate_pct - 300.0 / 7.0).abs() < 0.00001);
    let aligned_range = AnalyticsTimeRange {
        since: base + Duration::hours(1),
        until: base + Duration::hours(2),
    };
    assert_eq!(
        Analytics::execution_failure_rate(&mut *connection, &aligned_range)
            .await?
            .total_terminal,
        7
    );
    let earlier_end = AnalyticsTimeRange {
        since: aligned_range.since,
        until: aligned_range.until - Duration::nanoseconds(1),
    };
    assert_eq!(
        Analytics::execution_failure_rate(&mut *connection, &earlier_end)
            .await?
            .total_terminal,
        4
    );
    let events = Analytics::event_volume_hourly(&mut *connection, &range).await?;
    assert_eq!(
        events
            .iter()
            .map(|row| (row.bucket, row.event_count))
            .collect::<Vec<_>>(),
        vec![
            (base + Duration::hours(1), 2),
            (base + Duration::hours(2), 3)
        ]
    );
    assert_eq!(
        Analytics::event_volume_hourly_by_trigger(&mut *connection, &range, "fixture.trigger")
            .await?
            .iter()
            .map(|row| row.event_count)
            .sum::<i64>(),
        5
    );
    assert_eq!(
        Analytics::enforcement_volume_hourly(&mut *connection, &range)
            .await?
            .iter()
            .map(|row| row.enforcement_count)
            .sum::<i64>(),
        5
    );
    assert_eq!(
        Analytics::enforcement_volume_hourly_by_rule(&mut *connection, &range, "fixture.rule")
            .await?
            .iter()
            .map(|row| row.enforcement_count)
            .sum::<i64>(),
        5
    );
    assert_eq!(
        Analytics::worker_status_hourly(&mut *connection, &range)
            .await?
            .iter()
            .map(|row| row.transition_count)
            .sum::<i64>(),
        5
    );
    assert_eq!(
        Analytics::worker_status_hourly_by_name(&mut *connection, &range, "fixture-worker")
            .await?
            .iter()
            .map(|row| row.transition_count)
            .sum::<i64>(),
        5
    );
    let volume = Analytics::execution_volume_hourly(&mut *connection, &range).await?;
    assert_eq!(volume.len(), 1);
    assert_eq!(volume[0].initial_status.as_deref(), Some("completed"));
    assert_eq!(
        Analytics::execution_volume_hourly_by_action(&mut *connection, &range, "fixture.live")
            .await?[0]
            .execution_count,
        1
    );
    drop(connection);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn dashboard_raw_bounds_preserve_partial_buckets_filters_and_terminal_counts() -> Result<()> {
    let database = fixture().await?;
    // Own all connections in this pool and set their timezone before any query.
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SELECT set_config('search_path', 'attune,public', false)")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database.database_url())
        .await?;
    for (minute, status) in [
        (50, "completed"),
        (60, "failed"),
        (61, "running"),
        (62, "failed"),
        (63, "cancelled"),
        (64, "abandoned"),
        (124, "completed"),
        (125, "completed"),
    ] {
        history(&pool, minute, "UPDATE", 1, "fixture.a", status, &["status"]).await?;
    }
    history(&pool, 100, "INSERT", 2, "fixture.a", "requested", &[]).await?;
    history(
        &pool,
        100,
        "UPDATE",
        3,
        "fixture.b",
        "completed",
        &["status"],
    )
    .await?;
    let range = range();
    let refs = BTreeSet::from(["fixture.a".to_string()]);
    let global = Analytics::dashboard_bucket_rows(
        &pool,
        &range,
        DashboardBucketKind::ExecutionThroughput,
        None,
    )
    .await?;
    assert_eq!(
        global
            .iter()
            .map(|row| (row.series.as_str(), row.count))
            .collect::<Vec<_>>(),
        vec![("all", 5), ("all", 1)]
    );
    let filtered = Analytics::dashboard_bucket_rows(
        &pool,
        &range,
        DashboardBucketKind::ExecutionThroughput,
        Some(&refs),
    )
    .await?;
    assert_eq!(
        filtered
            .iter()
            .map(|row| (row.series.as_str(), row.count))
            .collect::<Vec<_>>(),
        vec![("fixture.a", 4), ("fixture.a", 1)]
    );
    assert_eq!(filtered[1].bucket_start, range.until - Duration::minutes(5));
    let statuses = Analytics::dashboard_bucket_rows(
        &pool,
        &range,
        DashboardBucketKind::ExecutionStatus,
        Some(&refs),
    )
    .await?;
    assert_eq!(
        statuses
            .iter()
            .map(|row| (row.series.as_str(), row.count))
            .collect::<Vec<_>>(),
        vec![
            ("abandoned", 1),
            ("cancelled", 1),
            ("failed", 2),
            ("completed", 1)
        ]
    );
    let no_refs = BTreeSet::new();
    assert!(Analytics::dashboard_bucket_rows(
        &pool,
        &range,
        DashboardBucketKind::ExecutionThroughput,
        Some(&no_refs)
    )
    .await?
    .is_empty());

    for minute in [50, 60, 124, 125] {
        sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ('fixture.trigger', $1)")
            .bind(range.since - Duration::minutes(30) + Duration::minutes(minute))
            .execute(&pool)
            .await?;
    }
    let event_refs = BTreeSet::from(["fixture.trigger".to_string()]);
    for filter in [None, Some(&event_refs)] {
        let events = Analytics::dashboard_bucket_rows(
            &pool,
            &range,
            DashboardBucketKind::EventVolume,
            filter,
        )
        .await?;
        assert_eq!(
            events.iter().map(|row| row.count).collect::<Vec<_>>(),
            vec![1, 1]
        );
    }
    pool.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn empty_hourly_analytics_return_empty_series_and_zero_failure_rate() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let range = range();
    assert!(Analytics::execution_status_hourly(pool, &range)
        .await?
        .is_empty());
    assert!(Analytics::execution_throughput_hourly(pool, &range)
        .await?
        .is_empty());
    assert!(Analytics::event_volume_hourly(pool, &range)
        .await?
        .is_empty());
    assert!(Analytics::enforcement_volume_hourly(pool, &range)
        .await?
        .is_empty());
    assert!(Analytics::worker_status_hourly(pool, &range)
        .await?
        .is_empty());
    assert!(Analytics::execution_volume_hourly(pool, &range)
        .await?
        .is_empty());
    let failure = Analytics::execution_failure_rate(pool, &range).await?;
    assert_eq!(
        (
            failure.total_terminal,
            failure.failed_count,
            failure.timeout_count,
            failure.completed_count
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(failure.failure_rate_pct, 0.0);
    database.cleanup().await?;
    Ok(())
}
