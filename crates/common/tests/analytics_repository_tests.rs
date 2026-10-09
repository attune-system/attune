//! PostgreSQL boundary contracts for raw analytics, independent of hourly views.

use std::collections::BTreeSet;

use attune_common::repositories::native_maintenance::{
    read::{self, ReadMode},
    SummaryKind,
};
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

/// Test-owned materialization of the raw oracle, including empty ledger hours.
/// Production builders have their own acknowledgement/concurrency tests.
async fn cover(
    pool: &PgPool,
    kind: SummaryKind,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> Result<()> {
    let (reference, status, predicate, count) = match kind {
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
    };
    let dimension = match kind {
        SummaryKind::ExecutionStatus | SummaryKind::ExecutionCreation => "action_ref",
        SummaryKind::EventVolume => "trigger_ref",
        SummaryKind::WorkerStatus => "worker_name",
    };
    let has_status = matches!(
        kind,
        SummaryKind::ExecutionStatus | SummaryKind::WorkerStatus
    );
    let status_column = if has_status { ", new_status" } else { "" };
    let status_select = if has_status {
        format!(", {status}")
    } else {
        String::new()
    };
    let status_group = if has_status { ", 3" } else { "" };
    let mut tx = pool.begin().await?;
    sqlx::query(&format!("INSERT INTO {} (bucket, {dimension}{status_column}, {count}) SELECT date_trunc('hour', {}, 'UTC'), {reference}{status_select}, COUNT(*)::bigint FROM {} WHERE {} >= $1 AND {} < $2 AND ({predicate}) GROUP BY 1, 2{status_group}", kind.summary_table(), kind.time_column(), kind.source_table(), kind.time_column(), kind.time_column()))
        .bind(start).bind(end).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO native_summary_hour (kind, bucket, refreshed_at) SELECT $1, bucket, NOW() FROM generate_series($2::timestamptz, $3::timestamptz - INTERVAL '1 hour', INTERVAL '1 hour') AS bucket")
        .bind(kind).bind(start).bind(end).execute(&mut *tx).await?;
    sqlx::query(
        "DELETE FROM native_summary_invalidation WHERE kind = $1 AND bucket >= $2 AND bucket < $3",
    )
    .bind(kind)
    .bind(start)
    .bind(end)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn native_all_four_kinds_match_raw_oracle_with_nulls_scopes_and_ledger_holes() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let base = range().since - Duration::minutes(30);
    let end = base + Duration::hours(4);
    for (index, action, status) in [
        (1, Some("fixture.a"), Some("failed")),
        (2, None, None),
        (3, Some(""), Some("")),
        (4, Some("fixture.b"), Some("completed")),
    ] {
        for at in [base + Duration::minutes(5), base + Duration::hours(2)] {
            sqlx::query("INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, new_values) VALUES ($1, 'INSERT', $2, $3, ARRAY['status'], $4)")
                .bind(at).bind(index).bind(action).bind(json!({"status":status})).execute(pool).await?;
            sqlx::query("INSERT INTO worker_history (time, operation, entity_id, entity_ref, changed_fields, new_values) VALUES ($1, 'UPDATE', $2, $3, ARRAY['status'], $4)")
                .bind(at).bind(index).bind(action).bind(json!({"status":status})).execute(pool).await?;
        }
    }
    for (at, trigger) in [
        (base, "fixture.a"),
        (base + Duration::hours(2), "fixture.b"),
    ] {
        sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ($1, $2)")
            .bind(trigger)
            .bind(at)
            .execute(pool)
            .await?;
    }
    let refs = ["fixture.a".to_string()];
    for kind in SummaryKind::ALL {
        let raw = read::read(pool, kind, base, end, None, None, true).await?;
        assert_eq!(raw.metadata.mode, ReadMode::RawOnly);
        let scoped_raw = read::read(pool, kind, base, end, Some(&refs), None, true).await?;
        cover(pool, kind, base, end).await?;
        let summary = read::read(pool, kind, base, end, None, None, true).await?;
        assert_eq!(summary.metadata.mode, ReadMode::SummaryOnly);
        assert_eq!(summary.data, raw.data, "{kind:?}");
        assert_eq!(
            summary.metadata.summary_ranges.len(),
            1,
            "empty hours have coverage"
        );
        let scoped_summary = read::read(pool, kind, base, end, Some(&refs), None, true).await?;
        assert_eq!(scoped_summary.data, scoped_raw.data);
        assert!(read::read(pool, kind, base, end, Some(&[]), None, true)
            .await?
            .data
            .is_empty());
        // Remove one ledger hour and dirty its adjacent hour, including an old
        // ID. Both hours become one raw range, regardless of sequence order.
        sqlx::query("DELETE FROM native_summary_hour WHERE kind = $1 AND bucket = $2")
            .bind(kind)
            .bind(base + Duration::hours(1))
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO native_summary_invalidation (kind, bucket) VALUES ($1, $2)")
            .bind(kind)
            .bind(base + Duration::hours(2))
            .execute(pool)
            .await?;
        let mixed = read::read(pool, kind, base, end, None, None, true).await?;
        assert_eq!(mixed.metadata.mode, ReadMode::SummaryPlusRaw);
        assert_eq!(mixed.metadata.raw_ranges.len(), 1);
        assert_eq!(
            mixed.metadata.raw_ranges[0].start,
            base + Duration::hours(1)
        );
        assert_eq!(mixed.metadata.raw_ranges[0].end, base + Duration::hours(3));
        assert_eq!(mixed.data, raw.data);
    }
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_backdates_uncommitted_notifications_partial_expiry_and_missing_metadata(
) -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let base = range().since - Duration::minutes(30);
    let end = base + Duration::hours(3);
    sqlx::query(
        "INSERT INTO event (trigger_ref, created) VALUES ('fixture.a', $1), ('fixture.a', $2)",
    )
    .bind(base + Duration::minutes(10))
    .bind(base + Duration::minutes(40))
    .execute(pool)
    .await?;
    cover(pool, SummaryKind::EventVolume, base, end).await?;
    let mut writer = pool.begin().await?;
    sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ('fixture.a', $1)")
        .bind(base + Duration::minutes(50))
        .execute(&mut *writer)
        .await?;
    let lower_id: i64 = sqlx::query_scalar("SELECT id FROM native_summary_invalidation WHERE kind = 'event_volume' AND bucket = $1 ORDER BY id LIMIT 1")
        .bind(base).fetch_one(&mut *writer).await?;
    // Commit a higher ID first in another hour. The reader must later notice
    // the lower ID, rather than using sequence order as a global watermark.
    let higher_id: i64 = sqlx::query_scalar("INSERT INTO native_summary_invalidation (kind, bucket) VALUES ('event_volume', $1) RETURNING id")
        .bind(base + Duration::hours(2)).fetch_one(pool).await?;
    assert!(lower_id < higher_id);
    // The source row and its notification share commit visibility. An
    // uncommitted lower-ID notification must not force a false dirty read.
    let before = read::read(pool, SummaryKind::EventVolume, base, end, None, None, false).await?;
    assert_eq!(before.metadata.mode, ReadMode::SummaryPlusRaw);
    assert_eq!(before.metadata.summary_ranges[0].start, base);
    assert_eq!(
        before.metadata.summary_ranges[0].end,
        base + Duration::hours(2)
    );
    assert_eq!(before.data[0].count, 2);
    writer.commit().await?;
    let after = read::read(pool, SummaryKind::EventVolume, base, end, None, None, false).await?;
    assert_eq!(after.metadata.mode, ReadMode::SummaryPlusRaw);
    assert_eq!(after.metadata.raw_ranges[0].start, base);
    assert_eq!(after.data[0].count, 3);
    sqlx::query("DELETE FROM event WHERE created < $1")
        .bind(base + Duration::minutes(30))
        .execute(pool)
        .await?;
    let retained = read::read(pool, SummaryKind::EventVolume, base, end, None, None, false).await?;
    assert_eq!(retained.data[0].count, 2);
    let clipped = read::read(
        pool,
        SummaryKind::EventVolume,
        base + Duration::minutes(45),
        end,
        None,
        None,
        false,
    )
    .await?;
    assert_eq!(clipped.data[0].count, 1);
    // Missing metadata must roll back its failed statement before raw SQL.
    sqlx::query("ALTER TABLE native_summary_hour RENAME TO test_unavailable_ledger")
        .execute(pool)
        .await?;
    let fallback = read::read(pool, SummaryKind::EventVolume, base, end, None, None, false).await?;
    assert_eq!(fallback.metadata.mode, ReadMode::RawOnly);
    assert_eq!(fallback.data, retained.data);
    sqlx::query("ALTER TABLE test_unavailable_ledger RENAME TO native_summary_hour")
        .execute(pool)
        .await?;
    // A covered hour with an unavailable summary table has the same raw
    // fallback contract. Force that query by clearing the test-owned notices.
    sqlx::query("DELETE FROM native_summary_invalidation WHERE kind = 'event_volume'")
        .execute(pool)
        .await?;
    sqlx::query("ALTER TABLE event_volume_hourly_summary RENAME TO test_unavailable_summary")
        .execute(pool)
        .await?;
    let fallback = read::read(pool, SummaryKind::EventVolume, base, end, None, None, false).await?;
    assert_eq!(fallback.metadata.mode, ReadMode::RawOnly);
    assert_eq!(fallback.data, retained.data);
    sqlx::query("ALTER TABLE test_unavailable_summary RENAME TO event_volume_hourly_summary")
        .execute(pool)
        .await?;
    // A malformed schema is a logic error, not an availability fallback.
    sqlx::query("ALTER TABLE native_summary_hour RENAME COLUMN refreshed_at TO test_bad_column")
        .execute(pool)
        .await?;
    assert!(
        read::read(pool, SummaryKind::EventVolume, base, end, None, None, false)
            .await
            .is_err()
    );
    database.cleanup().await?;
    Ok(())
}

async fn wait_for_lock(pool: &PgPool, sql: &str) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if sqlx::query_scalar::<_, bool>(sql).fetch_one(pool).await? {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn native_source_lock_precedes_snapshot_and_partition_expiry() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let base = range().since - Duration::minutes(30);
    let end = base + Duration::hours(1);
    sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ('fixture.a', $1)")
        .bind(base)
        .execute(pool)
        .await?;
    cover(pool, SummaryKind::EventVolume, base, end).await?;
    // Hold only the ledger so the real reader reaches its parent lock and then
    // waits at metadata. Readiness is an observed lock, not a fixed sleep.
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE native_summary_hour IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let reader_pool = pool.clone();
    let reader = tokio::spawn(async move {
        read::read(
            &reader_pool,
            SummaryKind::EventVolume,
            base,
            end,
            None,
            None,
            false,
        )
        .await
    });
    wait_for_lock(pool, "SELECT EXISTS (SELECT 1 FROM pg_locks source JOIN pg_locks ledger ON source.pid = ledger.pid WHERE source.relation = 'event'::regclass AND source.mode = 'AccessShareLock' AND source.granted AND ledger.relation = 'native_summary_hour'::regclass AND NOT ledger.granted)").await?;
    let mut expiry = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *expiry)
        .await?;
    let error = sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *expiry)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    expiry.rollback().await?;
    blocker.commit().await?;
    let result = reader.await??;
    assert_eq!(result.metadata.mode, ReadMode::SummaryOnly);
    assert_eq!(result.data[0].count, 1);

    // Now expiry wins. The waiting reader must take its first snapshot after
    // DROP and maintenance cleanup commit, not retain the old ledger count.
    let mut expiry = pool.begin().await?;
    sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *expiry)
        .await?;
    let reader_pool = pool.clone();
    let reader = tokio::spawn(async move {
        read::read(
            &reader_pool,
            SummaryKind::EventVolume,
            base,
            end,
            None,
            None,
            false,
        )
        .await
    });
    wait_for_lock(pool, "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE relation = 'event'::regclass AND mode = 'AccessShareLock' AND NOT granted)").await?;
    sqlx::query("DROP TABLE event_default")
        .execute(&mut *expiry)
        .await?;
    for table in ["native_summary_invalidation", "native_summary_hour"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE kind = 'event_volume'"))
            .execute(&mut *expiry)
            .await?;
    }
    sqlx::query("DELETE FROM event_volume_hourly_summary")
        .execute(&mut *expiry)
        .await?;
    expiry.commit().await?;
    let result = reader.await??;
    assert_eq!(result.metadata.mode, ReadMode::RawOnly);
    assert!(result.data.is_empty());
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_current_hour_ignores_even_a_clean_ledger_and_stale_summary() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let base: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT date_trunc('hour', NOW(), 'UTC')")
        .fetch_one(pool)
        .await?;
    sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ('fixture.a', $1)")
        .bind(base)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO event_volume_hourly_summary (bucket, trigger_ref, event_count) VALUES ($1, 'fixture.a', 999)").bind(base).execute(pool).await?;
    sqlx::query("INSERT INTO native_summary_hour (kind, bucket, refreshed_at) VALUES ('event_volume', $1, NOW())").bind(base).execute(pool).await?;
    sqlx::query("DELETE FROM native_summary_invalidation WHERE kind = 'event_volume'")
        .execute(pool)
        .await?;
    let result = read::read(
        pool,
        SummaryKind::EventVolume,
        base,
        base + Duration::hours(1),
        None,
        None,
        false,
    )
    .await?;
    assert_eq!(result.metadata.mode, ReadMode::RawOnly);
    assert_eq!(result.data[0].count, 1);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_batched_lock_failure_rolls_back_before_connection_reuse() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let base = range().since - Duration::minutes(30);
    let end = base + Duration::hours(1);
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let mut connection = pool.acquire().await?;
    sqlx::query("SET lock_timeout = '100ms'")
        .execute(&mut *connection)
        .await?;
    let error = read::read(
        &mut *connection,
        SummaryKind::EventVolume,
        base,
        end,
        None,
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("lock timeout"), "{error}");
    // Failure happens before the batched SAVEPOINT command executes. BEGIN
    // must already be tracked, and rollback must have completed before return.
    let (one, isolation): (i32, String) =
        sqlx::query_as("SELECT 1, current_setting('transaction_isolation')")
            .fetch_one(&mut *connection)
            .await?;
    assert_eq!(one, 1);
    assert_eq!(isolation, "read committed");
    sqlx::query("RESET lock_timeout")
        .execute(&mut *connection)
        .await?;
    blocker.rollback().await?;
    let result = read::read(
        &mut *connection,
        SummaryKind::EventVolume,
        base,
        end,
        None,
        None,
        false,
    )
    .await?;
    assert_eq!(result.metadata.mode, ReadMode::RawOnly);
    assert!(result.data.is_empty());
    drop(connection);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_reader_rejects_nested_call_without_rolling_back_caller() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let base = range().since - Duration::minutes(30);
    let mut caller = pool.begin().await?;
    sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ('fixture.nested', $1)")
        .bind(base)
        .execute(&mut *caller)
        .await?;
    let error = read::read(
        &mut *caller,
        SummaryKind::EventVolume,
        base,
        base + Duration::hours(1),
        None,
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        attune_common::Error::Database(sqlx::Error::InvalidSavePointStatement)
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM event WHERE trigger_ref = 'fixture.nested'")
            .fetch_one(&mut *caller)
            .await?;
    assert_eq!(count, 1);
    caller.rollback().await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM event WHERE trigger_ref = 'fixture.nested'")
            .fetch_one(pool)
            .await?;
    assert_eq!(count, 0);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_cancelled_bootstrap_keeps_connection_reuse_safe() -> Result<()> {
    use sqlx::Connection;
    let database = fixture().await?;
    let pool = database.pool();
    let base = range().since - Duration::minutes(30);
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let mut connection = pool.acquire().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *connection)
        .await?;
    {
        let read = read::read(
            &mut *connection,
            SummaryKind::EventVolume,
            base,
            base + Duration::hours(1),
            None,
            None,
            false,
        );
        tokio::pin!(read);
        let readiness = format!("SELECT EXISTS (SELECT 1 FROM pg_locks WHERE pid = {pid} AND relation = 'event'::regclass AND mode = 'AccessShareLock' AND NOT granted)");
        tokio::select! {
            result = &mut read => panic!("reader completed before observed cancellation: {result:?}"),
            result = wait_for_lock(pool, &readiness) => result?,
        }
        // Drop precisely while bootstrap is waiting at its observed source lock.
    }
    assert!(
        !connection.is_in_transaction(),
        "tracked transaction must queue rollback on cancellation"
    );
    blocker.rollback().await?;
    let (one, isolation): (i32, String) =
        sqlx::query_as("SELECT 1, current_setting('transaction_isolation')")
            .fetch_one(&mut *connection)
            .await?;
    assert_eq!((one, isolation.as_str()), (1, "read committed"));
    drop(connection);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_reader_without_maintenance_privileges_keeps_raw_access() -> Result<()> {
    let database = fixture().await?;
    let pool = database.pool();
    let base = range().since - Duration::minutes(30);
    let end = base + Duration::hours(1);
    sqlx::query("INSERT INTO event (trigger_ref, created) VALUES ('fixture.a', $1)")
        .bind(base)
        .execute(pool)
        .await?;
    cover(pool, SummaryKind::EventVolume, base, end).await?;
    let role = format!("unit5_reader_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE ROLE {role}"))
        .execute(pool)
        .await?;
    // Schema is the fixture's constant. Application repository SQL remains
    // unqualified and inherits this connection's search_path and role.
    sqlx::query(&format!("GRANT USAGE ON SCHEMA attune TO {role}"))
        .execute(pool)
        .await?;
    sqlx::query(&format!("GRANT SELECT ON event TO {role}"))
        .execute(pool)
        .await?;
    let mut connection = pool.acquire().await?;
    sqlx::query(&format!("SET ROLE {role}"))
        .execute(&mut *connection)
        .await?;
    let read_result = read::read(
        &mut *connection,
        SummaryKind::EventVolume,
        base,
        end,
        None,
        None,
        false,
    )
    .await;
    sqlx::query("RESET ROLE").execute(&mut *connection).await?;
    drop(connection);
    sqlx::query(&format!("DROP OWNED BY {role}"))
        .execute(pool)
        .await?;
    sqlx::query(&format!("DROP ROLE {role}"))
        .execute(pool)
        .await?;
    let result = read_result?;
    assert_eq!(result.metadata.mode, ReadMode::RawOnly);
    assert_eq!(result.data[0].count, 1);
    database.cleanup().await?;
    Ok(())
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
    let status = Analytics::execution_status_hourly(&mut *connection, &range)
        .await?
        .data;
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
            .data
            .len(),
        status.len()
    );
    assert!(
        Analytics::execution_status_hourly_by_action(&mut *connection, &range, "missing")
            .await?
            .data
            .is_empty()
    );

    let throughput = Analytics::execution_throughput_hourly(&mut *connection, &range)
        .await?
        .data;
    assert_eq!(
        throughput
            .iter()
            .map(|row| row.execution_count)
            .sum::<i64>(),
        2
    );
    let filtered =
        Analytics::execution_throughput_hourly_by_action(&mut *connection, &range, "fixture.a")
            .await?
            .data;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].execution_count, 1);
    assert_eq!(filtered[0].bucket, base + Duration::hours(1));

    let failure = Analytics::execution_failure_rate(&mut *connection, &range)
        .await?
        .data;
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
            .data
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
            .data
            .total_terminal,
        4
    );
    let events = Analytics::event_volume_hourly(&mut *connection, &range)
        .await?
        .data;
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
            .data
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
            .data
            .iter()
            .map(|row| row.transition_count)
            .sum::<i64>(),
        5
    );
    assert_eq!(
        Analytics::worker_status_hourly_by_name(&mut *connection, &range, "fixture-worker")
            .await?
            .data
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
    for kind in SummaryKind::ALL {
        cover(pool, kind, base, base + Duration::hours(4)).await?;
    }
    let summarized = Analytics::execution_status_hourly(&mut *connection, &range).await?;
    assert_eq!(summarized.metadata.mode, ReadMode::SummaryOnly);
    assert_eq!(
        serde_json::to_value(&summarized.data)?,
        serde_json::to_value(&status)?
    );
    let creations = Analytics::execution_throughput_hourly(&mut *connection, &range).await?;
    assert_eq!(creations.metadata.mode, ReadMode::SummaryOnly);
    assert_eq!(
        serde_json::to_value(creations.data)?,
        serde_json::to_value(throughput)?
    );
    let summarized_events = Analytics::event_volume_hourly(&mut *connection, &range).await?;
    assert_eq!(summarized_events.metadata.mode, ReadMode::SummaryOnly);
    assert_eq!(
        serde_json::to_value(summarized_events.data)?,
        serde_json::to_value(events)?
    );
    let workers = Analytics::worker_status_hourly(&mut *connection, &range).await?;
    assert_eq!(workers.metadata.mode, ReadMode::SummaryOnly);
    assert_eq!(
        workers
            .data
            .iter()
            .map(|row| row.transition_count)
            .sum::<i64>(),
        5
    );
    let summarized_failure = Analytics::execution_failure_rate(&mut *connection, &range).await?;
    assert_eq!(summarized_failure.metadata.mode, ReadMode::SummaryOnly);
    assert_eq!(
        serde_json::to_value(summarized_failure.data)?,
        serde_json::to_value(failure)?
    );
    assert_eq!(
        Analytics::execution_throughput_hourly_by_action(&mut *connection, &range, "fixture.a")
            .await?
            .data[0]
            .execution_count,
        1
    );
    assert_eq!(
        Analytics::event_volume_hourly_by_trigger(&mut *connection, &range, "fixture.trigger")
            .await?
            .data
            .iter()
            .map(|row| row.event_count)
            .sum::<i64>(),
        5
    );
    assert_eq!(
        Analytics::worker_status_hourly_by_name(&mut *connection, &range, "fixture-worker")
            .await?
            .data
            .iter()
            .map(|row| row.transition_count)
            .sum::<i64>(),
        5
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
    .await?
    .data;
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
    .await?
    .data;
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
    .await?
    .data;
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
    .data
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
        .await?
        .data;
        assert_eq!(
            events.iter().map(|row| row.count).collect::<Vec<_>>(),
            vec![1, 1]
        );
    }
    let base = range.since - Duration::minutes(30);
    cover(
        &pool,
        SummaryKind::ExecutionStatus,
        base,
        base + Duration::hours(3),
    )
    .await?;
    cover(
        &pool,
        SummaryKind::EventVolume,
        base,
        base + Duration::hours(3),
    )
    .await?;
    for (kind, refs, oracle) in [
        (DashboardBucketKind::ExecutionThroughput, None, &global),
        (
            DashboardBucketKind::ExecutionThroughput,
            Some(&refs),
            &filtered,
        ),
        (DashboardBucketKind::ExecutionStatus, Some(&refs), &statuses),
    ] {
        let result = Analytics::dashboard_bucket_rows(&pool, &range, kind, refs).await?;
        assert_eq!(result.metadata.mode, ReadMode::SummaryPlusRaw);
        assert_eq!(
            result.metadata.summary_ranges[0].end,
            base + Duration::hours(2)
        );
        assert_eq!(result.metadata.raw_ranges[0].end, range.until);
        assert_eq!(
            serde_json::to_value(result.data)?,
            serde_json::to_value(oracle)?
        );
    }
    let result = Analytics::dashboard_bucket_rows(
        &pool,
        &range,
        DashboardBucketKind::EventVolume,
        Some(&event_refs),
    )
    .await?;
    assert_eq!(result.metadata.mode, ReadMode::SummaryPlusRaw);
    assert_eq!(
        result.data.iter().map(|row| row.count).collect::<Vec<_>>(),
        vec![1, 1]
    );
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
        .data
        .is_empty());
    assert!(Analytics::execution_throughput_hourly(pool, &range)
        .await?
        .data
        .is_empty());
    assert!(Analytics::event_volume_hourly(pool, &range)
        .await?
        .data
        .is_empty());
    assert!(Analytics::enforcement_volume_hourly(pool, &range)
        .await?
        .is_empty());
    assert!(Analytics::worker_status_hourly(pool, &range)
        .await?
        .data
        .is_empty());
    assert!(Analytics::execution_volume_hourly(pool, &range)
        .await?
        .is_empty());
    let failure = Analytics::execution_failure_rate(pool, &range).await?.data;
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
