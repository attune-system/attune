use attune_common::{config::Config, test_database::TestDatabase};
use sqlx::postgres::PgPoolOptions;
use std::{path::PathBuf, time::Duration};
use tokio::time::{timeout, Instant};

fn test_config() -> Config {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config.test.yaml");
    Config::load_from_file(path.to_str().expect("UTF-8 config path")).expect("load test config")
}

async fn database_exists(database_url: &str, database_name: &str) -> bool {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await
        .expect("connect admin pool");
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_database WHERE datname = $1)",
    )
    .bind(database_name)
    .fetch_one(&pool)
    .await
    .expect("query database existence");
    pool.close().await;
    exists
}

#[tokio::test]
async fn explicit_cleanup_removes_owned_database() {
    let config = test_config();
    let database = TestDatabase::create(&config.database)
        .await
        .expect("create test database");
    let database_name = database.database_name().to_string();
    assert!(database_exists(&config.database.url, &database_name).await);
    let restoring: String = sqlx::query_scalar("SHOW timescaledb.restoring")
        .fetch_one(database.pool())
        .await
        .expect("read Timescale restoring mode");
    assert_eq!(restoring, "off");
    let compression_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM timescaledb_information.jobs WHERE proc_name = 'policy_compression' AND hypertable_schema = 'attune'",
    )
    .fetch_one(database.pool())
    .await
    .expect("count cloned Timescale jobs");
    assert_eq!(compression_jobs, 5);

    timeout(Duration::from_secs(30), database.cleanup())
        .await
        .expect("cleanup remained bounded")
        .expect("explicit cleanup succeeded");
    assert!(!database_exists(&config.database.url, &database_name).await);
}

#[tokio::test]
async fn panic_drop_fallback_removes_owned_database() {
    let config = test_config();
    let database = TestDatabase::create(&config.database)
        .await
        .expect("create test database")
        .with_cleanup_on_drop();
    let database_name = database.database_name().to_string();

    let panic_task = tokio::spawn(async move {
        let _owner = database;
        panic!("intentional fixture panic");
    });
    assert!(panic_task.await.expect_err("task must panic").is_panic());

    timeout(Duration::from_secs(30), async {
        while database_exists(&config.database.url, &database_name).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("drop fallback did not remove database");
}

#[tokio::test]
async fn held_database_lock_is_terminated_and_drop_remains_bounded() {
    let config = test_config();
    let database = TestDatabase::create(&config.database)
        .await
        .expect("create test database");
    let database_name = database.database_name().to_string();
    let pool = database.pool().clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let holder = tokio::spawn(async move {
        let mut connection = pool.acquire().await.expect("hold checkout");
        sqlx::query("BEGIN")
            .execute(&mut *connection)
            .await
            .expect("begin held transaction");
        sqlx::query("LOCK TABLE action IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *connection)
            .await
            .expect("hold database lock");
        ready_tx.send(()).ok();
        std::future::pending::<()>().await;
    });
    ready_rx.await.expect("database lock holder ready");

    let started = Instant::now();
    let cleanup_result = timeout(Duration::from_secs(30), database.cleanup())
        .await
        .expect("cleanup remained bounded");
    if let Err(error) = cleanup_result {
        assert!(
            error.to_string().contains("pool close timed out"),
            "unexpected cleanup error: {error}"
        );
    }
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!database_exists(&config.database.url, &database_name).await);

    holder.abort();
    let _ = holder.await;
}
