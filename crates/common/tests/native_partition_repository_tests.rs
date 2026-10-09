//! Real PostgreSQL contracts for the bounded partition repository.
//! Each test owns a physical database clone, including committed DDL and races.

use attune_common::{
    config::{Config, NativeMaintenanceConfig, RetentionConfig},
    repositories::{
        native_maintenance::{
            partitions::{utc_day, PartitionRepairOutcome, PartitionRepository},
            ManagedTable,
        },
        retention::{RetentionRepository, RetentionTarget},
    },
    test_database::TestDatabase,
};
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

async fn database() -> TestDatabase {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop()
}

fn day(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn config() -> NativeMaintenanceConfig {
    NativeMaintenanceConfig {
        default_repair_row_limit: 3,
        lock_timeout_milliseconds: 80,
        operation_timeout_milliseconds: 10_000,
        max_partition_cycle_milliseconds: 20_000,
        ..Default::default()
    }
}

async fn events(pool: &PgPool, time: DateTime<Utc>, count: i64) {
    sqlx::query("INSERT INTO event(created,trigger_ref) SELECT $1,'partition.fixture' FROM generate_series(1,$2::bigint)")
        .bind(time).bind(count).execute(pool).await.unwrap();
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn oversized_default_day_defers_and_atomic_repair_preserves_parent_visibility() {
    let db = database().await;
    let start = day("2020-01-02T00:00:00Z");
    events(db.pool(), start + Duration::hours(3), 4).await;
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
            .await
            .unwrap(),
        PartitionRepairOutcome::DeferredOverBudget { rows_at_least: 4 }
    );
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        4
    );
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 4);
    assert_eq!(
        count(
            db.pool(),
            "SELECT count(*) FROM native_partition_registry WHERE lower_bound='2020-01-02'"
        )
        .await,
        0
    );
    let mut settings = config();
    settings.default_repair_row_limit = 4;
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &settings)
            .await
            .unwrap(),
        PartitionRepairOutcome::Applied { rows_moved: 4 }
    );
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        0
    );
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 4);
    assert_eq!(
        count(
            db.pool(),
            "SELECT count(*) FROM native_summary_invalidation WHERE bucket='2020-01-02 03:00+00'"
        )
        .await,
        2
    );
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &settings)
            .await
            .unwrap(),
        PartitionRepairOutcome::AlreadyPresent
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn busy_default_repair_returns_typed_outcome_then_retries_after_reader_unlocks() {
    let db = database().await;
    let start = day("2020-02-02T00:00:00Z");
    events(db.pool(), start, 3).await;
    let mut reader = db.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE ONLY event IN ACCESS SHARE MODE")
        .execute(&mut *reader)
        .await
        .unwrap();
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
            .await
            .unwrap(),
        PartitionRepairOutcome::DeferredBusy
    );
    reader.rollback().await.unwrap();
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        3
    );
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
            .await
            .unwrap(),
        PartitionRepairOutcome::Applied { rows_moved: 3 }
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn operation_deadline_rolls_back_destination_and_source_move() {
    let db = database().await;
    let start = day("2020-03-02T00:00:00Z");
    events(db.pool(), start, 3).await;
    // Install a transaction-local leaf DELETE delay on this owned fixture. The
    // server statement timeout interrupts after the physical move has begun.
    sqlx::raw_sql("CREATE FUNCTION fixture_slow_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(1); RETURN OLD; END $$; CREATE TRIGGER fixture_slow_delete BEFORE DELETE ON event_default FOR EACH ROW EXECUTE FUNCTION fixture_slow_delete();")
        .execute(db.pool()).await.unwrap();
    let settings = NativeMaintenanceConfig {
        operation_timeout_milliseconds: 150,
        ..config()
    };
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &settings)
            .await
            .unwrap(),
        PartitionRepairOutcome::DeferredDeadline
    );
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 3);
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        3
    );
    assert_eq!(count(db.pool(), "SELECT count(*) FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relname='event_p20200302'").await, 0);
    sqlx::query("DROP TRIGGER fixture_slow_delete ON event_default")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
            .await
            .unwrap(),
        PartitionRepairOutcome::Applied { rows_moved: 3 }
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn incompatible_catalog_or_owner_never_gets_adopted_or_dropped() {
    let db = database().await;
    let start = day("2020-04-02T00:00:00Z");
    sqlx::query("CREATE TABLE event_p20200402(LIKE event INCLUDING ALL)")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
            .await
            .is_err()
    );
    sqlx::query("DROP TABLE event_p20200402")
        .execute(db.pool())
        .await
        .unwrap();
    PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
        .await
        .unwrap();
    sqlx::query("UPDATE native_partition_registry SET upper_bound=upper_bound+interval '1 day',lower_bound=lower_bound+interval '1 day' WHERE partition_name='event_p20200402'")
        .execute(db.pool()).await.unwrap();
    let error = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        start + Duration::days(3),
        &config(),
        false,
        || false,
    )
    .await
    .unwrap_err();
    assert_eq!(error.partitions_dropped, 0);
    assert_eq!(count(db.pool(), "SELECT count(*) FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relname='event_p20200402'").await, 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn whole_leaf_expiry_removes_summary_coverage_and_notices_with_separate_drop_count() {
    let db = database().await;
    let start = day("2020-05-02T00:00:00Z");
    PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
        .await
        .unwrap();
    events(db.pool(), start + Duration::hours(1), 10).await;
    sqlx::raw_sql("INSERT INTO event_volume_hourly_summary VALUES('2020-05-02 01:00+00','partition.fixture',10); INSERT INTO native_summary_hour VALUES('event_volume','2020-05-02 01:00+00',now());")
        .execute(db.pool()).await.unwrap();
    let dry = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        start + Duration::days(1),
        &config(),
        true,
        || false,
    )
    .await
    .unwrap();
    assert_eq!((dry.candidates, dry.partitions_dropped), (1, 0));
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 10);
    let result = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        start + Duration::days(1),
        &config(),
        false,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(result.partitions_dropped, 1);
    for relation in [
        "event",
        "event_volume_hourly_summary",
        "native_summary_hour",
        "native_summary_invalidation",
    ] {
        assert_eq!(
            count(db.pool(), &format!("SELECT count(*) FROM {relation}")).await,
            0,
            "{relation}"
        );
    }
    events(db.pool(), start + Duration::hours(1), 1).await;
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        1
    );
    assert_eq!(
        count(
            db.pool(),
            "SELECT count(*) FROM native_summary_invalidation"
        )
        .await,
        1
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancellation_and_failure_preserve_only_confirmed_partition_progress() {
    let db = database().await;
    let start = day("2020-06-02T00:00:00Z");
    for n in 0..3 {
        PartitionRepository::ensure_day(
            db.pool(),
            ManagedTable::Event,
            start + Duration::days(n),
            &config(),
        )
        .await
        .unwrap();
        events(db.pool(), start + Duration::days(n), 1).await;
    }
    let mut checks = 0;
    let cancelled = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        start + Duration::days(4),
        &config(),
        false,
        || {
            checks += 1;
            checks >= 3
        },
    )
    .await
    .unwrap();
    assert_eq!(cancelled.partitions_dropped, 1);
    assert!(cancelled.cancelled);
    // Break the last leaf's registry identity. The preceding valid drop commits.
    sqlx::query("UPDATE native_partition_registry SET partition_name='event_fixture_wrong' WHERE partition_name='event_p20200604'").execute(db.pool()).await.unwrap();
    let error = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        start + Duration::days(4),
        &config(),
        false,
        || false,
    )
    .await
    .unwrap_err();
    assert_eq!(error.partitions_dropped, 1);
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn same_ctid_in_two_leaves_deletes_only_the_selected_tableoid_pair() {
    let db = database().await;
    let start = day("2020-07-02T00:00:00Z");
    for n in 0..2 {
        PartitionRepository::ensure_day(
            db.pool(),
            ManagedTable::ExecutionHistory,
            start + Duration::days(n),
            &config(),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO execution_history(time,operation,entity_id,changed_fields) VALUES($1,'UPDATE',42,ARRAY['status'])")
            .bind(start+Duration::days(n)).execute(db.pool()).await.unwrap();
    }
    assert_eq!(
        count(
            db.pool(),
            "SELECT count(DISTINCT ctid) FROM execution_history"
        )
        .await,
        1
    );
    // Disable DDL expiry to exercise the repository's statement-local TID path.
    let retention = RetentionConfig {
        batch_size: 1,
        max_batches_per_target: 1,
        native_maintenance: NativeMaintenanceConfig {
            enabled: false,
            ..config()
        },
        ..Default::default()
    };
    let result = RetentionRepository::run_target_bounded(
        db.pool(),
        RetentionTarget::ExecutionHistory,
        60,
        &retention,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(
        (result.candidates, result.deleted, result.partitions_dropped),
        (2, 1, 0)
    );
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM execution_history").await,
        1
    );
    assert_eq!(
        count(
            db.pool(),
            "SELECT count(*) FROM native_summary_invalidation"
        )
        .await,
        0
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_retention_uses_metadata_for_full_days_and_bounded_candidates_for_default() {
    let db = database().await;
    let start = day("2020-08-02T00:00:00Z");
    PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
        .await
        .unwrap();
    events(db.pool(), start, 100).await;
    // A different old day remains in DEFAULT, exceeding the two-row batch cap.
    events(db.pool(), start + Duration::days(3), 5).await;
    let retention = RetentionConfig {
        batch_size: 2,
        max_batches_per_target: 1,
        native_maintenance: config(),
        ..Default::default()
    };
    let result = RetentionRepository::run_target_bounded(
        db.pool(),
        RetentionTarget::Events,
        60,
        &retention,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            result.partitions_dropped,
            result.partition_candidates,
            result.deleted,
            result.candidates
        ),
        (1, 1, 2, 3)
    );
    assert!(!result.candidates_exact);
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 3);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn reconcile_retries_future_creation_after_restart_and_has_utc_boundaries() {
    let db = database().await;
    let now = utc_day(Utc::now()) + Duration::days(10);
    let settings = NativeMaintenanceConfig {
        partition_lookahead_days: 1,
        max_partition_operations_per_cycle: 6,
        ..config()
    };
    let first = PartitionRepository::reconcile(db.pool(), &settings, now + Duration::hours(23))
        .await
        .unwrap();
    assert_eq!(first.created, 6);
    let second = PartitionRepository::reconcile(db.pool(), &settings, now)
        .await
        .unwrap();
    assert_eq!(second.created, 0);
    events(db.pool(), now, 1).await;
    events(
        db.pool(),
        now + Duration::days(2) - Duration::microseconds(1),
        1,
    )
    .await;
    events(db.pool(), now + Duration::days(2), 1).await;
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        1
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn small_reconcile_budget_resumes_beyond_already_registered_days() {
    let db = database().await;
    let now = utc_day(Utc::now()) + Duration::days(20);
    let settings = NativeMaintenanceConfig {
        partition_lookahead_days: 1,
        max_partition_operations_per_cycle: 1,
        ..config()
    };
    for _ in 0..6 {
        assert_eq!(
            PartitionRepository::reconcile(db.pool(), &settings, now)
                .await
                .unwrap()
                .created,
            1
        );
    }
    assert_eq!(
        PartitionRepository::reconcile(db.pool(), &settings, now)
            .await
            .unwrap()
            .created,
        0
    );
    let status = PartitionRepository::status(db.pool(), &settings, now)
        .await
        .unwrap();
    for row in status {
        assert_eq!(row.future_partitions, 2);
        assert_eq!(row.missing_future_partitions, 0);
        assert_eq!(row.default_rows_at_least, 0);
        assert!(row.default_count_exact);
    }
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn concurrent_repairers_serialize_on_parent_and_restart_converges() {
    let db = database().await;
    let start = day("2020-11-02T00:00:00Z");
    events(db.pool(), start, 3).await;
    sqlx::raw_sql("CREATE FUNCTION fixture_repair_delay() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(0.5); RETURN OLD; END $$; CREATE TRIGGER fixture_repair_delay BEFORE DELETE ON event_default FOR EACH ROW EXECUTE FUNCTION fixture_repair_delay();")
        .execute(db.pool()).await.unwrap();
    let pool = db.pool().clone();
    let first = tokio::spawn(async move {
        PartitionRepository::ensure_day(&pool, ManagedTable::Event, start, &config()).await
    });
    let ready = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            // Physical clones retain relation OIDs. pg_locks is cluster-wide,
            // so relation alone can observe another concurrently running test.
            let locked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='event'::regclass AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND mode='AccessExclusiveLock' AND granted AND pid <> pg_backend_pid())")
                .fetch_one(db.pool()).await.unwrap();
            if locked { break; }
            tokio::task::yield_now().await;
        }
    }).await;
    let second = if ready.is_ok() {
        Some(
            PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config()).await,
        )
    } else {
        None
    };
    let first = first.await.unwrap();
    // Join the writer before any assertion can unwind and close its database.
    assert!(ready.is_ok(), "first repairer must acquire its parent lock");
    assert_eq!(
        second.unwrap().unwrap(),
        PartitionRepairOutcome::DeferredBusy
    );
    assert_eq!(
        first.unwrap(),
        PartitionRepairOutcome::Applied { rows_moved: 3 }
    );
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &config())
            .await
            .unwrap(),
        PartitionRepairOutcome::AlreadyPresent
    );
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 3);
    assert_eq!(
        count(
            db.pool(),
            "SELECT count(*) FROM native_partition_registry WHERE lower_bound='2020-11-02'"
        )
        .await,
        1
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cap_one_rotates_oversized_future_and_default_days_across_restarts_and_leaders() {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;
    let db = database().await;
    let now = utc_day(Utc::now()) + Duration::days(30);
    let settings = NativeMaintenanceConfig {
        partition_lookahead_days: 1,
        max_partition_operations_per_cycle: 1,
        ..config()
    };
    events(db.pool(), now, 4).await; // The original starving future candidate.
    events(db.pool(), now - Duration::days(2), 4).await; // Persistent oldest backlog.
    events(db.pool(), now - Duration::days(1), 1).await;
    sqlx::query("INSERT INTO execution_history(time,operation,entity_id,changed_fields) VALUES($1,'INSERT',42,ARRAY['status'])")
        .bind(now - Duration::days(1)).execute(db.pool()).await.unwrap();
    let mut leader = db.pool().acquire().await.unwrap();
    let mut standby = db.pool().acquire().await.unwrap();
    let key = 9865321;
    assert!(RetentionRepository::try_advisory_lock(&mut leader, key)
        .await
        .unwrap());
    assert!(!RetentionRepository::try_advisory_lock(&mut standby, key)
        .await
        .unwrap());
    let mut deferred = 0;
    for turn in 0..15 {
        // A fresh pool models process restart. Clock changes do not drive rotation.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(
                PgConnectOptions::from_str(db.database_url())
                    .unwrap()
                    .options([("search_path", "attune")]),
            )
            .await
            .unwrap();
        let result = PartitionRepository::reconcile(&pool, &settings, now)
            .await
            .unwrap();
        assert_eq!(result.attempted, 1);
        assert!(result.created + result.deferred_over_budget <= 1);
        deferred += result.deferred_over_budget;
        pool.close().await;
        if turn == 4 {
            assert!(RetentionRepository::advisory_unlock(&mut leader, key)
                .await
                .unwrap());
            assert!(RetentionRepository::try_advisory_lock(&mut standby, key)
                .await
                .unwrap());
        }
    }
    assert!(deferred > 1);
    let status = PartitionRepository::status(db.pool(), &settings, now)
        .await
        .unwrap();
    for row in status {
        assert_eq!(
            row.missing_future_partitions,
            i64::from(row.parent == ManagedTable::Event)
        );
    }
    assert_eq!(
        count(
            db.pool(),
            "SELECT count(*) FROM ONLY execution_history_default"
        )
        .await,
        0
    );
    let repaired: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_partition_registry WHERE parent='event' AND lower_bound=$1)")
        .bind(now - Duration::days(1)).fetch_one(db.pool()).await.unwrap();
    assert!(
        repaired,
        "a second DEFAULT day must get a turn despite the oversized oldest day"
    );
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 9);
    RetentionRepository::advisory_unlock(&mut standby, key)
        .await
        .unwrap();
    drop(leader);
    drop(standby);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cap_one_deadline_on_oldest_day_does_not_starve_other_days_of_the_same_parent() {
    let db = database().await;
    let now = utc_day(Utc::now()) + Duration::days(40);
    events(db.pool(), now - Duration::days(1), 1).await;
    sqlx::raw_sql("CREATE FUNCTION delay_old_repair() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(2); RETURN OLD; END $$;
        CREATE TRIGGER delay_old_repair BEFORE DELETE ON event_default FOR EACH ROW EXECUTE FUNCTION delay_old_repair();")
        .execute(db.pool()).await.unwrap();
    let settings = NativeMaintenanceConfig {
        partition_lookahead_days: 1,
        max_partition_operations_per_cycle: 1,
        // Make only the deliberately blocked day exceed the normal one-second
        // operation budget. A 150-ms budget also deferred every unrelated DDL
        // operation on the constrained fixture, hiding the fairness assertion.
        operation_timeout_milliseconds: 1000,
        ..config()
    };
    let mut deadlines = 0;
    let mut turns = Vec::new();
    // Repeated maintenance may also defer unrelated DDL on the constrained
    // server. Bound convergence without extending any operation deadline.
    for _ in 0..36 {
        let result = PartitionRepository::reconcile(db.pool(), &settings, now)
            .await
            .unwrap();
        assert_eq!(result.attempted, 1);
        deadlines += result.deferred_deadline;
        let cursors: Vec<(ManagedTable, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT parent,last_day FROM native_partition_reconcile_cursor ORDER BY parent",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        turns.push((result, cursors));
        let covered: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM native_partition_registry WHERE lower_bound >= $1 AND upper_bound <= $2",
        ).bind(now).bind(now + Duration::days(2)).fetch_one(db.pool()).await.unwrap();
        if covered == 6 {
            break;
        }
    }
    assert!(deadlines > 0);
    for row in PartitionRepository::status(db.pool(), &settings, now)
        .await
        .unwrap()
    {
        assert_eq!(
            row.missing_future_partitions, 0,
            "status={row:?}, turns={turns:?}"
        );
    }
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        1
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn reconcile_failure_keeps_committed_creation_and_moved_rows_and_rejects_drift() {
    let db = database().await;
    let now = utc_day(Utc::now()) + Duration::days(50);
    events(db.pool(), now, 2).await;
    let incompatible = format!("execution_history_p{}", now.format("%Y%m%d"));
    sqlx::query(&format!(
        "CREATE TABLE {incompatible}(LIKE execution_history INCLUDING ALL)"
    ))
    .execute(db.pool())
    .await
    .unwrap();
    let failure = PartitionRepository::reconcile(db.pool(), &config(), now)
        .await
        .unwrap_err();
    assert_eq!(
        (
            failure.partial.attempted,
            failure.partial.created,
            failure.partial.rows_moved
        ),
        (1, 1, 2)
    );
    assert!(
        matches!(failure.source, attune_common::Error::Database(sqlx::Error::Database(ref error)) if error.code().as_deref() == Some("55000"))
    );
    assert_eq!(count(db.pool(), "SELECT count(*) FROM event").await, 2);
    assert_eq!(
        count(db.pool(), "SELECT count(*) FROM ONLY event_default").await,
        0
    );
    let invalid = NativeMaintenanceConfig {
        max_partition_operations_per_cycle: 0,
        ..config()
    };
    let validation = PartitionRepository::reconcile(db.pool(), &invalid, now)
        .await
        .unwrap_err();
    assert_eq!(
        (
            validation.partial.attempted,
            validation.partial.created,
            validation.partial.rows_moved
        ),
        (0, 0, 0)
    );
    db.cleanup().await.unwrap();
}
