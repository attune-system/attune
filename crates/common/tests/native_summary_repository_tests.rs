//! Real repository and migration contracts on an owned PostgreSQL 16+ clone.

use std::{str::FromStr, time::Duration as StdDuration};

use attune_common::{
    config::{Config, NativeMaintenanceConfig, RetentionTargetsConfig},
    repositories::native_maintenance::{
        summaries::{SummaryHour, SummaryRepository as Summaries},
        SummaryKind,
    },
    test_database::TestDatabase,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn base() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap()
}

fn config() -> NativeMaintenanceConfig {
    NativeMaintenanceConfig {
        operation_timeout_milliseconds: 2500,
        ..Default::default()
    }
}

async fn fixture() -> TestResult<TestDatabase> {
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    Ok(TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop())
}

async fn event(pool: &PgPool, at: DateTime<Utc>, trigger: &str) -> TestResult<()> {
    sqlx::query("INSERT INTO event (created, trigger_ref) VALUES ($1, $2)")
        .bind(at)
        .bind(trigger)
        .execute(pool)
        .await?;
    Ok(())
}

async fn refresh(pool: &PgPool, kind: SummaryKind, bucket: DateTime<Utc>) -> TestResult<()> {
    Summaries::refresh_bucket_at(pool, kind, bucket, &config(), base() + Duration::days(1)).await?;
    Ok(())
}

async fn total(pool: &PgPool, bucket: DateTime<Utc>) -> TestResult<i64> {
    Ok(sqlx::query_scalar("SELECT coalesce(sum(event_count), 0)::bigint FROM event_volume_hourly_summary WHERE bucket = $1")
        .bind(bucket).fetch_one(pool).await?)
}

async fn pending(pool: &PgPool, bucket: DateTime<Utc>) -> TestResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM native_summary_invalidation WHERE kind = 'event_volume' AND bucket = $1 ORDER BY id")
        .bind(bucket).fetch_all(pool).await?)
}

async fn named_pool(database: &TestDatabase, name: &str) -> TestResult<PgPool> {
    let options = PgConnectOptions::from_str(database.database_url())?
        .application_name(name)
        .options([("search_path", "attune"), ("TimeZone", "Asia/Kathmandu")]);
    Ok(PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?)
}

async fn session_commit_policy(pool: &PgPool) -> TestResult<(i32, String)> {
    Ok(
        sqlx::query_as("SELECT pg_backend_pid(), current_setting('synchronous_commit')")
            .fetch_one(pool)
            .await?,
    )
}

async fn crash_command(expected: &str) -> TestResult<()> {
    let line = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await??;
    if line.trim() != expected {
        return Err(format!("expected crash owner command {expected}, got {line:?}").into());
    }
    Ok(())
}

async fn crash_snapshot(pool: &PgPool) -> TestResult<serde_json::Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object(
        'raw', coalesce((SELECT jsonb_agg(jsonb_build_array(id,created,trigger_ref,payload) ORDER BY id) FROM event WHERE created=$1),'[]'::jsonb),
        'groups', coalesce((SELECT jsonb_agg(jsonb_build_array(trigger_ref,event_count) ORDER BY trigger_ref) FROM event_volume_hourly_summary WHERE bucket=$1),'[]'::jsonb),
        'coverage', coalesce((SELECT jsonb_agg(jsonb_build_array(bucket,refreshed_at)) FROM native_summary_hour WHERE kind='event_volume' AND bucket=$1),'[]'::jsonb),
        'ids', coalesce((SELECT jsonb_agg(id ORDER BY id) FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1),'[]'::jsonb),
        'state', (SELECT to_jsonb(updated) FROM native_summary_state WHERE kind='event_volume'))")
        .bind(base()).fetch_one(pool).await?)
}

/// The owning script pauses WAL flushers, kills the exact container immediately
/// after our acknowledgement, restarts its persistent volume, then resumes here.
#[tokio::test]
#[ignore = "requires scripts/prove-native-summary-crash.py and its exclusively owned crash/restart server"]
async fn async_materialization_crash_probe() -> TestResult<()> {
    use attune_common::repositories::{
        analytics::{AnalyticsRepository, AnalyticsTimeRange},
        native_maintenance::{
            read::ReadMode,
            schedule::{MaintenanceJob, ScheduleRepository},
        },
    };
    let scenario = std::env::var("ATTUNE_SUMMARY_CRASH_CASE")?;
    let expected = std::env::var("ATTUNE_SUMMARY_CRASH_OUTCOME")?;
    assert!(["bootstrap", "covered", "empty"].contains(&scenario.as_str()));
    assert!(["lost", "retained"].contains(&expected.as_str()));
    let db = fixture().await?;
    let builder = named_pool(&db, "native-summary-crash-probe").await?;
    assert_eq!(session_commit_policy(&builder).await?.1, "on");
    event(&builder, base(), "crash.fixture").await?;
    if scenario != "bootstrap" {
        refresh(&builder, SummaryKind::EventVolume, base()).await?;
    }
    // These synchronous raw commits also flush any previous baseline cache commit.
    event(&builder, base(), "crash.fixture").await?;
    event(&builder, base(), "crash.fixture").await?;
    if scenario == "empty" {
        sqlx::query("DELETE FROM event WHERE created=$1")
            .bind(base())
            .execute(&builder)
            .await?;
    }
    sqlx::query("CHECKPOINT").execute(&builder).await?;
    let before = crash_snapshot(&builder).await?;
    println!(
        "{}",
        serde_json::json!({"type":"crash_ready","case":scenario,"expected":expected,"database":db.database_name(),"before":before})
    );
    crash_command("refresh").await?;
    let config = NativeMaintenanceConfig {
        max_summary_invalidations_per_bucket: 1,
        ..Default::default()
    };
    let result = Summaries::refresh_bucket_at(
        &builder,
        SummaryKind::EventVolume,
        base(),
        &config,
        base() + Duration::days(1),
    )
    .await?;
    assert_eq!(result.notifications_processed, 1);
    // No database operation between this acknowledgement and the owner's crash.
    println!(
        "{}",
        serde_json::json!({"type":"cache_acknowledged","result":result})
    );
    if expected == "retained" {
        crash_command("flush_schedule").await?;
        assert_eq!(session_commit_policy(&builder).await?.1, "on");
        ScheduleRepository::finish_attempt(
            &builder,
            MaintenanceJob::Summary,
            base() + Duration::days(1),
            300,
            true,
        )
        .await?;
        println!(
            "{}",
            serde_json::json!({"type":"schedule_acknowledged","synchronous_commit":"on"})
        );
    }
    crash_command("recovered").await?;
    // New pool avoids any transport state inherited from connections killed by the owner.
    let recovered = named_pool(&db, "native-summary-after-crash").await?;
    let after = crash_snapshot(&recovered).await?;
    assert_eq!(
        after["raw"], before["raw"],
        "business source rows must remain durable"
    );
    let lost = before == after;
    let raw_rows = before["raw"].as_array().unwrap().len();
    let groups = if raw_rows == 0 {
        serde_json::json!([])
    } else {
        serde_json::json!([["crash.fixture", raw_rows]])
    };
    let retained = after["groups"] == groups
        && after["coverage"].as_array().unwrap().len() == 1
        && after["coverage"] != before["coverage"]
        && after["state"] != before["state"]
        && after["ids"].as_array().unwrap() == &before["ids"].as_array().unwrap()[1..];
    assert!(
        lost || retained,
        "partial cache/coverage/state/ack recovery: before={before} after={after}"
    );
    assert_eq!(
        lost,
        expected == "lost",
        "owner failed to establish requested WAL recovery branch"
    );
    assert_eq!(session_commit_policy(&recovered).await?.1, "on");
    let mut conn = recovered.acquire().await?;
    let range = AnalyticsTimeRange {
        since: base(),
        until: base() + Duration::hours(1) - Duration::nanoseconds(1),
    };
    let read = AnalyticsRepository::event_volume_hourly(&mut *conn, &range).await?;
    assert_eq!(
        read.data.iter().map(|row| row.event_count).sum::<i64>(),
        raw_rows as i64
    );
    assert_eq!(
        read.metadata.mode,
        ReadMode::RawOnly,
        "pending restored/uncaptured IDs must force raw fallback"
    );
    drop(conn);
    refresh(&recovered, SummaryKind::EventVolume, base()).await?;
    assert_eq!(total(&recovered, base()).await?, raw_rows as i64);
    assert!(pending(&recovered, base()).await?.is_empty());
    println!(
        "{}",
        serde_json::json!({"type":"crash_verified","case":scenario,"outcome":if lost {"lost"} else {"retained"},"before":before,"after":after,"raw_fallback":true,"rebuild_converged":true})
    );
    recovered.close().await;
    builder.close().await;
    db.cleanup().await?;
    Ok(())
}

/// Readiness is the exact blocked backend, never a fixed sleep.
async fn blocked(pool: &PgPool, name: &str) -> TestResult<()> {
    let deadline = Instant::now() + StdDuration::from_secs(3);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database()
             AND application_name = $1 AND cardinality(pg_blocking_pids(pid)) > 0)",
        )
        .bind(name)
        .fetch_one(pool)
        .await?;
        if waiting {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("backend {name} did not reach its lock wait").into());
        }
        tokio::task::yield_now().await;
    }
}

#[test]
fn hour_bounds_are_checked_not_silently_rounded() {
    let _public_entry_point = Summaries::refresh_bucket;
    assert_eq!(SummaryHour::new(base()).unwrap().start(), base());
    assert!(SummaryHour::new(base() + Duration::nanoseconds(1)).is_err());
    assert!(SummaryHour::new(base() + Duration::minutes(1)).is_err());
    let before_epoch = Utc.timestamp_opt(-3600, 0).unwrap();
    assert_eq!(
        SummaryHour::new(before_epoch).unwrap().end(),
        Utc.timestamp_opt(0, 0).unwrap()
    );
    assert!(SummaryHour::new(DateTime::<Utc>::MAX_UTC).is_err());
    let mut targets = RetentionTargetsConfig::default();
    targets.events.max_age_seconds = None;
    assert_eq!(
        Summaries::source_cutoff(SummaryKind::EventVolume, &targets, base()).unwrap(),
        None
    );
    targets.execution_history.max_age_seconds = Some(1);
    assert_eq!(
        Summaries::source_cutoff(SummaryKind::ExecutionStatus, &targets, base()).unwrap(),
        Some(base() - Duration::seconds(1))
    );
    targets.worker_history.max_age_seconds = Some(u64::MAX);
    assert!(Summaries::source_cutoff(SummaryKind::WorkerStatus, &targets, base()).is_err());
}

#[tokio::test]
async fn all_kinds_preserve_null_groups_replace_removed_groups_and_cover_empty_hours(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    for table in ["execution_history", "worker_history"] {
        sqlx::query(&format!("INSERT INTO {table} (time, operation, entity_id, entity_ref, changed_fields, new_values)
            VALUES ($1, 'INSERT', 1, NULL, ARRAY['status'], '{{}}'),
                   ($1, 'UPDATE', 1, '', ARRAY['status'], '{{\"status\":\"\"}}'),
                   ($1, 'UPDATE', 1, NULL, ARRAY['other'], '{{\"status\":\"ignored\"}}')"))
            .bind(base()).execute(pool).await?;
    }
    event(pool, base(), "first").await?;
    event(pool, base() + Duration::hours(1), "boundary").await?;
    for kind in SummaryKind::ALL {
        refresh(pool, kind, base()).await?;
    }
    let null_group: i64 = sqlx::query_scalar("SELECT transition_count FROM execution_status_hourly_summary WHERE bucket = $1 AND action_ref IS NULL AND new_status IS NULL")
        .bind(base()).fetch_one(pool).await?;
    assert_eq!(null_group, 1);
    let creation: i64 = sqlx::query_scalar("SELECT execution_count FROM execution_creation_hourly_summary WHERE bucket = $1 AND action_ref IS NULL")
        .bind(base()).fetch_one(pool).await?;
    assert_eq!(creation, 1);
    let workers: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM worker_status_hourly_summary WHERE bucket = $1",
    )
    .bind(base())
    .fetch_one(pool)
    .await?;
    assert_eq!(workers, 2);
    assert_eq!(total(pool, base()).await?, 1);

    sqlx::query("UPDATE event SET trigger_ref = 'corrected', created = $2 WHERE created = $1")
        .bind(base())
        .bind(base() + Duration::hours(2))
        .execute(pool)
        .await?;
    assert!(!pending(pool, base()).await?.is_empty());
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    assert_eq!(total(pool, base()).await?, 0);
    assert!(!pending(pool, base() + Duration::hours(2)).await?.is_empty());
    sqlx::query("DELETE FROM execution_history WHERE time = $1")
        .bind(base())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM worker_history WHERE time = $1")
        .bind(base())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM event WHERE created = $1")
        .bind(base() + Duration::hours(2))
        .execute(pool)
        .await?;
    for kind in SummaryKind::ALL {
        refresh(pool, kind, base()).await?;
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*)::bigint FROM {} WHERE bucket = $1",
            kind.summary_table()
        ))
        .bind(base())
        .fetch_one(pool)
        .await?;
        assert_eq!(count, 0);
    }
    refresh(pool, SummaryKind::EventVolume, base() + Duration::hours(2)).await?;
    let coverage: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM native_summary_hour WHERE bucket = $1")
            .bind(base())
            .fetch_one(pool)
            .await?;
    assert_eq!(coverage, 4);
    let status = Summaries::status(pool).await?;
    assert_eq!(status.len(), 4);
    assert!(status.iter().all(|s| s.latest_success.is_some()));
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn capped_capture_acknowledges_exact_ids_and_replacement_does_not_accumulate(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    for _ in 0..3 {
        event(pool, base(), "fixture").await?;
    }
    let ids = pending(pool, base()).await?;
    assert_eq!(ids.len(), 3);
    let config = NativeMaintenanceConfig {
        max_summary_invalidations_per_bucket: 1,
        ..config()
    };
    let result = Summaries::refresh_bucket_at(
        pool,
        SummaryKind::EventVolume,
        base(),
        &config,
        base() + Duration::hours(1),
    )
    .await?;
    assert_eq!(result.notifications_processed, 1);
    assert_eq!(total(pool, base()).await?, 3);
    assert_eq!(pending(pool, base()).await?, ids[1..]);
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    assert_eq!(total(pool, base()).await?, 3);
    assert!(pending(pool, base()).await?.is_empty());
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn out_of_order_commit_and_reserved_lower_ids_survive_the_builder_snapshot() -> TestResult<()>
{
    let db = fixture().await?;
    let pool = db.pool();
    let mut late = pool.begin().await?;
    sqlx::query("INSERT INTO event (created, trigger_ref) VALUES ($1, 'late')")
        .bind(base())
        .execute(&mut *late)
        .await?;
    let low: i64 = sqlx::query_scalar(
        "SELECT id FROM native_summary_invalidation WHERE kind = 'event_volume' AND bucket = $1",
    )
    .bind(base())
    .fetch_one(&mut *late)
    .await?;
    event(pool, base(), "early").await?;
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    assert_eq!(total(pool, base()).await?, 1);
    late.commit().await?;
    assert_eq!(pending(pool, base()).await?, vec![low]);
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    assert_eq!(total(pool, base()).await?, 2);

    let reserved: i64 = sqlx::query_scalar(
        "SELECT nextval(pg_get_serial_sequence('native_summary_invalidation', 'id'))",
    )
    .fetch_one(pool)
    .await?;
    event(pool, base(), "upper").await?;
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    let mut late = pool.begin().await?;
    // Explicit notification accompanies a maintenance-directed leaf write.
    sqlx::query("LOCK TABLE ONLY event IN ACCESS SHARE MODE")
        .execute(&mut *late)
        .await?;
    sqlx::query("INSERT INTO event_default (created, trigger_ref) VALUES ($1, 'reserved')")
        .bind(base())
        .execute(&mut *late)
        .await?;
    sqlx::query("INSERT INTO native_summary_invalidation (id, kind, bucket) VALUES ($1, 'event_volume', $2)")
        .bind(reserved).bind(base()).execute(&mut *late).await?;
    late.commit().await?;
    assert_eq!(pending(pool, base()).await?, vec![reserved]);
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    assert_eq!(total(pool, base()).await?, 4);
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn bootstrap_snapshot_does_not_block_producers_and_second_builder_retries() -> TestResult<()>
{
    let db = fixture().await?;
    let pool = db.pool();
    event(pool, base(), "initial").await?;
    sqlx::query("DELETE FROM native_summary_invalidation")
        .execute(pool)
        .await?;
    let first_pool = named_pool(&db, "native-summary-first").await?;
    let second_pool = named_pool(&db, "native-summary-second").await?;
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE event_volume_hourly_summary IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let first = tokio::spawn({
        let pool = first_pool.clone();
        async move {
            // Deliberate lock barrier must survive catalog coordination while the
            // other four-thread fixtures clone databases. Production/default-budget
            // lock waits are tested separately; this is the serialization probe.
            let config = NativeMaintenanceConfig {
                lock_timeout_milliseconds: 1500,
                ..config()
            };
            Summaries::refresh_bucket_at(
                &pool,
                SummaryKind::EventVolume,
                base(),
                &config,
                base() + Duration::days(1),
            )
            .await
        }
    });
    blocked(pool, "native-summary-first").await?;
    let second = tokio::spawn({
        let pool = second_pool.clone();
        async move {
            let config = NativeMaintenanceConfig {
                lock_timeout_milliseconds: 1500,
                ..config()
            };
            Summaries::refresh_bucket_at(
                &pool,
                SummaryKind::EventVolume,
                base(),
                &config,
                base() + Duration::days(1),
            )
            .await
        }
    });
    blocked(pool, "native-summary-second").await?;
    // Must commit while the first builder holds its per-kind state row FOR UPDATE.
    timeout(StdDuration::from_secs(1), event(pool, base(), "concurrent")).await??;
    blocker.commit().await?;
    let first_result = first.await??;
    assert_eq!(first_result.notifications_processed, 0);
    assert_eq!(first_result.groups_written, 1);
    let second_result = second.await??;
    assert_eq!(second_result.serialization_retries, 1);
    assert_eq!(total(pool, base()).await?, 2);
    assert!(pending(pool, base()).await?.is_empty());
    first_pool.close().await;
    second_pool.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn failed_refresh_rolls_back_deletion_coverage_and_ack_then_restart_converges(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    event(pool, base(), "initial").await?;
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    event(pool, base(), "new").await?;
    let ids = pending(pool, base()).await?;
    let previous: DateTime<Utc> = sqlx::query_scalar(
        "SELECT refreshed_at FROM native_summary_hour WHERE kind = 'event_volume' AND bucket = $1",
    )
    .bind(base())
    .fetch_one(pool)
    .await?;
    // Fail after summary replacement and ledger upsert, at acknowledgement.
    sqlx::raw_sql("CREATE FUNCTION reject_summary_ack() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected acknowledgement failure'; END $$;
        CREATE TRIGGER reject_summary_ack BEFORE DELETE ON native_summary_invalidation FOR EACH STATEMENT EXECUTE FUNCTION reject_summary_ack();")
        .execute(pool).await?;
    assert!(Summaries::refresh_bucket_at(
        pool,
        SummaryKind::EventVolume,
        base(),
        &config(),
        base() + Duration::days(1)
    )
    .await
    .is_err());
    assert_eq!(total(pool, base()).await?, 1);
    assert_eq!(pending(pool, base()).await?, ids);
    let after: DateTime<Utc> = sqlx::query_scalar(
        "SELECT refreshed_at FROM native_summary_hour WHERE kind = 'event_volume' AND bucket = $1",
    )
    .bind(base())
    .fetch_one(pool)
    .await?;
    assert_eq!(after, previous);
    sqlx::query("DROP TRIGGER reject_summary_ack ON native_summary_invalidation")
        .execute(pool)
        .await?;
    let restarted = named_pool(&db, "native-summary-restarted").await?;
    refresh(&restarted, SummaryKind::EventVolume, base()).await?;
    assert_eq!(total(pool, base()).await?, 2);
    restarted.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn parent_lock_wait_precedes_snapshot_and_state_and_is_bounded() -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    let builder = named_pool(&db, "native-summary-parent-wait").await?;
    let mut expiry = pool.begin().await?;
    sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *expiry)
        .await?;
    let config = NativeMaintenanceConfig {
        lock_timeout_milliseconds: 150,
        ..config()
    };
    let started = Instant::now();
    let task = tokio::spawn({
        let pool = builder.clone();
        async move {
            Summaries::refresh_bucket_at(
                &pool,
                SummaryKind::EventVolume,
                base(),
                &config,
                base() + Duration::days(1),
            )
            .await
        }
    });
    blocked(pool, "native-summary-parent-wait").await?;
    // Builder has not taken the state lock while waiting for its source parent.
    let mut state_probe = pool.begin().await?;
    sqlx::query(
        "SELECT kind FROM native_summary_state WHERE kind = 'event_volume' FOR UPDATE NOWAIT",
    )
    .fetch_one(&mut *state_probe)
    .await?;
    state_probe.rollback().await?;
    let error = task.await?.unwrap_err();
    assert!(
        matches!(error, attune_common::Error::Database(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("55P03"))
    );
    assert!(started.elapsed() < StdDuration::from_secs(2));
    expiry.rollback().await?;
    refresh(&builder, SummaryKind::EventVolume, base()).await?;
    builder.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn cycle_bootstraps_recent_then_backfills_holes_with_fair_caps_and_keep_forever(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    let now = base() + Duration::hours(60);
    let config = NativeMaintenanceConfig {
        max_summary_buckets_per_cycle: 8,
        summary_bootstrap_hours: 2,
        max_summary_cycle_milliseconds: 5000,
        ..config()
    };
    let mut targets = RetentionTargetsConfig::default();
    targets.events.max_age_seconds = None;
    targets.execution_history.max_age_seconds = Some(3 * 3600);
    targets.worker_history.max_age_seconds = Some(3 * 3600);
    // No notification is needed to bootstrap existing historical data.
    event(pool, now - Duration::hours(5), "historical").await?;
    sqlx::query("DELETE FROM native_summary_invalidation")
        .execute(pool)
        .await?;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let stopped = Summaries::refresh_cycle(pool, &config, &targets, now, &cancelled).await?;
    assert!(stopped.cancelled);
    assert_eq!(stopped.buckets_processed, 0);
    let cancellation = CancellationToken::new();
    let first = Summaries::refresh_cycle(pool, &config, &targets, now, &cancellation).await?;
    assert!(first.failures.is_empty(), "{:?}", first.failures);
    assert_eq!(first.buckets_processed, 8);
    for kind in SummaryKind::ALL {
        let progress = Summaries::progress(pool, kind, &config, &targets, now).await?;
        assert_eq!(progress.bootstrap_missing_hours, 0);
    }
    let second = Summaries::refresh_cycle(pool, &config, &targets, now, &cancellation).await?;
    assert!(second.failures.is_empty(), "{:?}", second.failures);
    assert!(second.buckets_processed <= 8);
    // Restart with a new connection and repair a historical hole inside extrema.
    let hole = now - Duration::hours(3);
    sqlx::query("DELETE FROM native_summary_hour WHERE kind = 'event_volume' AND bucket = $1")
        .bind(hole)
        .execute(pool)
        .await?;
    let restarted = named_pool(&db, "native-summary-cycle-restart").await?;
    for _ in 0..3 {
        let result =
            Summaries::refresh_cycle(&restarted, &config, &targets, now, &cancellation).await?;
        assert!(result.failures.is_empty(), "{:?}", result.failures);
    }
    let covered: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM native_summary_hour WHERE kind = 'event_volume' AND bucket = $1)")
        .bind(hole).fetch_one(pool).await?;
    assert!(covered);
    assert_eq!(total(pool, now - Duration::hours(5)).await?, 1);
    sqlx::query("DELETE FROM event WHERE created = $1")
        .bind(now - Duration::hours(5))
        .execute(pool)
        .await?;
    let deletion =
        Summaries::refresh_cycle(&restarted, &config, &targets, now, &cancellation).await?;
    assert!(deletion.failures.is_empty(), "{:?}", deletion.failures);
    assert_eq!(total(pool, now - Duration::hours(5)).await?, 0);
    assert!(pending(pool, now - Duration::hours(5)).await?.is_empty());
    let statuses = Summaries::status(pool).await?;
    assert!(statuses.iter().all(|status| status.coverage_hours >= 2));
    restarted.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn historical_dirty_hours_outside_coverage_are_serviced_but_expired_and_open_hours_are_not(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    let now = base() + Duration::hours(60);
    let config = NativeMaintenanceConfig {
        max_summary_buckets_per_cycle: 12,
        summary_bootstrap_hours: 2,
        ..config()
    };
    let mut targets = RetentionTargetsConfig::default();
    targets.events.max_age_seconds = Some(10 * 3600);
    let retained = now - Duration::hours(9);
    let expired = now - Duration::hours(11);
    event(pool, retained, "old-retained").await?;
    event(pool, expired, "expired-default").await?;
    event(pool, now, "open-hour").await?;
    let result =
        Summaries::refresh_cycle(pool, &config, &targets, now, &CancellationToken::new()).await?;
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    assert_eq!(total(pool, retained).await?, 1);
    assert!(pending(pool, retained).await?.is_empty());
    assert!(!pending(pool, expired).await?.is_empty());
    assert!(!pending(pool, now).await?.is_empty());
    for bucket in [expired, now] {
        let covered: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM native_summary_hour WHERE kind = 'event_volume' AND bucket = $1)")
            .bind(bucket).fetch_one(pool).await?;
        assert!(!covered);
    }
    assert!(
        Summaries::refresh_bucket_at(pool, SummaryKind::EventVolume, now, &config, now)
            .await
            .is_err()
    );
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn cancellation_during_refresh_preserves_confirmed_cycle_progress_and_releases_locks(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    let now = base() + Duration::hours(60);
    let config = NativeMaintenanceConfig {
        max_summary_buckets_per_cycle: 8,
        summary_bootstrap_hours: 2,
        ..config()
    };
    let offset = now
        .timestamp()
        .div_euclid(config.summary_interval_seconds as i64)
        .rem_euclid(4) as usize;
    let blocked_kind = SummaryKind::ALL[(offset + 1) % 4];
    let mut blocker = pool.begin().await?;
    sqlx::query(&format!(
        "LOCK TABLE {} IN ACCESS EXCLUSIVE MODE",
        blocked_kind.summary_table()
    ))
    .execute(&mut *blocker)
    .await?;
    let builder = named_pool(&db, "native-summary-cancelled-cycle").await?;
    let cancellation = CancellationToken::new();
    let resume_config = config.clone();
    let run = tokio::spawn({
        let pool = builder.clone();
        let cancellation = cancellation.clone();
        async move {
            Summaries::refresh_cycle(
                &pool,
                &config,
                &RetentionTargetsConfig::default(),
                now,
                &cancellation,
            )
            .await
        }
    });
    blocked(pool, "native-summary-cancelled-cycle").await?;
    cancellation.cancel();
    let result = timeout(StdDuration::from_secs(3), run).await???;
    assert!(result.cancelled);
    assert_eq!(result.buckets_processed, 1);
    assert_eq!(result.deadline_failures, 0);
    assert_eq!(result.failures.len(), 1);
    let covered: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM native_summary_hour")
        .fetch_one(pool)
        .await?;
    assert_eq!(covered, 1);
    // Rollback is awaited even though the SQL future was cancelled mid-lock wait.
    let mut probe = pool.begin().await?;
    sqlx::query("SELECT kind FROM native_summary_state WHERE kind = $1 FOR UPDATE NOWAIT")
        .bind(blocked_kind)
        .fetch_one(&mut *probe)
        .await?;
    probe.rollback().await?;
    blocker.rollback().await?;
    let resumed = Summaries::refresh_cycle(
        &builder,
        &resume_config,
        &RetentionTargetsConfig::default(),
        now,
        &CancellationToken::new(),
    )
    .await?;
    assert!(resumed.failures.is_empty(), "{:?}", resumed.failures);
    builder.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn busy_kind_does_not_hide_other_kinds_progress_or_pending_lag() -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    let now = base() + Duration::hours(60);
    let config = NativeMaintenanceConfig {
        max_summary_buckets_per_cycle: 8,
        summary_bootstrap_hours: 2,
        lock_timeout_milliseconds: 100,
        ..config()
    };
    event(pool, now - Duration::hours(1), "pending").await?;
    let mut busy = pool.begin().await?;
    sqlx::query("SELECT kind FROM native_summary_state WHERE kind = 'event_volume' FOR UPDATE")
        .fetch_one(&mut *busy)
        .await?;
    let result = Summaries::refresh_cycle(
        pool,
        &config,
        &RetentionTargetsConfig::default(),
        now,
        &CancellationToken::new(),
    )
    .await?;
    assert_eq!(result.buckets_processed, 6);
    assert_eq!(result.lock_failures, 2);
    let status = Summaries::status(pool).await?;
    let events = status
        .iter()
        .find(|s| s.kind == SummaryKind::EventVolume)
        .unwrap();
    assert_eq!(events.coverage_hours, 0);
    assert_eq!(events.dirty_notifications, 1);
    assert_eq!(events.oldest_dirty_bucket, Some(now - Duration::hours(1)));
    assert!(events.oldest_notification.is_some());
    busy.rollback().await?;
    let next = Summaries::refresh_cycle(
        pool,
        &config,
        &RetentionTargetsConfig::default(),
        now,
        &CancellationToken::new(),
    )
    .await?;
    assert!(next.failures.is_empty(), "{:?}", next.failures);
    assert!(pending(pool, now - Duration::hours(1)).await?.is_empty());
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn clean_completed_coverage_does_no_source_recomputation_and_small_caps_rotate_kinds(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    let now = base() + Duration::hours(60);
    let config = NativeMaintenanceConfig {
        max_summary_buckets_per_cycle: 1,
        summary_bootstrap_hours: 1,
        ..config()
    };
    let mut targets = RetentionTargetsConfig::default();
    for target in [
        &mut targets.events,
        &mut targets.execution_history,
        &mut targets.worker_history,
    ] {
        target.max_age_seconds = Some(3600);
    }
    for tick in 0..4 {
        let clock = now + Duration::seconds(tick * config.summary_interval_seconds as i64);
        let result =
            Summaries::refresh_cycle(pool, &config, &targets, clock, &CancellationToken::new())
                .await?;
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert_eq!(result.buckets_processed, 1);
    }
    let before: Vec<(SummaryKind, DateTime<Utc>)> =
        sqlx::query_as("SELECT kind, refreshed_at FROM native_summary_hour ORDER BY kind")
            .fetch_all(pool)
            .await?;
    assert_eq!(before.len(), 4);
    // A source aggregation would now fail, proving a clean cycle does not run it.
    sqlx::raw_sql("CREATE FUNCTION reject_recomputation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'unexpected clean-hour rebuild'; END $$;
        CREATE TRIGGER reject_recomputation BEFORE INSERT ON event_volume_hourly_summary FOR EACH STATEMENT EXECUTE FUNCTION reject_recomputation();")
        .execute(pool).await?;
    let idle =
        Summaries::refresh_cycle(pool, &config, &targets, now, &CancellationToken::new()).await?;
    assert!(idle.failures.is_empty(), "{:?}", idle.failures);
    assert_eq!(idle.buckets_processed, 0);
    let after: Vec<(SummaryKind, DateTime<Utc>)> =
        sqlx::query_as("SELECT kind, refreshed_at FROM native_summary_hour ORDER BY kind")
            .fetch_all(pool)
            .await?;
    assert_eq!(after, before);
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn timeout_configuration_does_not_evict_reusable_prepared_refresh_queries() -> TestResult<()>
{
    let db = fixture().await?;
    let builder = named_pool(&db, "native-summary-prepared-budget").await?;
    for hour in 0..6 {
        for kind in SummaryKind::ALL {
            Summaries::refresh_bucket_at(
                &builder,
                kind,
                base() + Duration::hours(hour),
                &NativeMaintenanceConfig::default(),
                base() + Duration::days(1),
            )
            .await?;
        }
    }
    let timeout_plans: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM pg_prepared_statements WHERE statement LIKE 'SET LOCAL statement_timeout%'",
    ).fetch_one(&builder).await?;
    assert_eq!(
        timeout_plans, 0,
        "one-off remaining timeout values must not consume the prepared-plan cache"
    );
    let enum_discovery: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM pg_prepared_statements WHERE statement NOT LIKE '%pg_prepared_statements%' AND (statement LIKE 'SELECT $1::regtype::oid%' OR statement LIKE '%FROM pg_catalog.pg_enum%')",
    ).fetch_one(&builder).await?;
    assert_eq!(enum_discovery, 0, "cold summary refresh must use built-in parameter/result OIDs without extra enum discovery queries");
    let generic_source_plans: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(generic_plans),0)::bigint FROM pg_prepared_statements WHERE statement LIKE 'WITH refreshed AS%'",
    ).fetch_one(&builder).await?;
    assert_eq!(
        generic_source_plans, 0,
        "the sixth hourly refresh must retain plan-time source partition pruning"
    );
    builder.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn cache_only_async_commit_is_local_after_success_failure_and_cancellation() -> TestResult<()>
{
    let db = fixture().await?;
    let builder = named_pool(&db, "native-summary-async-policy").await?;
    let before = session_commit_policy(&builder).await?;
    assert_eq!(before.1, "on");
    sqlx::raw_sql("CREATE FUNCTION require_cache_async() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF current_setting('synchronous_commit') <> 'off' THEN RAISE EXCEPTION 'cache commit must be async'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER require_cache_async BEFORE INSERT ON native_summary_hour FOR EACH ROW EXECUTE FUNCTION require_cache_async();
        CREATE FUNCTION require_source_sync() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF current_setting('synchronous_commit') <> 'on' THEN RAISE EXCEPTION 'source commit must stay synchronous'; END IF; RETURN NEW; END $$;
        CREATE TRIGGER require_source_sync BEFORE INSERT ON event FOR EACH ROW EXECUTE FUNCTION require_source_sync();")
        .execute(db.pool()).await?;
    event(&builder, base(), "sync-source").await?;
    refresh(&builder, SummaryKind::EventVolume, base()).await?;
    assert_eq!(session_commit_policy(&builder).await?, before);

    event(&builder, base() + Duration::hours(1), "sync-after-cache").await?;
    sqlx::raw_sql("CREATE FUNCTION fail_async_ack() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected async cache error'; END $$;
        CREATE TRIGGER fail_async_ack BEFORE DELETE ON native_summary_invalidation FOR EACH STATEMENT EXECUTE FUNCTION fail_async_ack();")
        .execute(db.pool()).await?;
    assert!(Summaries::refresh_bucket_at(
        &builder,
        SummaryKind::EventVolume,
        base() + Duration::hours(1),
        &config(),
        base() + Duration::days(1)
    )
    .await
    .is_err());
    assert_eq!(session_commit_policy(&builder).await?, before);
    sqlx::query("DROP TRIGGER fail_async_ack ON native_summary_invalidation")
        .execute(db.pool())
        .await?;

    let mut now = base() + Duration::hours(60);
    let offset = now.timestamp().div_euclid(300).rem_euclid(4);
    now += Duration::seconds((2 - offset).rem_euclid(4) * 300);
    let mut blocker = db.pool().begin().await?;
    sqlx::query("LOCK TABLE event_volume_hourly_summary IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let cancellation = CancellationToken::new();
    let run = tokio::spawn({
        let pool = builder.clone();
        let token = cancellation.clone();
        async move {
            let config = NativeMaintenanceConfig {
                max_summary_buckets_per_cycle: 1,
                summary_bootstrap_hours: 1,
                lock_timeout_milliseconds: 1500,
                ..config()
            };
            Summaries::refresh_cycle(
                &pool,
                &config,
                &RetentionTargetsConfig::default(),
                now,
                &token,
            )
            .await
        }
    });
    blocked(db.pool(), "native-summary-async-policy").await?;
    cancellation.cancel();
    assert!(timeout(StdDuration::from_secs(3), run).await???.cancelled);
    assert_eq!(session_commit_policy(&builder).await?, before);
    blocker.rollback().await?;
    event(&builder, base() + Duration::hours(2), "sync-after-cancel").await?;
    builder.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn acknowledged_commit_over_deadline_is_a_failure_with_confirmed_cycle_progress(
) -> TestResult<()> {
    let db = fixture().await?;
    sqlx::raw_sql("CREATE FUNCTION slow_summary_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(0.4); RETURN NEW; END $$;
        CREATE CONSTRAINT TRIGGER slow_summary_commit AFTER INSERT ON native_summary_hour DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION slow_summary_commit();")
        .execute(db.pool()).await?;
    let config = NativeMaintenanceConfig {
        operation_timeout_milliseconds: 200,
        max_summary_buckets_per_cycle: 1,
        summary_bootstrap_hours: 1,
        ..Default::default()
    };
    let result = Summaries::refresh_cycle(
        db.pool(),
        &config,
        &RetentionTargetsConfig::default(),
        base() + Duration::hours(60),
        &CancellationToken::new(),
    )
    .await?;
    assert_eq!(
        result.buckets_processed, 1,
        "acknowledged committed progress must survive the budget failure"
    );
    assert_eq!(result.deadline_failures, 1);
    assert_eq!(result.failures.len(), 1);
    assert!(result.failures[0]
        .message
        .contains("COMMIT acknowledged after"));
    let covered: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM native_summary_hour")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(covered, 1);
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn builder_waiting_for_source_expiry_takes_its_snapshot_only_after_parent_lock(
) -> TestResult<()> {
    let db = fixture().await?;
    let pool = db.pool();
    event(pool, base(), "before-expiry").await?;
    refresh(pool, SummaryKind::EventVolume, base()).await?;
    let mut expiry = pool.begin().await?;
    sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *expiry)
        .await?;
    sqlx::query("DELETE FROM event WHERE created = $1")
        .bind(base())
        .execute(&mut *expiry)
        .await?;
    sqlx::query("DELETE FROM event_volume_hourly_summary WHERE bucket = $1")
        .bind(base())
        .execute(&mut *expiry)
        .await?;
    sqlx::query("DELETE FROM native_summary_hour WHERE kind = 'event_volume' AND bucket = $1")
        .bind(base())
        .execute(&mut *expiry)
        .await?;
    sqlx::query(
        "DELETE FROM native_summary_invalidation WHERE kind = 'event_volume' AND bucket = $1",
    )
    .bind(base())
    .execute(&mut *expiry)
    .await?;
    let builder = named_pool(&db, "native-summary-expiry-snapshot").await?;
    let task = tokio::spawn({
        let pool = builder.clone();
        async move {
            let config = NativeMaintenanceConfig {
                lock_timeout_milliseconds: 1000,
                ..config()
            };
            Summaries::refresh_bucket_at(
                &pool,
                SummaryKind::EventVolume,
                base(),
                &config,
                base() + Duration::days(1),
            )
            .await
        }
    });
    blocked(pool, "native-summary-expiry-snapshot").await?;
    expiry.commit().await?;
    let result = task.await??;
    assert_eq!(result.groups_written, 0);
    assert_eq!(result.notifications_processed, 0);
    assert_eq!(result.serialization_retries, 0);
    assert_eq!(total(pool, base()).await?, 0);
    builder.close().await;
    db.cleanup().await?;
    Ok(())
}
