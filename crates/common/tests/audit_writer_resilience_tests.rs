mod helpers;
use attune_common::audit::{
    writer::spawn_writer_with, AuditCategory, AuditEventBuilder, AuditOutcome,
};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn audit_flush_and_shutdown_confirm_persistence() {
    let db = helpers::create_test_pool().await.unwrap();
    let handle = spawn_writer_with((*db).clone(), 2, 10);
    let stats = handle.emitter.stats.clone();
    for _ in 0..3 {
        handle.emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "probe.persist", AuditOutcome::Success)
                .build(),
        );
    }
    assert!(handle.emitter.flush().await);
    handle.shutdown().await;
    assert_eq!(stats.persisted.load(Ordering::Relaxed), 3);
    assert_eq!(stats.lost.load(Ordering::Relaxed), 0);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE event_type='probe.persist'")
            .fetch_one(&*db)
            .await
            .unwrap();
    assert_eq!(count, 3);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn audit_definitive_failure_is_not_successful_flush() {
    let db = helpers::create_test_pool().await.unwrap();
    // An owned table-local trigger rejects only this fixture's writes. No global
    // database state or live service is modified.
    sqlx::raw_sql("CREATE FUNCTION reject_probe_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned rejection'; END $$; CREATE TRIGGER reject_probe BEFORE INSERT ON audit_event FOR EACH ROW EXECUTE FUNCTION reject_probe_audit();").execute(&*db).await.unwrap();
    let handle = spawn_writer_with((*db).clone(), 2, 10);
    let stats = handle.emitter.stats.clone();
    handle.emitter.emit(
        AuditEventBuilder::new(AuditCategory::Api, "probe.failure", AuditOutcome::Failure).build(),
    );
    assert!(!handle.emitter.flush().await);
    handle.shutdown().await;
    assert_eq!(stats.persisted.load(Ordering::Relaxed), 0);
    assert_eq!(stats.lost.load(Ordering::Relaxed), 1);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn ambiguous_insert_retry_reuses_partitioned_identity() {
    let db = helpers::create_test_pool().await.unwrap();
    sqlx::raw_sql("CREATE SEQUENCE probe_audit_attempt; CREATE FUNCTION gate_first_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF nextval('probe_audit_attempt')=1 THEN PERFORM pg_advisory_xact_lock(918273647); END IF; RETURN NEW; END $$; CREATE TRIGGER gate_probe BEFORE INSERT ON audit_event FOR EACH ROW EXECUTE FUNCTION gate_first_audit();").execute(&*db).await.unwrap();
    let mut blocker = db.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(918273647)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let handle = spawn_writer_with((*db).clone(), 1, 10);
    handle.emitter.emit(
        AuditEventBuilder::new(AuditCategory::Api, "probe.ambiguous", AuditOutcome::Success)
            .build(),
    );
    // First insertion is server-owned and unacknowledged. Wait for the bounded
    // retry to reach the real trigger on a second session, then release it.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let attempt: i64 = sqlx::query_scalar("SELECT last_value FROM probe_audit_attempt")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            if attempt >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    sqlx::query("SELECT pg_advisory_unlock(918273647)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    assert!(handle.emitter.flush().await);
    handle.shutdown().await;
    drop(blocker);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_event WHERE event_type='probe.ambiguous'")
            .fetch_one(&*db)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "ambiguous retry must not create a second audit identity"
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn shutdown_drains_accepted_events_with_live_emitter_clones() {
    let db = helpers::create_test_pool().await.unwrap();
    // Explicit tuning cannot bypass the bounded batch allocation.
    let handle = spawn_writer_with((*db).clone(), usize::MAX, 60_000);
    let emitter = handle.emitter.clone();
    let stats = emitter.stats.clone();
    for _ in 0..17 {
        emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "probe.shutdown", AuditOutcome::Success)
                .build(),
        );
    }
    handle.shutdown().await;
    assert_eq!(stats.persisted.load(Ordering::Relaxed), 17);
    assert_eq!(stats.lost.load(Ordering::Relaxed), 0);
    emitter.emit(
        AuditEventBuilder::new(AuditCategory::Api, "probe.closed", AuditOutcome::Success).build(),
    );
    assert_eq!(stats.emitted.load(Ordering::Relaxed), 18);
    assert_eq!(stats.lost.load(Ordering::Relaxed), 1);
    assert!(!emitter.flush().await);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn abort_accounts_buffer_and_queued_events_exactly_once() {
    let db = helpers::create_test_pool().await.unwrap();
    let handle = spawn_writer_with((*db).clone(), 256, 60_000);
    let stats = handle.emitter.stats.clone();
    // The task has not been polled on this current-thread runtime, so every
    // accepted event is still queued when cancellation takes ownership.
    for _ in 0..17 {
        handle.emitter.emit(
            AuditEventBuilder::new(AuditCategory::Api, "probe.abort", AuditOutcome::Success)
                .build(),
        );
    }
    handle.task.abort();
    assert!(handle.task.await.unwrap_err().is_cancelled());
    assert_eq!(stats.emitted.load(Ordering::Relaxed), 17);
    assert_eq!(stats.persisted.load(Ordering::Relaxed), 0);
    assert_eq!(stats.lost.load(Ordering::Relaxed), 17);
    db.cleanup().await.unwrap();
}
