//! Fresh reclamation entry interleavings. No source rows are manufactured.
mod helpers;
use attune_common::repositories::reclamation_cancellation::CancellationOwner;

use anyhow::{ensure, Context, Result};
use attune_common::repositories::{
    cache::{CacheEntryRepository, CacheReclamationAdmission, CacheTransactionMode},
    file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
    schema_type::SchemaTypeRepository,
};
use sqlx::{Acquire, PgPool, Postgres, Transaction};
use std::time::Duration;
use tokio::time::Instant;

async fn waiting_backend(pool: &PgPool, blocker: i32) -> Result<i32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pid: Option<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE datname=current_database() \
             AND $1=ANY(pg_blocking_pids(pid)) ORDER BY pid LIMIT 1",
        )
        .bind(blocker)
        .fetch_optional(pool)
        .await?;
        if let Some(pid) = pid {
            return Ok(pid);
        }
        ensure!(
            Instant::now() < deadline,
            "owned actor never reached its lock wait"
        );
        tokio::task::yield_now().await;
    }
}

async fn pid(transaction: &mut sqlx::PgConnection) -> Result<i32> {
    Ok(sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *transaction)
        .await?)
}

async fn bounded_test_locks(transaction: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn require_no_granted_entry_locks(pool: &PgPool, actor: i32) -> Result<()> {
    let held: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted), \
         (SELECT COUNT(*) FROM pg_locks WHERE pid=$1 AND granted AND relation IN \
          ('cache_entry'::regclass,'pack'::regclass,'pack_release'::regclass, \
           'workflow_execution'::regclass,'workflow_cache_iteration'::regclass))",
    )
    .bind(actor)
    .fetch_one(pool)
    .await?;
    ensure!(held == (0, 0), "waiting entry retained locks: {held:?}");
    Ok(())
}

async fn require_acquired_and_rollback(outcome: CacheReclamationAdmission) -> Result<()> {
    let CacheReclamationAdmission::Acquired(mut transaction) = outcome else {
        anyhow::bail!("fresh entry unexpectedly exhausted its deadline");
    };
    let held: Result<(bool, bool, bool), sqlx::Error> = sqlx::query_as(
        "SELECT file_evidence_graph_held(true), \
         EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='advisory' \
                AND classid=7821101 AND objid=0 AND objsubid=2 AND granted), \
         EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() \
                AND relation='cache_entry'::regclass AND mode='AccessShareLock' AND granted)",
    )
    .fetch_one(&mut *transaction)
    .await;
    transaction.rollback().await?;
    ensure!(
        held? == (true, true, true),
        "successful entry did not retain its complete lock set"
    );
    Ok(())
}

#[tokio::test]
async fn cache_busy_preflight_does_not_queue_graph_or_block_later_capture_and_read() -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let pool = database.pool();
    let mut cache_holder = pool.begin().await?;
    bounded_test_locks(&mut cache_holder).await?;
    CacheEntryRepository::protect_transaction(&mut cache_holder, CacheTransactionMode::PinMutation)
        .await?;
    let blocker = pid(&mut cache_holder).await?;
    let entry = CacheEntryRepository::begin_reclamation_transaction(
        pool,
        &cancellation,
        Instant::now() + Duration::from_secs(15),
    );
    let observe = async {
        let result: Result<()> = async {
            let actor = waiting_backend(pool, blocker).await?;
            require_no_granted_entry_locks(pool, actor).await?;
            // This holder takes shared graph AFTER cache admission. An exclusive
            // graph waiter queued before cache preflight would block this call.
            Evidence::lock_capture(&mut cache_holder).await?;
            let mut reader = pool.begin().await?;
            let read: Result<()> = async {
                bounded_test_locks(&mut reader).await?;
                SchemaTypeRepository::lock_selection(&mut reader).await?;
                CacheEntryRepository::protect_transaction(&mut reader, CacheTransactionMode::Read)
                    .await?;
                Ok(())
            }
            .await;
            reader.rollback().await?;
            read
        }
        .await;
        cache_holder.rollback().await?;
        result
    };
    let (entry, observed) = tokio::join!(entry, observe);
    let acquired = match entry {
        Ok(outcome) => require_acquired_and_rollback(outcome).await,
        Err(error) => Err(error.into()),
    };
    cleanup_cancellation(&cancellation, database).await?;
    observed.context("cache-busy read/capture interleaving")?;
    acquired
}

#[tokio::test]
async fn graph_busy_entry_releases_preflight_and_retries_when_cache_changes_owner() -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let pool = database.pool();
    let mut graph_holder = Some(pool.begin().await?);
    Evidence::lock_capture(graph_holder.as_mut().unwrap()).await?;
    let blocker = pid(graph_holder.as_mut().unwrap()).await?;
    let mut cache_holder = None;
    let entry = CacheEntryRepository::begin_reclamation_transaction(
        pool,
        &cancellation,
        Instant::now() + Duration::from_secs(15),
    );
    let observe = async {
        let result: Result<()> = async {
            let actor = waiting_backend(pool, blocker).await?;
            require_no_granted_entry_locks(pool, actor).await?;
            // Entry completed its cache preflight and is now blocked on graph.
            // Hold cache before releasing graph to force the try-cache failure.
            cache_holder = Some(pool.begin().await?);
            let cache = cache_holder.as_mut().unwrap();
            bounded_test_locks(cache).await?;
            CacheEntryRepository::protect_transaction(cache, CacheTransactionMode::PinMutation)
                .await?;
            let cache_pid = pid(cache).await?;
            graph_holder.take().unwrap().rollback().await?;
            // The failed try must roll back exclusive graph, not wait on cache
            // with it held. The cache holder can then take shared graph.
            Evidence::lock_capture(cache).await?;
            let retrying_actor = waiting_backend(pool, cache_pid).await?;
            ensure!(retrying_actor == actor, "observed an unrelated backend");
            require_no_granted_entry_locks(pool, actor).await?;
            Ok(())
        }
        .await;
        let graph_cleanup = match graph_holder.take() {
            Some(graph) => graph.rollback().await,
            None => Ok(()),
        };
        let cache_cleanup = match cache_holder.take() {
            Some(cache) => cache.rollback().await,
            None => Ok(()),
        };
        graph_cleanup?;
        cache_cleanup?;
        result
    };
    let (entry, observed) = tokio::join!(entry, observe);
    let acquired = match entry {
        Ok(outcome) => require_acquired_and_rollback(outcome).await,
        Err(error) => Err(error.into()),
    };
    cleanup_cancellation(&cancellation, database).await?;
    observed.context("graph-first forced cache reacquisition race")?;
    acquired
}

#[derive(Debug)]
struct OwnedBackend {
    pid: i32,
    started: chrono::DateTime<chrono::Utc>,
}

async fn phase_waiter(
    pool: &PgPool,
    blocker: i32,
    cache: bool,
    mode: &str,
) -> Result<OwnedBackend> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let actor: Option<(i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT a.pid,a.backend_start FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid \
             WHERE a.datname=current_database() AND $1=ANY(pg_blocking_pids(a.pid)) \
             AND l.locktype='advisory' AND NOT l.granted AND l.mode=$3 \
             AND (($2 AND l.classid=7821101 AND l.objid=0 AND l.objsubid=2) \
               OR (NOT $2 AND l.objsubid=1 \
                 AND l.classid=((hashtextextended(current_schema() || ':named-type-graph',0)>>32)&4294967295)::OID \
                 AND l.objid=(hashtextextended(current_schema() || ':named-type-graph',0)&4294967295)::OID)) \
             ORDER BY a.pid LIMIT 1",
        )
        .bind(blocker).bind(cache).bind(mode).fetch_optional(pool).await?;
        if let Some((pid, started)) = actor {
            return Ok(OwnedBackend { pid, started });
        }
        ensure!(
            Instant::now() < deadline,
            "owned actor did not reach expected lock phase"
        );
        tokio::task::yield_now().await;
    }
}

async fn backend_gone(pool: &PgPool, actor: &OwnedBackend) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
             AND pid=$1 AND backend_start=$2)",
        )
        .bind(actor.pid)
        .bind(actor.started)
        .fetch_one(pool)
        .await?;
        if !present {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "owned backend survived disposal: {actor:?}"
        );
        tokio::task::yield_now().await;
    }
}

async fn isolated_pool(
    database: &attune_common::test_database::TestDatabase,
    lock: &str,
    statement: &str,
    catalog_only: bool,
) -> Result<PgPool> {
    isolated_pool_with_disconnect(database, lock, statement, catalog_only, None).await
}

async fn isolated_pool_with_disconnect(
    database: &attune_common::test_database::TestDatabase,
    lock: &str,
    statement: &str,
    catalog_only: bool,
    disconnect: Option<&str>,
) -> Result<PgPool> {
    let options = database.pool().connect_options().as_ref().clone();
    let schema = if catalog_only {
        "pg_catalog".to_owned()
    } else {
        format!("\"{}\"", database.schema().replace('"', "\"\""))
    };
    let lock = lock.to_owned();
    let statement = statement.to_owned();
    let disconnect = disconnect.map(str::to_owned);
    Ok(sqlx::postgres::PgPoolOptions::new().max_connections(1)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            let lock = lock.clone();
            let statement = statement.clone();
            let disconnect = disconnect.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path',$1,false), set_config('lock_timeout',$2,false), set_config('statement_timeout',$3,false)")
                    .bind(schema).bind(lock).bind(statement).execute(&mut *connection).await?;
                if let Some(disconnect) = disconnect {
                    sqlx::query("SELECT set_config('client_connection_check_interval',$1,false)")
                        .bind(disconnect).execute(&mut *connection).await?;
                }
                Ok(())
            })
        }).connect_with(options).await?)
}

async fn numeric_settings(connection: &mut sqlx::PgConnection) -> Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT setting::BIGINT FROM pg_settings WHERE name='lock_timeout'), \
         (SELECT setting::BIGINT FROM pg_settings WHERE name='statement_timeout')",
    )
    .fetch_one(connection)
    .await?)
}

async fn blocked_end_case(graph: bool, cancel: bool) -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let pool = database.pool();
    let isolated = isolated_pool(&database, "0", "0", false).await?;
    let mut holder = pool.begin().await?;
    if graph {
        Evidence::lock_capture(&mut holder).await?;
    } else {
        CacheEntryRepository::protect_transaction(&mut holder, CacheTransactionMode::PinMutation)
            .await?;
    }
    let blocker = pid(&mut holder).await?;
    let deadline = Instant::now() + Duration::from_secs(if cancel { 15 } else { 8 });
    let worker_pool = isolated.clone();
    let worker_cancellation = cancellation.clone();
    let mut task = Some(tokio::spawn(async move {
        CacheEntryRepository::begin_reclamation_transaction(
            &worker_pool,
            &worker_cancellation,
            deadline,
        )
        .await
    }));
    let result: Result<()> = async {
        let actor = phase_waiter(pool, blocker, !graph, "ExclusiveLock").await?;
        ensure!(Instant::now() < deadline, "never observed the phase before its deadline");
        require_no_granted_entry_locks(pool, actor.pid).await?;
        let worker = task.take().unwrap();
        if cancel {
            worker.abort();
            let joined = worker.await;
            ensure!(matches!(joined, Err(ref error) if error.is_cancelled()), "worker did not acknowledge cancellation");
        } else {
            match worker.await?? {
                CacheReclamationAdmission::Unavailable => anyhow::bail!("owned cancellation channel unavailable"),
                CacheReclamationAdmission::DeadlineExceeded => {}
                CacheReclamationAdmission::Acquired(transaction) => {
                    transaction.rollback().await?;
                    anyhow::bail!("blocked entry acquired without blocker release");
                }
            }
        }
        // The blocker is deliberately still held. PID plus backend_start proves
        // disposal, not merely a successful SELECT on a different connection.
        backend_gone(pool, &actor).await?;
        ensure!(pid(&mut holder).await? == blocker, "blocker connection was lost");
        let held: bool = if graph {
            sqlx::query_scalar("SELECT file_evidence_graph_held(false)").fetch_one(&mut *holder).await?
        } else {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='advisory' AND classid=7821101 AND objid=0 AND objsubid=2 AND granted)").fetch_one(&mut *holder).await?
        };
        ensure!(held, "blocker was released before disposal evidence");
        let mut replacement = isolated.acquire().await?;
        ensure!(numeric_settings(&mut replacement).await? == (0, 0), "acquisition settings leaked after failed/cancelled entry");
        Ok(())
    }.await;
    if let Some(worker) = task.take() {
        worker.abort();
        let _ = worker.await;
    }
    let rollback = holder.rollback().await;
    isolated.close().await;
    let cleanup = cleanup_cancellation(&cancellation, database).await;
    rollback?;
    cleanup?;
    result
}

#[tokio::test]
async fn blocked_cache_deadline_disposes_exact_backend_before_blocker_release() -> Result<()> {
    blocked_end_case(false, false).await
}
#[tokio::test]
async fn blocked_graph_deadline_disposes_exact_backend_before_blocker_release() -> Result<()> {
    blocked_end_case(true, false).await
}
#[tokio::test]
async fn cancelled_cache_wait_disposes_exact_backend_before_blocker_release() -> Result<()> {
    blocked_end_case(false, true).await
}
#[tokio::test]
async fn cancelled_graph_wait_disposes_exact_backend_before_blocker_release() -> Result<()> {
    blocked_end_case(true, true).await
}

#[tokio::test]
async fn constructor_first_retains_graph_and_cache_until_caller_rollback() -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let pool = database.pool();
    let outcome = CacheEntryRepository::begin_reclamation_transaction(
        pool,
        &cancellation,
        Instant::now() + Duration::from_secs(15),
    )
    .await?;
    let CacheReclamationAdmission::Acquired(mut owner) = outcome else {
        cleanup_cancellation(&cancellation, database).await?;
        anyhow::bail!("uncontended entry exhausted deadline");
    };
    let blocker = pid(&mut owner).await?;
    let capture_pool = pool.clone();
    let capture = tokio::spawn(async move {
        let mut transaction = capture_pool.begin().await?;
        let result: attune_common::Result<()> = async {
            sqlx::query("SET LOCAL lock_timeout='15s'")
                .execute(&mut *transaction)
                .await?;
            Evidence::lock_capture(&mut transaction).await
        }
        .await;
        transaction.rollback().await?;
        result
    });
    let mutation_pool = pool.clone();
    let mutation = tokio::spawn(async move {
        let mut transaction = mutation_pool.begin().await?;
        let result: attune_common::Result<()> = async {
            sqlx::query("SET LOCAL lock_timeout='15s'")
                .execute(&mut *transaction)
                .await?;
            CacheEntryRepository::protect_transaction(
                &mut transaction,
                CacheTransactionMode::PinMutation,
            )
            .await
        }
        .await;
        transaction.rollback().await?;
        result
    });
    let observation: Result<()> = async {
        let shared = phase_waiter(pool, blocker, false, "ShareLock").await?;
        let writer = phase_waiter(pool, blocker, true, "ExclusiveLock").await?;
        ensure!(
            shared.pid != writer.pid,
            "did not observe both owned actors"
        );
        ensure!(
            !capture.is_finished() && !mutation.is_finished(),
            "a later actor bypassed the retained gate"
        );
        Ok(())
    }
    .await;
    let rollback = owner.rollback().await;
    let captured = capture.await;
    let mutated = mutation.await;
    let cleanup = cleanup_cancellation(&cancellation, database).await;
    rollback?;
    cleanup?;
    observation?;
    captured??;
    mutated??;
    Ok(())
}

#[tokio::test]
async fn successful_entry_restores_original_settings_before_handoff() -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let isolated = isolated_pool(&database, "27000ms", "29000ms", false).await?;
    let result: Result<()> = async {
        let outcome = CacheEntryRepository::begin_reclamation_transaction(
            &isolated,
            &cancellation,
            Instant::now() + Duration::from_secs(5),
        )
        .await?;
        let CacheReclamationAdmission::Acquired(mut transaction) = outcome else {
            anyhow::bail!("uncontended entry exhausted deadline");
        };
        let settings = numeric_settings(&mut transaction).await;
        let actor: Result<(i32, chrono::DateTime<chrono::Utc>), sqlx::Error> = sqlx::query_as(
            "SELECT pid,backend_start FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(&mut *transaction)
        .await;
        transaction.rollback().await?;
        ensure!(
            settings? == (27000, 29000),
            "successful SAVEPOINT release retained acquisition timeouts"
        );
        let (pid, started) = actor?;
        backend_gone(database.pool(), &OwnedBackend { pid, started }).await?;
        let mut replacement = isolated.acquire().await?;
        ensure!(
            numeric_settings(&mut replacement).await? == (27000, 29000),
            "pool baseline was changed by local acquisition settings"
        );
        Ok(())
    }
    .await;
    isolated.close().await;
    let cleanup = cleanup_cancellation(&cancellation, database).await;
    cleanup?;
    result
}

async fn shorter_operator_limit(lock: &str, statement: &str, expected: &str) -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let pool = database.pool();
    let isolated = isolated_pool(&database, lock, statement, false).await?;
    let original = {
        let mut connection = isolated.acquire().await?;
        numeric_settings(&mut connection).await?
    };
    let mut holder = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut holder, CacheTransactionMode::PinMutation)
        .await?;
    let blocker = pid(&mut holder).await?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let worker_pool = isolated.clone();
    let worker_cancellation = cancellation.clone();
    let mut task = Some(tokio::spawn(async move {
        CacheEntryRepository::begin_reclamation_transaction(
            &worker_pool,
            &worker_cancellation,
            deadline,
        )
        .await
    }));
    let result: Result<()> = async {
        let actor = phase_waiter(pool, blocker, true, "ExclusiveLock").await?;
        let outcome = task.take().unwrap().await?;
        let error = match outcome {
            Err(attune_common::Error::Database(error)) => error,
            Ok(CacheReclamationAdmission::Acquired(transaction)) => {
                transaction.rollback().await?;
                anyhow::bail!("operator-limited entry unexpectedly acquired");
            }
            other => anyhow::bail!("operator limit lost its normal error distinction: {other:?}"),
        };
        ensure!(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref()
                == Some(expected),
            "wrong operator-limit error"
        );
        ensure!(
            Instant::now() < deadline,
            "shorter operator limit was extended to request budget"
        );
        backend_gone(pool, &actor).await?;
        let mut connection = isolated.acquire().await?;
        ensure!(
            numeric_settings(&mut connection).await? == original,
            "failure changed baseline settings"
        );
        Ok(())
    }
    .await;
    if let Some(worker) = task.take() {
        worker.abort();
        let _ = worker.await;
    }
    let rollback = holder.rollback().await;
    isolated.close().await;
    let cleanup = cleanup_cancellation(&cancellation, database).await;
    rollback?;
    cleanup?;
    result
}

#[tokio::test]
async fn shorter_operator_lock_limit_is_not_extended_or_relabelled_total_deadline() -> Result<()> {
    shorter_operator_limit("3s", "0", "55P03").await
}
#[tokio::test]
async fn shorter_operator_statement_limit_is_not_extended_or_relabelled_total_deadline(
) -> Result<()> {
    shorter_operator_limit("0", "3s", "57014").await
}

#[tokio::test]
async fn missing_guard_in_isolated_session_is_a_real_error_not_contention() -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    // Session search_path only. No business row, function or schema is modified.
    let isolated = isolated_pool(&database, "0", "0", true).await?;
    let result: Result<()> = async {
        let (pid, started): (i32, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
            "SELECT pid,backend_start FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(&isolated)
        .await?;
        let outcome = CacheEntryRepository::begin_reclamation_transaction(
            &isolated,
            &cancellation,
            Instant::now() + Duration::from_secs(5),
        )
        .await;
        let error = match outcome {
            Err(attune_common::Error::Database(error)) => error,
            Ok(CacheReclamationAdmission::Acquired(transaction)) => {
                transaction.rollback().await?;
                anyhow::bail!("missing guard unexpectedly admitted reclamation");
            }
            other => anyhow::bail!("real failure was converted to contention: {other:?}"),
        };
        ensure!(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref()
                == Some("42883"),
            "expected undefined guard function error"
        );
        backend_gone(database.pool(), &OwnedBackend { pid, started }).await?;
        Ok(())
    }
    .await;
    isolated.close().await;
    let cleanup = cleanup_cancellation(&cancellation, database).await;
    cleanup?;
    result
}

async fn disconnect_setting(connection: &mut sqlx::PgConnection) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT setting::BIGINT FROM pg_settings WHERE name='client_connection_check_interval'",
    )
    .fetch_one(connection)
    .await?)
}

async fn disconnect_handoff_case(original: &str, expected: i64, commit: bool) -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let isolated =
        isolated_pool_with_disconnect(&database, "27000ms", "29000ms", false, Some(original))
            .await?;
    let baseline = {
        let mut connection = isolated.acquire().await?;
        disconnect_setting(&mut connection).await?
    };
    let result: Result<()> = async {
        let outcome = CacheEntryRepository::begin_reclamation_transaction(
            &isolated,
            &cancellation,
            Instant::now() + Duration::from_secs(15),
        )
        .await?;
        let CacheReclamationAdmission::Acquired(mut transaction) = outcome else {
            anyhow::bail!("disconnect-policy fixture did not acquire");
        };
        let actor: (i32, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
            "SELECT pid,backend_start FROM pg_stat_activity WHERE pid=pg_backend_pid()",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let observed: Result<()> = async {
            ensure!(
                disconnect_setting(&mut transaction).await? == expected,
                "wrong handoff disconnect interval"
            );
            ensure!(
                numeric_settings(&mut transaction).await? == (27000, 29000),
                "acquisition timeout settings were not restored"
            );
            // Constructor preflight rollback has already occurred. Also prove
            // that a later nested rollback restores the retained ROOT setting.
            let mut savepoint = transaction.transaction_mut().begin().await?;
            let changed =
                sqlx::query("SELECT set_config('client_connection_check_interval','10ms',true)")
                    .execute(&mut *savepoint)
                    .await;
            let rollback = savepoint.rollback().await;
            rollback?;
            changed?;
            ensure!(
                disconnect_setting(&mut transaction).await? == expected,
                "nested rollback lost root disconnect policy"
            );
            Ok(())
        }
        .await;
        // Exercise the normal consuming API on both successful root outcomes.
        let ended = if commit {
            transaction.commit().await
        } else {
            transaction.rollback().await
        };
        ended?;
        observed?;
        backend_gone(
            database.pool(),
            &OwnedBackend {
                pid: actor.0,
                started: actor.1,
            },
        )
        .await?;
        let mut replacement = isolated.acquire().await?;
        ensure!(
            disconnect_setting(&mut replacement).await? == baseline,
            "replacement did not inherit operator disconnect policy"
        );
        Ok(())
    }
    .await;
    isolated.close().await;
    let cleanup = cleanup_cancellation(&cancellation, database).await;
    cleanup?;
    result
}

#[tokio::test]
async fn operator_250ms_disconnect_checks_survive_handoff_and_nested_rollback() -> Result<()> {
    for commit in [false, true] {
        disconnect_handoff_case("250ms", 250, commit).await?;
    }
    Ok(())
}

#[tokio::test]
async fn root_commit_and_rollback_preserve_original_disconnect_pool_policy() -> Result<()> {
    for commit in [false, true] {
        disconnect_handoff_case("0", 0, commit).await?;
        disconnect_handoff_case("2500ms", 2500, commit).await?;
    }
    Ok(())
}

// Normal consuming commit/rollback closes this constructor's connection, so a
// replacement's after_connect settings alone cannot prove SAME-SESSION reset.
// This companion uses the exact public transaction-manager operation called by
// those consuming methods, observes the same backend after depth returns to 0,
// then consumes the wrapper with its now-no-op rollback. No raw COMMIT SQL or
// private SQLx state manipulation is used. SQLx 0.8.6 rollback at depth 0 is a no-op.
#[tokio::test]
async fn transaction_manager_root_end_restores_disconnect_on_the_same_backend() -> Result<()> {
    use sqlx::TransactionManager;
    type Manager = <Postgres as sqlx::Database>::TransactionManager;
    for commit in [false, true] {
        let database = helpers::create_test_pool().await?;
        let cancellation = CancellationOwner::new(database.pool());
        let isolated =
            isolated_pool_with_disconnect(&database, "0", "0", false, Some("2500ms")).await?;
        let result: Result<()> = async {
            let outcome = CacheEntryRepository::begin_reclamation_transaction(
                &isolated,
                &cancellation,
                Instant::now() + Duration::from_secs(15),
            )
            .await?;
            let CacheReclamationAdmission::Acquired(mut transaction) = outcome else {
                anyhow::bail!("root restoration fixture did not acquire");
            };
            let observed: Result<()> = async {
                ensure!(
                    Manager::get_transaction_depth(&transaction) == 1,
                    "unexpected nested transaction at handoff"
                );
                ensure!(
                    disconnect_setting(&mut transaction).await? == 2500,
                    "channel must preserve operator disconnect interval"
                );
                // Caller-owned local policy still restores at root end. The
                // cancellation channel itself does not require this GUC.
                sqlx::query("SELECT set_config('client_connection_check_interval','10ms',true)")
                    .execute(&mut *transaction)
                    .await?;
                if commit {
                    Manager::commit(&mut transaction).await?;
                } else {
                    Manager::rollback(&mut transaction).await?;
                }
                ensure!(
                    Manager::get_transaction_depth(&transaction) == 0,
                    "root transaction did not end"
                );
                ensure!(
                    disconnect_setting(&mut transaction).await? == 2500,
                    "SET LOCAL leaked past root end on original backend"
                );
                Ok(())
            }
            .await;
            let closed = transaction.rollback().await;
            closed?;
            observed
        }
        .await;
        isolated.close().await;
        let cleanup = cleanup_cancellation(&cancellation, database).await;
        cleanup?;
        result?;
    }
    Ok(())
}

async fn sentinel_waiter(pool: &PgPool, blocker: i32, actor: &OwnedBackend) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid \
             WHERE a.datname=current_database() AND a.pid=$1 AND a.backend_start=$2 \
             AND $3=ANY(pg_blocking_pids(a.pid)) AND l.locktype='advisory' \
             AND l.classid=7821102 AND l.objid=0 AND l.objsubid=2 AND NOT l.granted)",
        )
        .bind(actor.pid)
        .bind(actor.started)
        .bind(blocker)
        .fetch_one(pool)
        .await?;
        if waiting {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "handoff owner did not reach the owned sentinel wait"
        );
        tokio::task::yield_now().await;
    }
}

async fn cancellation_after_handoff_case(original_disconnect: &str) -> Result<()> {
    let database = helpers::create_test_pool().await?;
    let cancellation = CancellationOwner::new(database.pool());
    let pool = database.pool();
    let isolated =
        isolated_pool_with_disconnect(&database, "0", "0", false, Some(original_disconnect))
            .await?;
    let mut sentinel = pool.begin().await?;
    // Separate two-integer key, exclusively owned by this disposable DB/test.
    sqlx::query("SELECT pg_advisory_xact_lock(7821102,0)")
        .execute(&mut *sentinel)
        .await?;
    let blocker = pid(&mut sentinel).await?;
    let outcome = CacheEntryRepository::begin_reclamation_transaction(
        &isolated,
        &cancellation,
        Instant::now() + Duration::from_secs(15),
    )
    .await?;
    let CacheReclamationAdmission::Acquired(mut transaction) = outcome else {
        sentinel.rollback().await?;
        isolated.close().await;
        cleanup_cancellation(&cancellation, database).await?;
        anyhow::bail!("handoff cancellation fixture did not acquire");
    };
    let actor_row: (i32, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT pid,backend_start FROM pg_stat_activity WHERE pid=pg_backend_pid()")
            .fetch_one(&mut *transaction)
            .await?;
    let actor = OwnedBackend {
        pid: actor_row.0,
        started: actor_row.1,
    };
    let mut owner = Some(tokio::spawn(async move {
        let result: attune_common::Result<()> = async {
            // Fail-safe for this NEW target wait, not a shorter constructor or
            // cancellation deadline. Existing two cancellation tests are untouched.
            sqlx::query("SET LOCAL statement_timeout='15s'")
                .execute(&mut *transaction)
                .await?;
            sqlx::query("SELECT pg_advisory_xact_lock(7821102,0)")
                .execute(&mut *transaction)
                .await?;
            Ok(())
        }
        .await;
        transaction.rollback().await?;
        result
    }));
    let capture_pool = pool.clone();
    let capture = tokio::spawn(async move {
        let mut transaction = capture_pool.begin().await?;
        let result: attune_common::Result<()> = async {
            sqlx::query("SET LOCAL lock_timeout='15s'")
                .execute(&mut *transaction)
                .await?;
            Evidence::lock_capture(&mut transaction).await
        }
        .await;
        transaction.rollback().await?;
        result
    });
    let mutation_pool = pool.clone();
    let mutation = tokio::spawn(async move {
        let mut transaction = mutation_pool.begin().await?;
        let result: attune_common::Result<()> = async {
            sqlx::query("SET LOCAL lock_timeout='15s'")
                .execute(&mut *transaction)
                .await?;
            CacheEntryRepository::protect_transaction(
                &mut transaction,
                CacheTransactionMode::PinMutation,
            )
            .await
        }
        .await;
        transaction.rollback().await?;
        result
    });
    let observed: Result<()> = async {
        sentinel_waiter(pool, blocker, &actor).await?;
        let graph_waiter = phase_waiter(pool, actor.pid, false, "ShareLock").await?;
        let cache_waiter = phase_waiter(pool, actor.pid, true, "ExclusiveLock").await?;
        ensure!(graph_waiter.pid != cache_waiter.pid, "did not observe independent graph/cache waiters");
        let worker = owner.take().unwrap();
        worker.abort();
        ensure!(matches!(worker.await, Err(ref error) if error.is_cancelled()), "handoff owner did not acknowledge cancellation");
        backend_gone(pool, &actor).await?;
        let still_held: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='advisory' AND classid=7821102 AND objid=0 AND objsubid=2 AND granted)")
            .fetch_one(&mut *sentinel).await?;
        ensure!(still_held, "sentinel released before exact backend disposal evidence");
        Ok(())
    }.await;
    if let Some(worker) = owner.take() {
        worker.abort();
        let _ = worker.await;
    }
    let rollback = sentinel.rollback().await;
    let captured = capture.await;
    let mutated = mutation.await;
    isolated.close().await;
    let cleanup = cleanup_cancellation(&cancellation, database).await;
    rollback?;
    cleanup?;
    observed?;
    captured??;
    mutated??;
    Ok(())
}

#[tokio::test]
async fn cancellation_after_handoff_disposes_owner_and_unblocks_graph_and_cache() -> Result<()> {
    cancellation_after_handoff_case("0").await
}

// Additional source-backed rearming probe, not a claimed observed failure.
// A pre-existing positive timer may remain armed after its GUC is lowered.
#[tokio::test]
async fn long_operator_disconnect_interval_does_not_delay_first_handoff_cancellation_check(
) -> Result<()> {
    cancellation_after_handoff_case("25000ms").await
}

async fn cleanup_cancellation(
    cancellation: &std::sync::Arc<CancellationOwner>,
    database: attune_common::test_database::TestDatabase,
) -> Result<()> {
    let shutdown = cancellation
        .shutdown(Instant::now() + Duration::from_secs(10))
        .await;
    let cleanup = database.cleanup().await;
    shutdown?;
    cleanup?;
    Ok(())
}
