//! Transaction-local invalidation coalescing through the actual source triggers.
//! Each test owns a physical clone and awaits every writer before teardown.

use attune_common::{
    config::{Config, NativeMaintenanceConfig},
    repositories::native_maintenance::{
        partitions::{PartitionRepairOutcome, PartitionRepository},
        summaries::SummaryRepository,
        ManagedTable, SummaryKind,
    },
    test_database::TestDatabase,
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Postgres, Transaction};

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

fn bucket() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2021-10-02T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn config() -> NativeMaintenanceConfig {
    NativeMaintenanceConfig {
        operation_timeout_milliseconds: 10_000,
        max_partition_cycle_milliseconds: 20_000,
        ..Default::default()
    }
}

async fn event(tx: &mut Transaction<'_, Postgres>, time: DateTime<Utc>) {
    sqlx::query("INSERT INTO event(created,trigger_ref) VALUES($1,'producer.fixture')")
        .bind(time)
        .execute(&mut **tx)
        .await
        .unwrap();
}

async fn pending(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar("SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1 ORDER BY id")
        .bind(bucket()).fetch_all(pool).await.unwrap()
}

async fn refresh(pool: &PgPool) -> u64 {
    SummaryRepository::refresh_bucket_at(
        pool,
        SummaryKind::EventVolume,
        bucket(),
        &config(),
        bucket() + Duration::hours(1),
    )
    .await
    .unwrap()
    .notifications_processed
}

#[tokio::test]
async fn real_execution_row_history_triggers_coalesce_across_statements_in_one_transaction() {
    let db = database().await;
    let mut tx = db.pool().begin().await.unwrap();
    let (start, was_called): (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM native_summary_invalidation_id_seq")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO execution(action_ref) SELECT 'producer.fixture' FROM generate_series(1,32)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE execution SET status='running' WHERE action_ref='producer.fixture'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE execution SET status='completed' WHERE action_ref='producer.fixture'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let records: Vec<(String, i64, bool)> = sqlx::query_as(
        "SELECT kind::text,count(*),bool_and(transaction_origin=pg_current_xact_id())
         FROM native_summary_invalidation GROUP BY kind ORDER BY native_summary_invalidation.kind",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        records,
        vec![
            ("execution_status".into(), 1, true),
            ("execution_creation".into(), 1, true)
        ]
    );
    let history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM execution_history WHERE entity_ref='producer.fixture'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(history, 96, "source history records must not be coalesced");
    let end: i64 = sqlx::query_scalar("SELECT last_value FROM native_summary_invalidation_id_seq")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        end - start + i64::from(!was_called),
        2,
        "filtered duplicates must not evaluate the sequence default"
    );
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM native_summary_invalidation")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        visible, 0,
        "coalesced records are invisible until the entire producer commits"
    );
    tx.commit().await.unwrap();
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM native_summary_invalidation")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(records, 2);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn different_origins_do_not_block_and_lower_id_late_commit_survives_exact_ack() {
    let db = database().await;
    let mut late = db.pool().begin().await.unwrap();
    event(&mut late, bucket()).await;
    event(&mut late, bucket()).await;
    let (low,origin):(i64,String)=sqlx::query_as("SELECT id,transaction_origin::text FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1")
        .bind(bucket()).fetch_one(&mut *late).await.unwrap();
    let mut early = db.pool().begin().await.unwrap();
    sqlx::query("SET LOCAL statement_timeout='1s'")
        .execute(&mut *early)
        .await
        .unwrap();
    event(&mut early, bucket()).await;
    event(&mut early, bucket()).await;
    let (high,early_origin):(i64,String)=sqlx::query_as("SELECT id,transaction_origin::text FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1 AND transaction_origin=pg_current_xact_id()")
        .bind(bucket()).fetch_one(&mut *early).await.unwrap();
    assert!(high > low);
    assert_ne!(origin, early_origin);
    early.commit().await.unwrap();
    assert_eq!(pending(db.pool()).await, vec![high]);
    assert_eq!(refresh(db.pool()).await, 1);
    let materialized: i64 = sqlx::query_scalar(
        "SELECT sum(event_count)::bigint FROM event_volume_hourly_summary WHERE bucket=$1",
    )
    .bind(bucket())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(materialized, 2);
    // A later source change still uses this transaction's original low-ID marker.
    event(&mut late, bucket()).await;
    late.commit().await.unwrap();
    assert_eq!(pending(db.pool()).await, vec![low]);
    assert_eq!(refresh(db.pool()).await, 1);
    let materialized: i64 = sqlx::query_scalar(
        "SELECT sum(event_count)::bigint FROM event_volume_hourly_summary WHERE bucket=$1",
    )
    .bind(bucket())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(materialized, 5);
    assert!(pending(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn rollback_and_savepoint_keep_top_level_origin_without_losing_later_changes() {
    let db = database().await;
    let mut rolled_back = db.pool().begin().await.unwrap();
    event(&mut rolled_back, bucket()).await;
    event(&mut rolled_back, bucket()).await;
    rolled_back.rollback().await.unwrap();
    assert!(pending(db.pool()).await.is_empty());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM event")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let mut tx = db.pool().begin().await.unwrap();
    event(&mut tx, bucket()).await;
    let origin: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SAVEPOINT nested_writer")
        .execute(&mut *tx)
        .await
        .unwrap();
    event(&mut tx, bucket()).await;
    event(&mut tx, bucket() + Duration::hours(1)).await;
    let nested: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(nested, origin);
    sqlx::query("ROLLBACK TO SAVEPOINT nested_writer")
        .execute(&mut *tx)
        .await
        .unwrap();
    event(&mut tx, bucket()).await;
    event(&mut tx, bucket() + Duration::hours(1)).await;
    tx.commit().await.unwrap();
    let groups:Vec<(DateTime<Utc>,i64,String)>=sqlx::query_as("SELECT bucket,count(*),min(transaction_origin::text) FROM native_summary_invalidation GROUP BY bucket ORDER BY bucket")
        .fetch_all(db.pool()).await.unwrap();
    assert_eq!(
        groups,
        vec![
            (bucket(), 1, origin.clone()),
            (bucket() + Duration::hours(1), 1, origin)
        ]
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn corrections_deduplicate_old_and_new_hours_for_both_history_kinds() {
    let db = database().await;
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO execution_history(time,operation,entity_id,changed_fields,new_values) VALUES($1,'INSERT',98765,ARRAY['status'],'{}')")
        .bind(bucket()).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "UPDATE execution_history SET time=$1,entity_ref='corrected' WHERE entity_id=98765",
    )
    .bind(bucket() + Duration::hours(1))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE execution_history SET entity_ref=NULL,new_values='{}' WHERE entity_id=98765",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("DELETE FROM execution_history WHERE entity_id=98765")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let groups:Vec<(String,DateTime<Utc>,i64)>=sqlx::query_as("SELECT kind::text,bucket,count(*) FROM native_summary_invalidation GROUP BY kind,bucket ORDER BY native_summary_invalidation.kind,bucket")
        .fetch_all(db.pool()).await.unwrap();
    assert_eq!(
        groups,
        vec![
            ("execution_status".into(), bucket(), 1),
            ("execution_status".into(), bucket() + Duration::hours(1), 1),
            ("execution_creation".into(), bucket(), 1),
            (
                "execution_creation".into(),
                bucket() + Duration::hours(1),
                1
            )
        ]
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn physical_move_coalesces_with_parent_write_and_expiry_removes_origins_atomically() {
    let db = database().await;
    let mut tx = db.pool().begin().await.unwrap();
    event(&mut tx, bucket()).await;
    event(&mut tx, bucket()).await;
    let outcome: String =
        sqlx::query_scalar("SELECT outcome FROM native_partition_ensure_day('event',$1,1000)")
            .bind(bucket())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(outcome, "applied");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM native_summary_invalidation")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        rows, 1,
        "physical maintenance and parent producer share the transaction origin"
    );
    tx.rollback().await.unwrap();
    assert!(pending(db.pool()).await.is_empty());
    assert_eq!(
        PartitionRepository::ensure_day(db.pool(), ManagedTable::Event, bucket(), &config())
            .await
            .unwrap(),
        PartitionRepairOutcome::Applied { rows_moved: 0 }
    );
    let mut tx = db.pool().begin().await.unwrap();
    event(&mut tx, bucket()).await;
    event(&mut tx, bucket()).await;
    tx.commit().await.unwrap();
    assert_eq!(pending(db.pool()).await.len(), 1);
    let expired = PartitionRepository::expire_before(
        db.pool(),
        ManagedTable::Event,
        bucket() + Duration::days(1),
        &config(),
        false,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(expired.partitions_dropped, 1);
    assert!(pending(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn restored_origin_with_foreign_xmin_cannot_suppress_producer_after_builder_snapshot() {
    let db = database().await;
    let mut producer = db.pool().begin().await.unwrap();
    let (origin, actual_xmin): (String, String) =
        sqlx::query_as("SELECT pg_current_xact_id()::text,pg_current_xact_id()::xid::text")
            .fetch_one(&mut *producer)
            .await
            .unwrap();
    // Logical import copies the user column, but PostgreSQL assigns the real
    // tuple xmin to the importing transaction. Never trust origin alone.
    let (imported_id,imported_xmin):(i64,String)=sqlx::query_as("INSERT INTO native_summary_invalidation(kind,bucket,transaction_origin) VALUES('event_volume',$1,$2::text::xid8) RETURNING id,xmin::text")
        .bind(bucket()).bind(&origin).fetch_one(db.pool()).await.unwrap();
    assert_ne!(imported_xmin, actual_xmin);
    let mut builder = db.pool().begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *builder)
        .await
        .unwrap();
    sqlx::query("LOCK TABLE ONLY event IN ACCESS SHARE MODE")
        .execute(&mut *builder)
        .await
        .unwrap();
    sqlx::query("SELECT kind FROM native_summary_state WHERE kind='event_volume' FOR UPDATE")
        .execute(&mut *builder)
        .await
        .unwrap();
    let captured:Vec<i64>=sqlx::query_scalar("SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1 ORDER BY id")
        .bind(bucket()).fetch_all(&mut *builder).await.unwrap();
    assert_eq!(captured, vec![imported_id]);
    let source_snapshot: i64 = sqlx::query_scalar("SELECT count(*) FROM event")
        .fetch_one(&mut *builder)
        .await
        .unwrap();
    assert_eq!(source_snapshot, 0);
    event(&mut producer, bucket()).await;
    event(&mut producer, bucket()).await;
    let own_ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket=$1 AND xmin=pg_current_xact_id()::xid")
        .bind(bucket()).fetch_all(&mut *producer).await.unwrap();
    assert_eq!(own_ids.len(), 1);
    assert_ne!(own_ids[0], imported_id);
    producer.commit().await.unwrap();
    sqlx::query("DELETE FROM native_summary_invalidation WHERE id=ANY($1::bigint[])")
        .bind(&captured)
        .execute(&mut *builder)
        .await
        .unwrap();
    builder.commit().await.unwrap();
    assert_eq!(pending(db.pool()).await, own_ids);
    assert_eq!(refresh(db.pool()).await, 1);
    let materialized: i64 = sqlx::query_scalar(
        "SELECT sum(event_count)::bigint FROM event_volume_hourly_summary WHERE bucket=$1",
    )
    .bind(bucket())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(materialized, 2);
    assert!(pending(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn outer_marker_is_reused_across_multiple_savepoint_commands_and_release() {
    let db = database().await;
    let mut tx = db.pool().begin().await.unwrap();
    event(&mut tx, bucket()).await;
    let initial: i64 =
        sqlx::query_scalar("SELECT last_value FROM native_summary_invalidation_id_seq")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    sqlx::query("SAVEPOINT child")
        .execute(&mut *tx)
        .await
        .unwrap();
    for _ in 0..3 {
        event(&mut tx, bucket()).await;
    }
    sqlx::query("RELEASE SAVEPOINT child")
        .execute(&mut *tx)
        .await
        .unwrap();
    event(&mut tx, bucket()).await;
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM native_summary_invalidation")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let final_sequence: i64 =
        sqlx::query_scalar("SELECT last_value FROM native_summary_invalidation_id_seq")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(records, 1);
    assert_eq!(final_sequence, initial);
    tx.commit().await.unwrap();
    assert_eq!(refresh(db.pool()).await, 1);
    let source: i64 = sqlx::query_scalar("SELECT count(*) FROM event")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(source, 5);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn child_created_markers_are_conservatively_retained_without_hiding_source_changes() {
    let db = database().await;
    let mut tx = db.pool().begin().await.unwrap();
    let top: String = sqlx::query_scalar("SELECT pg_current_xact_id()::xid::text")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SAVEPOINT child")
        .execute(&mut *tx)
        .await
        .unwrap();
    event(&mut tx, bucket()).await;
    event(&mut tx, bucket()).await;
    let child_xmins: Vec<String> =
        sqlx::query_scalar("SELECT xmin::text FROM native_summary_invalidation ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        child_xmins.len(),
        2,
        "a child tuple is not mistaken for an outer-owned tuple"
    );
    assert!(child_xmins.iter().all(|xmin| xmin != &top));
    sqlx::query("RELEASE SAVEPOINT child")
        .execute(&mut *tx)
        .await
        .unwrap();
    event(&mut tx, bucket()).await;
    event(&mut tx, bucket()).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM native_summary_invalidation")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        count, 3,
        "the first outer write creates a reusable outer-owned marker"
    );
    tx.commit().await.unwrap();
    assert_eq!(refresh(db.pool()).await, 3);
    let materialized: i64 = sqlx::query_scalar(
        "SELECT sum(event_count)::bigint FROM event_volume_hourly_summary WHERE bucket=$1",
    )
    .bind(bucket())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(materialized, 4);
    assert!(pending(db.pool()).await.is_empty());
    db.cleanup().await.unwrap();
}
