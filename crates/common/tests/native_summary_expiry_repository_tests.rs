//! Expired materializations must drain independently of remaining raw candidates.

use std::{str::FromStr, time::Duration as StdDuration};

use attune_common::{
    config::{Config, NativeMaintenanceConfig, RetentionConfig},
    repositories::{
        analytics::{AnalyticsRepository, AnalyticsTimeRange},
        native_maintenance::{
            partitions::{utc_day, PartitionRepairOutcome, PartitionRepository},
            read::ReadMode,
            summaries::SummaryRepository,
            ManagedTable, SummaryKind,
        },
        retention::{RetentionRepository, RetentionTarget},
    },
    test_database::TestDatabase,
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use tokio::time::{timeout, Instant};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn fixture() -> TestResult<TestDatabase> {
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    Ok(TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop())
}

async fn seed(pool: &PgPool, kind: SummaryKind, at: DateTime<Utc>, count: i64) -> TestResult {
    if kind == SummaryKind::EventVolume {
        sqlx::query("INSERT INTO event(created,trigger_ref) SELECT $1,'expiry.fixture' FROM generate_series(1,$2::bigint)")
            .bind(at).bind(count).execute(pool).await?;
    } else {
        sqlx::query(&format!("INSERT INTO {}(time,operation,entity_id,entity_ref,changed_fields,new_values)
            SELECT $1,'INSERT',42,'expiry.fixture',ARRAY['status'],'{{\"status\":\"completed\"}}' FROM generate_series(1,$2::bigint)", kind.source_table()))
            .bind(at).bind(count).execute(pool).await?;
    }
    Ok(())
}

async fn scalar(pool: &PgPool, sql: &str) -> TestResult<i64> {
    Ok(sqlx::query_scalar(sql).fetch_one(pool).await?)
}

async fn refresh(pool: &PgPool, kind: SummaryKind, bucket: DateTime<Utc>) -> TestResult {
    SummaryRepository::refresh_bucket(pool, kind, bucket, &NativeMaintenanceConfig::default())
        .await?;
    Ok(())
}

async fn absent(pool: &PgPool, kind: SummaryKind, bucket: DateTime<Utc>) -> TestResult {
    for sql in [
        format!(
            "SELECT count(*)::bigint FROM {} WHERE bucket=$1",
            kind.summary_table()
        ),
        "SELECT count(*)::bigint FROM native_summary_hour WHERE bucket=$1 AND kind=$2".into(),
        "SELECT count(*)::bigint FROM native_summary_invalidation WHERE bucket=$1 AND kind=$2"
            .into(),
    ] {
        let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(bucket);
        if sql.contains("kind=$2") {
            query = query.bind(kind);
        }
        assert_eq!(query.fetch_one(pool).await?, 0, "{sql}");
    }
    Ok(())
}

async fn named_pool(db: &TestDatabase, name: &str) -> TestResult<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            PgConnectOptions::from_str(db.database_url())?
                .application_name(name)
                .options([("search_path", "attune")]),
        )
        .await?)
}

async fn blocked(pool: &PgPool, name: &str) -> TestResult {
    timeout(StdDuration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1 AND cardinality(pg_blocking_pids(pid))>0)")
                .bind(name).fetch_one(pool).await?;
            if waiting { return Ok::<_, sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    Ok(())
}

#[tokio::test]
async fn failed_housekeeping_preserves_raw_progress_then_zero_candidate_restart_drains_all_kinds(
) -> TestResult {
    let db = fixture().await?;
    let bucket = utc_day(Utc::now()) - Duration::days(100);
    for kind in [
        SummaryKind::EventVolume,
        SummaryKind::ExecutionStatus,
        SummaryKind::WorkerStatus,
    ] {
        seed(db.pool(), kind, bucket, 1).await?;
    }
    for kind in SummaryKind::ALL {
        refresh(db.pool(), kind, bucket).await?;
    }
    sqlx::raw_sql("CREATE FUNCTION reject_expiry_ack() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'cleanup failed'; END $$;
        CREATE TRIGGER reject_expiry_ack BEFORE DELETE ON native_summary_invalidation FOR EACH STATEMENT EXECUTE FUNCTION reject_expiry_ack();")
        .execute(db.pool()).await?;
    let settings = RetentionConfig {
        batch_size: 10,
        max_batches_per_target: 1,
        native_maintenance: NativeMaintenanceConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    for target in [
        RetentionTarget::Events,
        RetentionTarget::ExecutionHistory,
        RetentionTarget::WorkerHistory,
    ] {
        let failure =
            RetentionRepository::run_target_bounded(db.pool(), target, 3600, &settings, || false)
                .await
                .unwrap_err();
        assert_eq!(
            (
                failure.candidates,
                failure.deleted,
                failure.partitions_dropped
            ),
            (Some(1), 1, 0)
        );
    }
    // Each cleanup transaction rolled back, but source deletes and their dirty
    // notifications committed. Neither the raw progress nor the dirty flag is lost.
    for kind in SummaryKind::ALL {
        assert_eq!(
            scalar(
                db.pool(),
                &format!("SELECT count(*) FROM {}", kind.summary_table())
            )
            .await?,
            1
        );
    }
    sqlx::query("DROP TRIGGER reject_expiry_ack ON native_summary_invalidation")
        .execute(db.pool())
        .await?;
    let restarted = named_pool(&db, "expiry-restarted").await?;
    for target in [
        RetentionTarget::Events,
        RetentionTarget::ExecutionHistory,
        RetentionTarget::WorkerHistory,
    ] {
        let result =
            RetentionRepository::run_target_bounded(&restarted, target, 3600, &settings, || false)
                .await?;
        assert_eq!(
            (result.candidates, result.deleted, result.partitions_dropped),
            (0, 0, 0)
        );
    }
    for kind in SummaryKind::ALL {
        absent(db.pool(), kind, bucket).await?;
    }
    for status in SummaryRepository::status(db.pool()).await? {
        assert_eq!(
            (
                status.coverage_hours,
                status.dirty_hours,
                status.dirty_notifications
            ),
            (0, 0, 0)
        );
    }
    restarted.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn oversized_expired_default_cleanup_removes_coverage_while_remaining_raw_uses_fallback(
) -> TestResult {
    let db = fixture().await?;
    let bucket = utc_day(Utc::now()) - Duration::days(100);
    seed(db.pool(), SummaryKind::EventVolume, bucket, 5).await?;
    refresh(db.pool(), SummaryKind::EventVolume, bucket).await?;
    let native = NativeMaintenanceConfig {
        default_repair_row_limit: 1,
        ..Default::default()
    };
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, bucket, &native).await?,
        PartitionRepairOutcome::DeferredOverBudget { rows_at_least: 2 }
    );
    let settings = RetentionConfig {
        batch_size: 2,
        max_batches_per_target: 1,
        native_maintenance: native,
        ..Default::default()
    };
    let result = RetentionRepository::run_target_bounded(
        db.pool(),
        RetentionTarget::Events,
        3600,
        &settings,
        || false,
    )
    .await?;
    assert_eq!(
        (result.deleted, result.partitions_dropped, result.candidates),
        (2, 0, 3)
    );
    assert!(!result.candidates_exact);
    absent(db.pool(), SummaryKind::EventVolume, bucket).await?;
    let mut conn = db.pool().acquire().await?;
    let read = AnalyticsRepository::event_volume_hourly(
        &mut *conn,
        &AnalyticsTimeRange {
            since: bucket,
            until: bucket + Duration::hours(1) - Duration::nanoseconds(1),
        },
    )
    .await?;
    assert_eq!(read.metadata.mode, ReadMode::RawOnly);
    assert_eq!(read.data.iter().map(|row| row.event_count).sum::<i64>(), 3);
    drop(conn);
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn housekeeping_failure_does_not_discard_confirmed_leaf_and_row_progress() -> TestResult {
    let db = fixture().await?;
    let start = utc_day(Utc::now()) - Duration::days(100);
    let native = NativeMaintenanceConfig {
        default_repair_row_limit: 1,
        ..Default::default()
    };
    let setup = NativeMaintenanceConfig {
        operation_timeout_milliseconds: 10_000,
        ..native.clone()
    };
    let created = PartitionRepository::ensure_day(
        db.pool(),
        ManagedTable::Event,
        start - Duration::days(1),
        &setup,
    )
    .await?;
    assert_eq!(created, PartitionRepairOutcome::Applied { rows_moved: 0 });
    seed(
        db.pool(),
        SummaryKind::EventVolume,
        start - Duration::days(1),
        3,
    )
    .await?;
    seed(db.pool(), SummaryKind::EventVolume, start, 5).await?;
    refresh(db.pool(), SummaryKind::EventVolume, start).await?;
    sqlx::raw_sql("CREATE FUNCTION reject_group_expiry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected group cleanup failure'; END $$;
        CREATE TRIGGER reject_group_expiry BEFORE DELETE ON event_volume_hourly_summary FOR EACH ROW EXECUTE FUNCTION reject_group_expiry();")
        .execute(db.pool()).await?;
    let settings = RetentionConfig {
        batch_size: 2,
        max_batches_per_target: 1,
        native_maintenance: native,
        ..Default::default()
    };
    let failure = RetentionRepository::run_target_bounded(
        db.pool(),
        RetentionTarget::Events,
        3600,
        &settings,
        || false,
    )
    .await
    .unwrap_err();
    assert_eq!(
        (
            failure.partitions_dropped,
            failure.partition_candidates,
            failure.deleted,
            failure.candidates
        ),
        (1, 1, 2, Some(3))
    );
    assert!(!failure.candidates_exact);
    assert_eq!(scalar(db.pool(), "SELECT count(*) FROM event").await?, 3);
    assert_eq!(
        scalar(db.pool(), "SELECT count(*) FROM native_summary_hour").await?,
        1
    );
    sqlx::query("DROP TRIGGER reject_group_expiry ON event_volume_hourly_summary")
        .execute(db.pool())
        .await?;
    let resumed = RetentionRepository::run_target_bounded(
        db.pool(),
        RetentionTarget::Events,
        3600,
        &settings,
        || false,
    )
    .await?;
    assert_eq!((resumed.deleted, resumed.partitions_dropped), (2, 0));
    absent(db.pool(), SummaryKind::EventVolume, start).await?;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_disabled_cleanup_preserves_partial_cutoff_hour_and_dry_run_does_not_mutate(
) -> TestResult {
    let db = fixture().await?;
    let start = utc_day(Utc::now()) - Duration::days(2);
    let cutoff = start + Duration::hours(2) + Duration::minutes(30);
    let age = (Utc::now() - cutoff).num_seconds() as u64;
    for kind in [
        SummaryKind::EventVolume,
        SummaryKind::ExecutionStatus,
        SummaryKind::WorkerStatus,
    ] {
        for at in [
            start,
            start + Duration::hours(2),
            start + Duration::hours(2) + Duration::minutes(45),
        ] {
            seed(db.pool(), kind, at, 1).await?;
        }
    }
    for kind in SummaryKind::ALL {
        refresh(db.pool(), kind, start).await?;
        refresh(db.pool(), kind, start + Duration::hours(2)).await?;
    }
    let mut settings = RetentionConfig {
        batch_size: 10,
        max_batches_per_target: 1,
        dry_run: true,
        native_maintenance: NativeMaintenanceConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    for target in [
        RetentionTarget::Events,
        RetentionTarget::ExecutionHistory,
        RetentionTarget::WorkerHistory,
    ] {
        let dry =
            RetentionRepository::run_target_bounded(db.pool(), target, age, &settings, || false)
                .await?;
        assert_eq!((dry.candidates, dry.deleted), (2, 0));
    }
    assert_eq!(
        scalar(db.pool(), "SELECT count(*) FROM native_summary_hour").await?,
        8
    );
    settings.dry_run = false;
    for target in [
        RetentionTarget::Events,
        RetentionTarget::ExecutionHistory,
        RetentionTarget::WorkerHistory,
    ] {
        let result =
            RetentionRepository::run_target_bounded(db.pool(), target, age, &settings, || false)
                .await?;
        assert_eq!((result.deleted, result.partitions_dropped), (2, 0));
    }
    for kind in SummaryKind::ALL {
        absent(db.pool(), kind, start).await?;
        let covered: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM native_summary_hour WHERE kind=$1 AND bucket=$2)",
        )
        .bind(kind)
        .bind(start + Duration::hours(2))
        .fetch_one(db.pool())
        .await?;
        assert!(covered, "partial hour must be rebuilt, not purged");
        let dirty: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM native_summary_invalidation WHERE kind=$1 AND bucket=$2)",
        )
        .bind(kind)
        .bind(start + Duration::hours(2))
        .fetch_one(db.pool())
        .await?;
        assert!(dirty);
        refresh(db.pool(), kind, start + Duration::hours(2)).await?;
        assert_eq!(
            scalar(
                db.pool(),
                &format!(
                    "SELECT sum({})::bigint FROM {}",
                    match kind {
                        SummaryKind::EventVolume => "event_count",
                        SummaryKind::ExecutionCreation => "execution_count",
                        _ => "transition_count",
                    },
                    kind.summary_table()
                )
            )
            .await?,
            1
        );
    }
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn expiry_hour_notification_deadline_and_cancellation_budgets_leave_raw_unchanged(
) -> TestResult {
    let db = fixture().await?;
    let start = utc_day(Utc::now()) - Duration::days(100);
    for hour in 0..3 {
        seed(
            db.pool(),
            SummaryKind::EventVolume,
            start + Duration::hours(hour),
            1,
        )
        .await?;
        refresh(
            db.pool(),
            SummaryKind::EventVolume,
            start + Duration::hours(hour),
        )
        .await?;
    }
    sqlx::query("INSERT INTO native_summary_invalidation(kind,bucket) SELECT 'event_volume',$1 + h*interval '1 hour' FROM generate_series(0,2) h CROSS JOIN generate_series(1,5)")
        .bind(start).execute(db.pool()).await?;
    let settings = NativeMaintenanceConfig {
        max_summary_buckets_per_cycle: 2,
        max_summary_invalidations_per_bucket: 2,
        ..Default::default()
    };
    let kinds = &[SummaryKind::EventVolume];
    let invalid = NativeMaintenanceConfig {
        max_summary_invalidations_per_bucket: 0,
        ..settings.clone()
    };
    assert!(
        SummaryRepository::expire_before(db.pool(), kinds, Utc::now(), &invalid, || false)
            .await
            .is_err()
    );
    let mut checks = 0;
    let cancelled =
        SummaryRepository::expire_before(db.pool(), kinds, Utc::now(), &settings, || {
            checks += 1;
            checks == 2
        })
        .await?;
    assert!(cancelled.cancelled);
    assert_eq!(
        (cancelled.hours_processed, cancelled.notifications_processed),
        (0, 0)
    );
    assert_eq!(
        scalar(db.pool(), "SELECT count(*) FROM native_summary_hour").await?,
        3
    );
    assert_eq!(
        scalar(
            db.pool(),
            "SELECT count(*) FROM native_summary_invalidation"
        )
        .await?,
        15
    );
    sqlx::raw_sql("CREATE FUNCTION delay_expiry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(0.3); RETURN OLD; END $$;
        CREATE TRIGGER delay_expiry BEFORE DELETE ON event_volume_hourly_summary FOR EACH ROW EXECUTE FUNCTION delay_expiry();")
        .execute(db.pool()).await?;
    let short = NativeMaintenanceConfig {
        lock_timeout_milliseconds: 50,
        operation_timeout_milliseconds: 100,
        ..settings.clone()
    };
    let started = Instant::now();
    assert!(
        SummaryRepository::expire_before(db.pool(), kinds, Utc::now(), &short, || false)
            .await
            .is_err()
    );
    assert!(started.elapsed() < StdDuration::from_secs(1));
    assert_eq!(
        scalar(db.pool(), "SELECT count(*) FROM native_summary_hour").await?,
        3
    );
    assert_eq!(
        scalar(
            db.pool(),
            "SELECT count(*) FROM native_summary_invalidation"
        )
        .await?,
        15
    );
    sqlx::query("DROP TRIGGER delay_expiry ON event_volume_hourly_summary")
        .execute(db.pool())
        .await?;
    let first =
        SummaryRepository::expire_before(db.pool(), kinds, Utc::now(), &settings, || false).await?;
    assert_eq!(
        (first.hours_processed, first.notifications_processed),
        (2, 4)
    );
    assert!(first.budget_exhausted);
    assert_eq!(
        scalar(db.pool(), "SELECT count(*) FROM native_summary_hour").await?,
        2
    );
    assert_eq!(
        scalar(
            db.pool(),
            "SELECT count(*) FROM native_summary_invalidation"
        )
        .await?,
        11
    );
    for _ in 0..4 {
        let result =
            SummaryRepository::expire_before(db.pool(), kinds, Utc::now(), &settings, || false)
                .await?;
        assert!(result.hours_processed <= 2 && result.notifications_processed <= 4);
    }
    for hour in 0..3 {
        absent(
            db.pool(),
            SummaryKind::EventVolume,
            start + Duration::hours(hour),
        )
        .await?;
    }
    assert_eq!(scalar(db.pool(), "SELECT count(*) FROM event").await?, 3);
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn expiry_acknowledges_only_visible_ids_and_late_lower_id_stays_pending() -> TestResult {
    let db = fixture().await?;
    let start = utc_day(Utc::now()) - Duration::days(100);
    seed(db.pool(), SummaryKind::EventVolume, start, 1).await?;
    refresh(db.pool(), SummaryKind::EventVolume, start).await?;
    let mut late = db.pool().begin().await?;
    sqlx::query("INSERT INTO event(created,trigger_ref) VALUES($1,'late.fixture')")
        .bind(start)
        .execute(&mut *late)
        .await?;
    let low: i64 = sqlx::query_scalar("SELECT id FROM native_summary_invalidation WHERE bucket=$1")
        .bind(start)
        .fetch_one(&mut *late)
        .await?;
    seed(db.pool(), SummaryKind::EventVolume, start, 1).await?;
    let mut barrier = db.pool().begin().await?;
    sqlx::query("SELECT bucket FROM event_volume_hourly_summary WHERE bucket=$1 FOR UPDATE")
        .bind(start)
        .fetch_one(&mut *barrier)
        .await?;
    let expirer = named_pool(&db, "expiry-late-id").await?;
    let task = tokio::spawn({
        let pool = expirer.clone();
        async move {
            let config = NativeMaintenanceConfig {
                max_summary_buckets_per_cycle: 1,
                ..Default::default()
            };
            SummaryRepository::expire_before(
                &pool,
                &[SummaryKind::EventVolume],
                Utc::now(),
                &config,
                || false,
            )
            .await
        }
    });
    blocked(db.pool(), "expiry-late-id").await?;
    // The producer does not reference builder state, so it commits while expiry
    // owns that state lock. Its lower ID was invisible to the captured snapshot.
    timeout(StdDuration::from_secs(1), late.commit()).await??;
    barrier.rollback().await?;
    let result = task.await??;
    assert_eq!(result.hours_processed, 1);
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM native_summary_invalidation WHERE bucket=$1 ORDER BY id",
    )
    .bind(start)
    .fetch_all(db.pool())
    .await?;
    assert_eq!(ids, vec![low]);
    assert_eq!(scalar(db.pool(), "SELECT count(*) FROM event").await?, 3);
    assert_eq!(
        scalar(db.pool(), "SELECT count(*) FROM native_summary_hour").await?,
        0
    );
    expirer.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn builder_waiting_for_cleanup_retries_instead_of_republishing_its_old_snapshot() -> TestResult
{
    let db = fixture().await?;
    let start = utc_day(Utc::now()) - Duration::days(100);
    seed(db.pool(), SummaryKind::EventVolume, start, 1).await?;
    refresh(db.pool(), SummaryKind::EventVolume, start).await?;
    let expirer = named_pool(&db, "expiry-before-builder").await?;
    let builder = named_pool(&db, "builder-after-expiry").await?;
    let race_config = NativeMaintenanceConfig {
        // This probe deliberately coordinates two lock waits and a producer
        // commit. It is not the separate 250-ms parent-acquisition test. Keep
        // the normal one-second operation deadline, including both barriers.
        lock_timeout_milliseconds: 750,
        max_summary_buckets_per_cycle: 1,
        ..Default::default()
    };
    // Prepare both connections before establishing the measured ordering.
    refresh(&builder, SummaryKind::EventVolume, start).await?;
    SummaryRepository::expire_before(
        &expirer,
        &[SummaryKind::EventVolume],
        start,
        &race_config,
        || false,
    )
    .await?;
    let mut barrier = db.pool().begin().await?;
    sqlx::query("SELECT bucket FROM event_volume_hourly_summary WHERE bucket=$1 FOR UPDATE")
        .bind(start)
        .fetch_one(&mut *barrier)
        .await?;
    let cleanup = tokio::spawn({
        let pool = expirer.clone();
        let config = race_config.clone();
        async move {
            SummaryRepository::expire_before(
                &pool,
                &[SummaryKind::EventVolume],
                Utc::now(),
                &config,
                || false,
            )
            .await
        }
    });
    blocked(db.pool(), "expiry-before-builder").await?;
    let refresh = tokio::spawn({
        let pool = builder.clone();
        let config = race_config;
        async move {
            SummaryRepository::refresh_bucket(&pool, SummaryKind::EventVolume, start, &config).await
        }
    });
    blocked(db.pool(), "builder-after-expiry").await?;
    seed(db.pool(), SummaryKind::EventVolume, start, 1).await?;
    barrier.rollback().await?;
    let cleaned = cleanup.await?;
    let refreshed = refresh.await?;
    assert_eq!(cleaned?.hours_processed, 1);
    let refreshed = refreshed?;
    assert_eq!(refreshed.serialization_retries, 1);
    assert_eq!(
        scalar(
            db.pool(),
            "SELECT sum(event_count)::bigint FROM event_volume_hourly_summary"
        )
        .await?,
        2
    );
    expirer.close().await;
    builder.close().await;
    db.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn cleanup_parent_lock_precedes_snapshot_and_state_and_timeout_leaves_everything_intact(
) -> TestResult {
    let db = fixture().await?;
    let start = utc_day(Utc::now()) - Duration::days(100);
    seed(db.pool(), SummaryKind::EventVolume, start, 1).await?;
    refresh(db.pool(), SummaryKind::EventVolume, start).await?;
    let mut parent = db.pool().begin().await?;
    sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *parent)
        .await?;
    let expirer = named_pool(&db, "expiry-parent-first").await?;
    let task = tokio::spawn({
        let pool = expirer.clone();
        async move {
            SummaryRepository::expire_before(
                &pool,
                &[SummaryKind::EventVolume],
                Utc::now(),
                &NativeMaintenanceConfig::default(),
                || false,
            )
            .await
        }
    });
    blocked(db.pool(), "expiry-parent-first").await?;
    let mut probe = db.pool().begin().await?;
    sqlx::query(
        "SELECT kind FROM native_summary_state WHERE kind='event_volume' FOR UPDATE NOWAIT",
    )
    .fetch_one(&mut *probe)
    .await?;
    probe.rollback().await?;
    let error = task.await?.unwrap_err();
    assert!(
        matches!(error, attune_common::Error::Database(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("55P03"))
    );
    parent.rollback().await?;
    assert_eq!(scalar(db.pool(), "SELECT count(*) FROM event").await?, 1);
    assert_eq!(
        scalar(db.pool(), "SELECT count(*) FROM native_summary_hour").await?,
        1
    );
    let result = SummaryRepository::expire_before(
        &expirer,
        &[SummaryKind::EventVolume],
        Utc::now(),
        &NativeMaintenanceConfig::default(),
        || false,
    )
    .await?;
    assert_eq!(result.hours_processed, 1);
    absent(db.pool(), SummaryKind::EventVolume, start).await?;
    expirer.close().await;
    db.cleanup().await?;
    Ok(())
}
