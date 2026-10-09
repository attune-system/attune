//! Cleanup admission and whole-cycle deadline regressions on owned databases.

use attune_common::{
    config::{CacheAdmissionConfig, CacheRetentionConfig, Config},
    repositories::cache::{
        CacheEntryInput, CacheEntryRepository, CacheGenerationCleanupOutcome,
        CacheGenerationRepository, CacheIngestRepository, CacheNamespacePolicy,
        CacheNamespaceRepository, CacheOwnerScope, CacheTransactionMode,
        CreateCacheGenerationInput, CreateCacheGenerationResult, CreateCacheNamespaceInput,
    },
    test_database::TestDatabase,
};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::{Duration, Instant};

async fn fixture() -> (TestDatabase, i64) {
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))
    .unwrap();
    let db = TestDatabase::create(&config.database)
        .await
        .unwrap()
        .with_cleanup_on_drop();
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        db.pool(),
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::system(),
            namespace: "cleanup-budget".into(),
            policy: CacheNamespacePolicy::default(),
        },
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let generation = CacheGenerationRepository::create_or_get(
        db.pool(),
        &CreateCacheGenerationInput {
            namespace: namespace.id,
            client_refresh_id: "owned-cleanup".into(),
            expected_active_generation: None,
            expected_chunk_count: 1,
            expected_count: Some(1),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        },
    )
    .await
    .unwrap();
    let CreateCacheGenerationResult::Created(generation) = generation else {
        panic!("expected owned fresh generation")
    };
    CacheIngestRepository::insert_chunk(
        db.pool(),
        generation.id,
        0,
        "owned-checksum",
        &[CacheEntryInput {
            external_id: "owned-record".into(),
            value: json!({"owned": true}),
            source_updated_at: None,
            source_checksum: None,
        }],
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(db.pool(), generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::fail(db.pool(), generation.id, "owned fixture")
        .await
        .unwrap();
    (db, generation.id)
}

fn limits(cycle_ms: u64) -> CacheRetentionConfig {
    CacheRetentionConfig {
        max_cleanup_cycle_milliseconds: cycle_ms,
        min_traversal_window_seconds: 0,
        ..Default::default()
    }
}

async fn usage(pool: &PgPool, generation: i64) -> Option<(i64, i64)> {
    sqlx::query_as(
        "SELECT record_count, physical_bytes FROM cache_generation_entry_usage WHERE generation=$1",
    )
    .bind(generation)
    .fetch_optional(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn cleanup_queues_past_ddl_lock_budget_then_reclaims_after_admission() {
    let (db, generation) = fixture().await;
    let before = usage(db.pool(), generation).await.unwrap();
    let mut holder = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(7821101,0)")
        .execute(&mut *holder)
        .await
        .unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let task = tokio::spawn(async move {
        CacheGenerationRepository::drop_if_cleanup_eligible(&pool, generation, &limits(2_000)).await
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut queued_past_ddl_budget = false;
    while !task.is_finished() && Instant::now() < deadline {
        queued_past_ddl_budget = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE $1=ANY(pg_blocking_pids(a.pid)) \
             AND a.wait_event='advisory' AND clock_timestamp()-a.query_start >= INTERVAL '350 milliseconds')",
        )
        .bind(holder_pid)
        .fetch_one(db.pool())
        .await
        .unwrap();
        if queued_past_ddl_budget {
            break;
        }
        tokio::task::yield_now().await;
    }
    holder.rollback().await.unwrap();
    let outcome = task.await.unwrap().unwrap();
    let after = usage(db.pool(), generation).await;
    db.cleanup().await.unwrap();
    assert!(
        queued_past_ddl_budget,
        "admission consumed the 250ms DDL lock budget"
    );
    assert_eq!(
        outcome,
        CacheGenerationCleanupOutcome::Dropped {
            records: before.0 as u64,
            bytes: before.1 as u64,
        }
    );
    assert!(after.is_none());
}

#[tokio::test]
async fn cleanup_admission_exhausts_cycle_without_parent_locks_or_waiters() {
    let (db, generation) = fixture().await;
    let before = usage(db.pool(), generation).await;
    let mut holder = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(7821101,0)")
        .execute(&mut *holder)
        .await
        .unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let started = Instant::now();
    let task = tokio::spawn(async move {
        CacheGenerationRepository::drop_if_cleanup_eligible(&pool, generation, &limits(300)).await
    });
    let mut parent_locks = None;
    while !task.is_finished() && started.elapsed() < Duration::from_secs(2) {
        parent_locks = sqlx::query_scalar::<_, i64>(
            "SELECT (SELECT count(*) FROM pg_locks l WHERE l.pid=a.pid \
             AND l.relation='cache_entry'::regclass) FROM pg_stat_activity a \
             WHERE $1=ANY(pg_blocking_pids(a.pid)) AND a.wait_event='advisory' LIMIT 1",
        )
        .bind(holder_pid)
        .fetch_optional(db.pool())
        .await
        .unwrap();
        if parent_locks.is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    let outcome = task.await.unwrap().unwrap();
    let elapsed = started.elapsed();
    let waiters: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND classid=7821101 \
         AND objid=0 AND objsubid=2 AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) \
         AND NOT granted",
    ).fetch_one(db.pool()).await.unwrap();
    let after = usage(db.pool(), generation).await;
    holder.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    assert_eq!(parent_locks, Some(0));
    assert_eq!(outcome, CacheGenerationCleanupOutcome::DeferredDeadline);
    assert!(
        elapsed < Duration::from_secs(1),
        "admission exceeded its short test cycle: {elapsed:?}"
    );
    assert_eq!(waiters, 0);
    assert_eq!(after, before);
}

#[tokio::test]
async fn cleanup_parent_read_lock_still_uses_ddl_budget() {
    parent_ddl_control("ACCESS SHARE").await;
}

#[tokio::test]
async fn cleanup_parent_maintenance_lock_still_uses_ddl_budget() {
    parent_ddl_control("SHARE UPDATE EXCLUSIVE").await;
}

#[tokio::test]
async fn cleanup_parent_exclusive_lock_still_uses_ddl_budget() {
    parent_ddl_control("ACCESS EXCLUSIVE").await;
}

async fn parent_ddl_control(mode: &str) {
    let (db, generation) = fixture().await;
    let before = usage(db.pool(), generation).await;
    let mut reader = db.pool().begin().await.unwrap();
    sqlx::query(&format!("LOCK TABLE ONLY cache_entry IN {mode} MODE"))
        .execute(&mut *reader)
        .await
        .unwrap();
    let started = Instant::now();
    let outcome =
        CacheGenerationRepository::drop_if_cleanup_eligible(db.pool(), generation, &limits(2_000))
            .await
            .unwrap();
    let elapsed = started.elapsed();
    let after = usage(db.pool(), generation).await;
    reader.rollback().await.unwrap();
    let released =
        CacheGenerationRepository::drop_if_cleanup_eligible(db.pool(), generation, &limits(2_000))
            .await
            .unwrap();
    db.cleanup().await.unwrap();
    assert_eq!(outcome, CacheGenerationCleanupOutcome::DeferredBusy);
    assert!(elapsed < Duration::from_secs(1));
    assert_eq!(after, before);
    assert!(matches!(
        released,
        CacheGenerationCleanupOutcome::Dropped { records: 1, .. }
    ));
}

#[tokio::test]
async fn cancelled_cleanup_discards_connection_and_server_waiter_finishes() {
    let (db, generation) = fixture().await;
    let before = usage(db.pool(), generation).await;
    let mut holder = db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(7821101,0)")
        .execute(&mut *holder)
        .await
        .unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let pool = db.pool().clone();
    let task = tokio::spawn(async move {
        CacheGenerationRepository::drop_if_cleanup_eligible(&pool, generation, &limits(300)).await
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut waiter_pid = None;
    while !task.is_finished() && Instant::now() < deadline {
        waiter_pid = sqlx::query_scalar::<_, i32>(
            "SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) \
             AND wait_event='advisory' LIMIT 1",
        )
        .bind(holder_pid)
        .fetch_optional(db.pool())
        .await
        .unwrap();
        if waiter_pid.is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    task.abort();
    let cancelled = task.await;
    let mut backend_gone = false;
    if let Some(pid) = waiter_pid {
        while Instant::now() < deadline {
            backend_gone = sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)",
            )
            .bind(pid)
            .fetch_one(db.pool())
            .await
            .unwrap();
            if backend_gone {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
    let after = usage(db.pool(), generation).await;
    holder.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    assert!(waiter_pid.is_some(), "cleanup never reached admission");
    assert!(cancelled.unwrap_err().is_cancelled());
    assert!(
        backend_gone,
        "cancelled cleanup session survived its server deadline"
    );
    assert_eq!(after, before);
}

#[tokio::test]
async fn cleanup_clamps_parent_deadline_to_nearly_spent_cycle() {
    let (db, generation) = fixture().await;
    let before = usage(db.pool(), generation).await;
    let mut reader = db.pool().begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut reader, CacheTransactionMode::Read)
        .await
        .unwrap();
    let started = Instant::now();
    let outcome =
        CacheGenerationRepository::drop_if_cleanup_eligible(db.pool(), generation, &limits(80))
            .await
            .unwrap();
    let elapsed = started.elapsed();
    let after = usage(db.pool(), generation).await;
    reader.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    assert_eq!(outcome, CacheGenerationCleanupOutcome::DeferredDeadline);
    assert!(elapsed < Duration::from_millis(400));
    assert_eq!(after, before);
}

#[tokio::test]
async fn cleanup_pool_acquisition_is_inside_cycle_budget() {
    let (db, generation) = fixture().await;
    let schema = db.schema().to_string();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path',$1,false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(db.database_url())
        .await
        .unwrap();
    let held = pool.acquire().await.unwrap();
    let cleanup_pool = pool.clone();
    let started = Instant::now();
    let mut task = tokio::spawn(async move {
        CacheGenerationRepository::drop_if_cleanup_eligible(&cleanup_pool, generation, &limits(100))
            .await
    });
    let result = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
    let outcome = match result {
        Ok(joined) => Some(joined.unwrap().unwrap()),
        Err(_) => {
            task.abort();
            let _ = task.await;
            None
        }
    };
    let elapsed = started.elapsed();
    drop(held);
    pool.close().await;
    db.cleanup().await.unwrap();
    assert_eq!(
        outcome,
        Some(CacheGenerationCleanupOutcome::DeferredDeadline)
    );
    assert!(elapsed < Duration::from_millis(500));
}
