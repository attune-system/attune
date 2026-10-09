//! Database contracts for bounded retention. Every test owns a physical clone.

use super::*;
use crate::{config::Config, test_database::TestDatabase};

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

fn config(batch_size: i64, max_batches_per_target: i64) -> RetentionConfig {
    RetentionConfig {
        batch_size,
        max_batches_per_target,
        // These tests exercise row-batch accounting. Native DROP accounting has
        // its own tests with historical registered leaves.
        native_maintenance: crate::config::NativeMaintenanceConfig {
            enabled: false,
            ..Default::default()
        },
        ..RetentionConfig::default()
    }
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*)::BIGINT FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn seed_history(pool: &PgPool, table: &str, time: DateTime<Utc>, rows: i64) {
    sqlx::query(&format!(
        "INSERT INTO {table} (time, operation, entity_id, entity_ref, new_values)
         SELECT $1, 'UPDATE', 42, 'retention.duplicate', '{{\"status\":\"failed\"}}'::jsonb
         FROM generate_series(1, $2::BIGINT)"
    ))
    .bind(time)
    .bind(rows)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn native_boundary_retention_keeps_exact_cutoff_and_reports_drop_separately() {
    let db = database().await;
    let day = DateTime::parse_from_rfc3339("2020-09-02T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let cutoff = day + Duration::hours(12);
    let native = crate::config::NativeMaintenanceConfig {
        operation_timeout_milliseconds: 10_000,
        max_partition_cycle_milliseconds: 20_000,
        ..Default::default()
    };
    for start in [day - Duration::days(1), day] {
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, start, &native)
            .await
            .unwrap();
    }
    for time in [
        day - Duration::days(1),
        day - Duration::days(3),
        cutoff - Duration::microseconds(1),
        cutoff,
        cutoff + Duration::microseconds(1),
    ] {
        sqlx::query("INSERT INTO event(created,trigger_ref) VALUES($1,'boundary.fixture')")
            .bind(time)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let settings = RetentionConfig {
        batch_size: 10,
        max_batches_per_target: 1,
        native_maintenance: native,
        ..Default::default()
    };
    let result = RetentionRepository::run_target_before(
        db.pool(),
        RetentionTarget::Events,
        cutoff,
        &settings,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            result.partitions_dropped,
            result.partition_candidates,
            result.candidates,
            result.deleted
        ),
        (1, 1, 2, 2)
    );
    assert!(result.candidates_exact);
    assert_eq!(count(db.pool(), "event").await, 2);
    let minimum: DateTime<Utc> = sqlx::query_scalar("SELECT min(created) FROM event")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(minimum, cutoff);
    assert_eq!(count(db.pool(), "event_default").await, 0);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_row_failure_retains_confirmed_partition_drops_and_bounded_candidate_metadata() {
    let db = database().await;
    let day = DateTime::parse_from_rfc3339("2020-10-02T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let native = crate::config::NativeMaintenanceConfig {
        operation_timeout_milliseconds: 10_000,
        max_partition_cycle_milliseconds: 20_000,
        ..Default::default()
    };
    PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, day, &native)
        .await
        .unwrap();
    sqlx::query("INSERT INTO event(created,trigger_ref) SELECT $1,'failure.fixture' FROM generate_series(1,100)").bind(day).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO event(created,trigger_ref) SELECT $1,'failure.fixture' FROM generate_series(1,4)").bind(day+Duration::days(2)).execute(db.pool()).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION fixture_fail_default() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned default failure'; END $$; CREATE TRIGGER fixture_fail_default BEFORE DELETE ON event_default FOR EACH ROW EXECUTE FUNCTION fixture_fail_default();")
        .execute(db.pool()).await.unwrap();
    let settings = RetentionConfig {
        batch_size: 2,
        max_batches_per_target: 1,
        native_maintenance: native,
        ..Default::default()
    };
    let failure = RetentionRepository::run_target_before(
        db.pool(),
        RetentionTarget::Events,
        day + Duration::days(3),
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
        (1, 1, 0, Some(3))
    );
    assert!(!failure.candidates_exact);
    assert!(failure.source.to_string().contains("owned default failure"));
    assert_eq!(count(db.pool(), "event").await, 4);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn duplicate_history_rows_drain_in_committed_batches_with_strict_cutoffs() {
    let db = database().await;
    let cutoff = Utc::now() - Duration::days(1);
    for target in [
        RetentionTarget::ExecutionHistory,
        RetentionTarget::WorkerHistory,
        RetentionTarget::SensorProcessHistory,
    ] {
        let (table, _, _) = RetentionRepository::target_sql(target);
        seed_history(db.pool(), table, cutoff - Duration::microseconds(1), 7).await;
        seed_history(db.pool(), table, cutoff, 1).await;
        seed_history(db.pool(), table, cutoff + Duration::microseconds(1), 1).await;

        let dry_run = RetentionConfig {
            dry_run: true,
            ..config(2, 2)
        };
        let result =
            RetentionRepository::run_target_before(db.pool(), target, cutoff, &dry_run, || false)
                .await
                .unwrap();
        assert_eq!((result.candidates, result.deleted), (7, 0));
        assert_eq!(count(db.pool(), table).await, 9);

        let result = RetentionRepository::run_target_before(
            db.pool(),
            target,
            cutoff,
            &config(2, 2),
            || false,
        )
        .await
        .unwrap();
        assert_eq!(result.cutoff, Some(cutoff));
        assert_eq!((result.candidates, result.deleted), (7, 4));
        // A new pool connection sees the committed deletes after budget exhaustion.
        assert_eq!(count(db.pool(), table).await, 5);

        let result = RetentionRepository::run_target_before(
            db.pool(),
            target,
            cutoff,
            &config(2, 100),
            || false,
        )
        .await
        .unwrap();
        assert_eq!((result.candidates, result.deleted), (3, 3));
        assert_eq!(count(db.pool(), table).await, 2);
    }
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn locked_history_stops_at_zero_progress_and_can_retry_next_cycle() {
    let db = database().await;
    let cutoff = Utc::now() - Duration::days(1);
    seed_history(
        db.pool(),
        "execution_history",
        cutoff - Duration::days(1),
        3,
    )
    .await;
    let mut lock = db.pool().begin().await.unwrap();
    sqlx::query("SELECT ctid FROM execution_history FOR UPDATE")
        .fetch_all(&mut *lock)
        .await
        .unwrap();

    let mut checks = 0;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        RetentionRepository::run_target_before(
            db.pool(),
            RetentionTarget::ExecutionHistory,
            cutoff,
            &config(2, 100),
            || {
                checks += 1;
                false
            },
        ),
    )
    .await
    .expect("retention must skip locked rows without waiting")
    .unwrap();
    assert_eq!((result.candidates, result.deleted), (3, 0));
    assert_eq!(
        checks, 4,
        "one initial check, one batch and two summary-kind checks, with no raw retry spin"
    );
    assert_eq!(count(db.pool(), "execution_history").await, 3);
    lock.rollback().await.unwrap();

    let result = RetentionRepository::run_target_before(
        db.pool(),
        RetentionTarget::ExecutionHistory,
        cutoff,
        &config(2, 100),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(result.deleted, 3);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancellation_after_one_batch_preserves_committed_progress() {
    let db = database().await;
    let cutoff = Utc::now() - Duration::days(1);
    seed_history(db.pool(), "worker_history", cutoff - Duration::days(1), 5).await;
    let mut checks = 0;
    let result = RetentionRepository::run_target_before(
        db.pool(),
        RetentionTarget::WorkerHistory,
        cutoff,
        &config(2, 100),
        || {
            checks += 1;
            checks == 3
        },
    )
    .await
    .unwrap();
    assert_eq!((result.candidates, result.deleted), (5, 2));
    assert_eq!(checks, 3);
    assert_eq!(count(db.pool(), "worker_history").await, 3);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn later_batch_failure_preserves_confirmed_progress_and_the_source_error() {
    let db = database().await;
    let cutoff = Utc::now() - Duration::days(1);
    sqlx::query(
        "INSERT INTO worker_history (time, operation, entity_id, entity_ref)
         SELECT $1 + n * INTERVAL '1 second', 'UPDATE', n, 'retention.failure'
         FROM generate_series(1, 5) n",
    )
    .bind(cutoff - Duration::days(1))
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION retention_reject_later_batch() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             IF OLD.entity_id = 4 THEN
                 RAISE EXCEPTION 'forced second retention batch failure';
             END IF;
             RETURN OLD;
         END;
         $$;
         CREATE TRIGGER retention_reject_later_batch
         BEFORE DELETE ON worker_history FOR EACH ROW
         EXECUTE FUNCTION retention_reject_later_batch();",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let failure = RetentionRepository::run_target_before(
        db.pool(),
        RetentionTarget::WorkerHistory,
        cutoff,
        &config(2, 100),
        || false,
    )
    .await
    .unwrap_err();
    let remaining: Vec<i64> =
        sqlx::query_scalar("SELECT entity_id FROM worker_history ORDER BY entity_id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    db.cleanup().await.unwrap();

    assert_eq!(failure.target, RetentionTarget::WorkerHistory);
    assert_eq!(failure.cutoff, cutoff);
    assert_eq!(failure.candidates, Some(5));
    assert_eq!(failure.deleted, 2);
    assert!(!failure.dry_run);
    assert_eq!(remaining, vec![3, 4, 5], "the failing batch must roll back");
    assert!(matches!(
        failure.source,
        crate::Error::Database(sqlx::Error::Database(ref error))
            if error.code().as_deref() == Some("P0001")
                && error.message() == "forced second retention batch failure"
    ));
    assert!(std::error::Error::source(&failure).is_some());
}

#[tokio::test]
async fn event_and_audit_counts_are_rows_and_every_batch_respects_the_limit() {
    let db = database().await;
    let cutoff = Utc::now() - Duration::days(1);
    sqlx::query(
        "INSERT INTO event (trigger_ref, created)
         SELECT 'retention.event', $1 FROM generate_series(1, 7)",
    )
    .bind(cutoff - Duration::days(1))
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO audit_event (category, event_type, outcome, created)
         SELECT 'api', 'retention.test', 'success', $1 FROM generate_series(1, 7)",
    )
    .bind(cutoff - Duration::days(1))
    .execute(db.pool())
    .await
    .unwrap();

    for target in [RetentionTarget::Events, RetentionTarget::AuditEvents] {
        let dry_run = RetentionConfig {
            dry_run: true,
            ..config(2, 2)
        };
        let result =
            RetentionRepository::run_target_before(db.pool(), target, cutoff, &dry_run, || false)
                .await
                .unwrap();
        assert_eq!((result.candidates, result.deleted), (7, 0));
        let result = RetentionRepository::run_target_before(
            db.pool(),
            target,
            cutoff,
            &config(2, 2),
            || false,
        )
        .await
        .unwrap();
        assert_eq!((result.candidates, result.deleted), (7, 4));
        assert_eq!(
            RetentionRepository::count_target_candidates(db.pool(), target, cutoff)
                .await
                .unwrap(),
            3,
        );
    }
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn batch_budget_and_unlimited_targets_round_trip_through_persistence() {
    let db = database().await;
    let defaults = RetentionRepository::load_config(db.pool()).await.unwrap();
    assert_eq!(defaults.max_batches_per_target, 100);
    let mut updated = config(3, 7);
    updated.targets.events = RetentionTargetConfig::keep_forever();
    let stored = RetentionRepository::update_config(db.pool(), &updated)
        .await
        .unwrap();
    assert_eq!(stored, updated);
    assert_eq!(
        RetentionRepository::load_config(db.pool()).await.unwrap(),
        updated
    );
    assert!(RetentionRepository::configured_targets(&stored.targets)
        .iter()
        .any(|target| {
            target.target == RetentionTarget::Events && target.max_age_seconds.is_none()
        }));
    for budget in [0_i64, -1] {
        assert!(
            sqlx::query("UPDATE runtime_retention_config SET max_batches_per_target = $1")
                .bind(budget)
                .execute(db.pool())
                .await
                .is_err()
        );
    }
    assert_eq!(
        RetentionRepository::load_config(db.pool()).await.unwrap(),
        updated
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM runtime_retention_target_config"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        16
    );
    db.cleanup().await.unwrap();
}

async fn execution(pool: &PgPool, status: &str, updated: DateTime<Utc>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO execution (action_ref, status, config, created, updated)
         VALUES ('retention.action', $1::execution_status_enum, '{}'::jsonb, $2, $2)
         RETURNING id",
    )
    .bind(status)
    .bind(updated)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn workflow(pool: &PgPool, definition: i64, execution: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflow_execution (execution, workflow_def, task_graph)
         VALUES ($1, $2, '{}'::jsonb) RETURNING id",
    )
    .bind(execution)
    .bind(definition)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn bounded_cleanup_preserves_waits_pending_logs_and_nonterminal_queue_rows() {
    let db = database().await;
    let pool = db.pool();
    let cutoff = Utc::now() - Duration::days(1);
    let old = cutoff - Duration::days(1);
    let pack: i64 = sqlx::query_scalar(
        "INSERT INTO pack (ref, label, version)
         VALUES ('retention', 'Retention', '1.0.0') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let definition: i64 = sqlx::query_scalar(
        "INSERT INTO workflow_definition (ref, pack, pack_ref, label, version, definition)
         VALUES ('retention.workflow', $1, 'retention', 'Retention', '1.0.0', '{}'::jsonb)
         RETURNING id",
    )
    .bind(pack)
    .fetch_one(pool)
    .await
    .unwrap();
    let running = execution(pool, "running", old).await;
    let scope = workflow(pool, definition, running).await;
    let waited = execution(pool, "completed", old).await;
    let pending_log = execution(pool, "failed", old).await;
    let delivered_log = execution(pool, "completed", old).await;
    let recent = execution(pool, "completed", cutoff).await;
    for status in ["completed", "cancelled", "timeout"] {
        execution(pool, status, old).await;
    }
    sqlx::query(
        "INSERT INTO workflow_task_wait (workflow_execution, task_name, kind, target_execution)
         VALUES ($1, 'execution_wait', 'execution', $2)",
    )
    .bind(scope)
    .bind(waited)
    .execute(pool)
    .await
    .unwrap();
    let pending_workflow = workflow(pool, definition, pending_log).await;
    let delivered_workflow = workflow(pool, definition, delivered_log).await;
    sqlx::query(
        "INSERT INTO workflow_log_outbox (workflow_execution, sequence, kind, delivered_at)
         VALUES ($1, 1, 'seal', NULL), ($2, 1, 'seal', NOW())",
    )
    .bind(pending_workflow)
    .bind(delivered_workflow)
    .execute(pool)
    .await
    .unwrap();

    let queue: i64 = sqlx::query_scalar(
        "INSERT INTO work_queue (ref, label, dispatch_action_ref)
         VALUES ('retention.queue', 'Retention', 'retention.action') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let mut protected_items = Vec::new();
    for status in [
        "queued",
        "leased",
        "completed",
        "failed",
        "skipped",
        "cancelled",
    ] {
        let item: i64 = sqlx::query_scalar(
            "INSERT INTO work_queue_item (queue, queue_ref, status, payload, enqueue_source, updated)
             VALUES ($1, 'retention.queue', $2::work_queue_item_status_enum, '{}', 'test', $3)
             RETURNING id",
        )
        .bind(queue)
        .bind(status)
        .bind(old)
        .fetch_one(pool)
        .await
        .unwrap();
        if ["queued", "leased", "completed"].contains(&status) {
            protected_items.push(item);
        }
        if status == "completed" {
            sqlx::query(
                "INSERT INTO workflow_task_wait (
                    workflow_execution, task_name, kind, work_queue_item, active_work_queue_item
                 ) VALUES ($1, 'queue_wait', 'work_queue_item', $2, $2)",
            )
            .bind(scope)
            .bind(item)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    let dry_run = RetentionConfig {
        dry_run: true,
        ..config(2, 2)
    };
    let result = RetentionRepository::run_target_before(
        pool,
        RetentionTarget::Executions,
        cutoff,
        &dry_run,
        || false,
    )
    .await
    .unwrap();
    assert_eq!((result.candidates, result.deleted), (4, 0));
    assert_eq!(count(pool, "workflow_log_outbox").await, 2);

    let result = RetentionRepository::run_target_before(
        pool,
        RetentionTarget::Executions,
        cutoff,
        &config(2, 2),
        || false,
    )
    .await
    .unwrap();
    assert_eq!((result.candidates, result.deleted), (4, 4));
    let remaining: Vec<i64> = sqlx::query_scalar("SELECT id FROM execution ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(remaining, vec![running, waited, pending_log, recent]);
    let outbox_scopes: Vec<i64> =
        sqlx::query_scalar("SELECT workflow_execution FROM workflow_log_outbox ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(outbox_scopes, vec![pending_workflow]);

    let result = RetentionRepository::run_target_before(
        pool,
        RetentionTarget::WorkQueueItems,
        cutoff,
        &config(2, 2),
        || false,
    )
    .await
    .unwrap();
    assert_eq!((result.candidates, result.deleted), (3, 3));
    let remaining: Vec<i64> = sqlx::query_scalar("SELECT id FROM work_queue_item ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(remaining, protected_items);
    assert_eq!(count(pool, "workflow_task_wait").await, 2);
    db.cleanup().await.unwrap();
}
