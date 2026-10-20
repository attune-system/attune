use super::*;
use crate::{
    config::Config,
    repositories::cache::{CacheEntryRepository, CacheReclamationAdmission},
    test_database::TestDatabase,
};
use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};

async fn database() -> Result<TestDatabase> {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))?;
    Ok(TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop())
}
fn primary(database: &TestDatabase) -> PgPool {
    let schema = format!("\"{}\"", database.schema().replace('"', "\"\""));
    sqlx::postgres::PgPoolOptions::new()
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
        .connect_lazy_with(database.pool().connect_options().as_ref().clone())
}
async fn identity(connection: &mut PgConnection) -> Result<(i32, DateTime<Utc>)> {
    Ok(
        sqlx::query_as("SELECT pid,backend_start FROM pg_stat_activity WHERE pid=pg_backend_pid()")
            .fetch_one(connection)
            .await?,
    )
}
async fn waiting(pool: &PgPool, target: &(i32, DateTime<Utc>)) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start=$2 AND wait_event_type='Lock' AND wait_event='advisory')")
        .bind(target.0).bind(target.1).fetch_one(pool).await?)
}
async fn ready(pool: &PgPool, target: &(i32, DateTime<Utc>)) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !waiting(pool, target).await? {
        ensure!(
            Instant::now() < deadline,
            "owned actor never reached sentinel wait"
        );
        tokio::task::yield_now().await;
    }
    Ok(())
}
async fn gone(pool: &PgPool, target: &(i32, DateTime<Utc>)) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start=$2)",
        )
        .bind(target.0)
        .bind(target.1)
        .fetch_one(pool)
        .await?;
        if !present {
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "owned backend survived disposal");
        tokio::task::yield_now().await;
    }
}
async fn admitted(pool: &PgPool, owner: &Arc<CancellationOwner>) -> Result<ReclamationTransaction> {
    let outcome = CacheEntryRepository::begin_reclamation_transaction(
        pool,
        owner,
        Instant::now() + Duration::from_secs(15),
    )
    .await?;
    let CacheReclamationAdmission::Acquired(transaction) = outcome else {
        anyhow::bail!("fixture did not acquire");
    };
    Ok(transaction)
}
async fn cleanup(
    database: TestDatabase,
    primary: PgPool,
    owner: Arc<CancellationOwner>,
    result: Result<()>,
) -> Result<()> {
    let shutdown = owner
        .shutdown(Instant::now() + Duration::from_secs(10))
        .await;
    primary.close().await;
    let cleanup = database.cleanup().await;
    shutdown?;
    cleanup?;
    result
}

#[tokio::test]
async fn single_control_permit_is_reserved_before_main_and_in_total_budget() -> Result<()> {
    let database = database().await?;
    let primary = primary(&database);
    let owner = CancellationOwner::new(&primary);
    let result: Result<()> = async {
        let held = primary.acquire().await?;
        let reservation = owner
            .reserve(Instant::now() + Duration::from_secs(5))
            .await?;
        ensure!(owner.slots.available_permits() == 0);
        ensure!(primary.options().get_max_connections() == 1);
        let outcome = CacheEntryRepository::begin_reclamation_transaction(
            &primary,
            &owner,
            Instant::now() + Duration::from_millis(200),
        )
        .await?;
        ensure!(matches!(
            outcome,
            CacheReclamationAdmission::DeadlineExceeded
        ));
        ensure!(
            owner.slots.available_permits() == 0,
            "request bypassed reserved control slot"
        );
        drop(reservation);
        drop(held);
        Ok(())
    }
    .await;
    cleanup(database, primary, owner, result).await
}

#[tokio::test]
async fn dropped_blocked_query_with_main_one_uses_backend_key_and_disposes_exact_owner(
) -> Result<()> {
    let database = database().await?;
    let primary = primary(&database);
    let owner = CancellationOwner::new(&primary);
    let mut blocker = database.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(7821198,991)")
        .execute(&mut *blocker)
        .await?;
    let mut task = None;
    let result: Result<()> = async {
        let mut transaction = admitted(&primary, &owner).await?;
        let target = identity(&mut transaction).await?;
        ensure!(
            owner.slots.available_permits() == 0,
            "handoff lost control custody"
        );
        task = Some(tokio::spawn(async move {
            sqlx::query("SELECT pg_advisory_xact_lock(7821198,991)")
                .execute(&mut *transaction)
                .await
        }));
        ready(database.pool(), &target).await?;
        let worker = task.take().unwrap();
        worker.abort();
        ensure!(matches!(worker.await,Err(error) if error.is_cancelled()));
        gone(database.pool(), &target).await?;
        Ok(())
    }
    .await;
    if let Some(worker) = task {
        worker.abort();
        let _ = worker.await;
    }
    let rollback = blocker.rollback().await;
    let cleanup = cleanup(database, primary, owner, result).await;
    rollback?;
    cleanup
}

#[tokio::test]
async fn another_sessions_opaque_token_cannot_interrupt_owned_blocked_session() -> Result<()> {
    let database = database().await?;
    let primary = primary(&database);
    let owner = CancellationOwner::new(&primary);
    let mut blocker = database.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(7821198,992)")
        .execute(&mut *blocker)
        .await?;
    let mut task = None;
    let result: Result<()> = async {
        let reservation = owner
            .reserve(Instant::now() + Duration::from_secs(5))
            .await?;
        let mut a = primary.acquire().await?;
        a.close_on_drop();
        let token_a = a.cancel_token()?;
        let mut b = database.pool().acquire().await?;
        b.close_on_drop();
        let token_b = b.cancel_token()?;
        let target = identity(&mut b).await?;
        task = Some(tokio::spawn(async move {
            sqlx::query("SELECT pg_advisory_xact_lock(7821198,992)")
                .execute(&mut *b)
                .await
        }));
        ready(database.pool(), &target).await?;
        tokio::time::timeout(Duration::from_secs(5), token_a.cancel()).await??;
        ensure!(
            waiting(database.pool(), &target).await?,
            "A's token interrupted B"
        );
        ensure!(!task.as_ref().unwrap().is_finished());
        let one: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&mut *a).await?;
        ensure!(one == 1);
        ensure!(
            owner.slots.available_permits() == 0,
            "cancel completion released someone else's lease"
        );
        tokio::time::timeout(Duration::from_secs(5), token_b.cancel()).await??;
        let query = tokio::time::timeout(Duration::from_secs(5), task.as_mut().unwrap()).await??;
        task.take();
        let error = query.expect_err("B's own token did not cancel its blocked query");
        ensure!(error
            .as_database_error()
            .and_then(|error| error.code())
            .is_some_and(|code| code == "57014"));
        gone(database.pool(), &target).await?;
        drop(a);
        drop(reservation);
        Ok(())
    }
    .await;
    if let Some(worker) = task {
        worker.abort();
        let _ = worker.await;
    }
    let rollback = blocker.rollback().await;
    let cleanup = cleanup(database, primary, owner, result).await;
    rollback?;
    cleanup
}

#[tokio::test]
async fn shutdown_joins_monitor_and_released_wrapper_before_returning() -> Result<()> {
    let database = database().await?;
    let primary = primary(&database);
    let owner = CancellationOwner::new(&primary);
    let mut worker = None;
    let result: Result<()> = async {
        let mut transaction = admitted(&primary, &owner).await?;
        let target = identity(&mut transaction).await?;
        let task_owner = owner.clone();
        worker = Some(tokio::spawn(async move {
            task_owner.closed().await;
            drop(transaction);
        }));
        owner
            .shutdown(Instant::now() + Duration::from_secs(10))
            .await?;
        worker.take().unwrap().await?;
        ensure!(owner.state().jobs.is_empty());
        ensure!(owner.slots.available_permits() == 1);
        gone(database.pool(), &target).await?;
        let outcome = CacheEntryRepository::begin_reclamation_transaction(
            &primary,
            &owner,
            Instant::now() + Duration::from_secs(1),
        )
        .await?;
        ensure!(matches!(outcome, CacheReclamationAdmission::Unavailable));
        Ok(())
    }
    .await;
    if let Some(task) = worker {
        task.abort();
        let _ = task.await;
    }
    cleanup(database, primary, owner, result).await
}

#[tokio::test]
async fn unknown_commit_never_retries_or_inserts_a_fresh_row() -> Result<()> {
    use sqlx::TransactionManager;
    type Manager = <Postgres as sqlx::Database>::TransactionManager;
    let database = database().await?;
    let primary = primary(&database);
    let owner = CancellationOwner::new(&primary);
    let result: Result<()>=async {
        sqlx::query("CREATE TABLE reclamation_unknown_commit_probe (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY)")
            .execute(database.pool()).await?;
        let mut transaction=admitted(&primary,&owner).await?;
        let target=identity(&mut transaction).await?;
        sqlx::query("INSERT INTO reclamation_unknown_commit_probe DEFAULT VALUES").execute(&mut *transaction).await?;
        transaction.guard.as_ref().unwrap().phase.store(COMMIT_PENDING,Ordering::Release);
        // Deterministic application-ack-loss seam: real database COMMIT, but no
        // guard acknowledgement. Not a claimed wire-level COMMIT-loss test.
        Manager::commit(&mut transaction).await?;
        drop(transaction);
        ensure!(owner.shutdown(Instant::now()+Duration::from_secs(10)).await.is_err(),"UNKNOWN became clean shutdown");
        gone(database.pool(),&target).await?;
        let rows:(i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(MAX(id),0) FROM reclamation_unknown_commit_probe")
            .fetch_one(database.pool()).await?;
        ensure!(rows==(1,1),"unknown commit was retried into fresh work");
        let outcome=CacheEntryRepository::begin_reclamation_transaction(&primary,&owner,Instant::now()+Duration::from_secs(1)).await?;
        ensure!(matches!(outcome,CacheReclamationAdmission::Unavailable));
        Ok(())
    }.await;
    // This test deliberately expects poisoned teardown. Still join again and
    // remove all run-owned resources, without treating poison as success.
    let shutdown = owner
        .shutdown(Instant::now() + Duration::from_secs(10))
        .await;
    primary.close().await;
    let cleanup = database.cleanup().await;
    ensure!(shutdown.is_err());
    cleanup?;
    result
}

#[tokio::test]
async fn default_owner_admits_without_new_routing_configuration() -> Result<()> {
    let database = database().await?;
    let primary = primary(&database);
    let owner = CancellationOwner::new(&primary);
    let result: Result<()> = async {
        let mut transaction = admitted(&primary, &owner).await?;
        let target = identity(&mut transaction).await?;
        transaction.rollback().await?;
        gone(database.pool(), &target).await?;
        ensure!(owner.slots.available_permits() == 1);
        Ok(())
    }
    .await;
    cleanup(database, primary, owner, result).await
}

#[tokio::test]
async fn pending_owned_cleanup_retains_only_permit_and_shutdown_can_retry_join() -> Result<()> {
    let database = database().await?;
    let primary = primary(&database);
    let owner = CancellationOwner::new(&primary);
    let permit = owner.slots.clone().acquire_owned().await?;
    let (release, wait) = oneshot::channel::<()>();
    // Deterministic custody seam, not a fabricated PgCancelToken or a claimed
    // live TLS timeout. The tracked task stands in for pending transport work.
    owner.state().jobs.push(tokio::spawn(async move {
        let released = wait.await.is_ok();
        drop(permit);
        released
    }));
    let result: Result<()> = async {
        ensure!(owner
            .shutdown(Instant::now() + Duration::from_millis(100))
            .await
            .is_err());
        ensure!(owner.slots.available_permits() == 0);
        ensure!(
            owner.state().jobs.len() == 1,
            "timed-out shutdown detached the owner task"
        );
        Ok(())
    }
    .await;
    let _ = release.send(());
    let shutdown = owner
        .shutdown(Instant::now() + Duration::from_secs(5))
        .await;
    let joined = owner.state().jobs.is_empty();
    let available = owner.slots.available_permits();
    primary.close().await;
    let cleanup = database.cleanup().await;
    shutdown?;
    cleanup?;
    result?;
    ensure!(joined && available == 1);
    Ok(())
}
