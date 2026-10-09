use attune_common::{config::Config, test_database::TestDatabase};
use sqlx::postgres::PgPoolOptions;
use std::{path::PathBuf, time::Duration};
use tokio::time::{timeout, Instant};

// The owning fixture already budgets 5s for pool close, 10s for connecting and
// 120s for DROP DATABASE, including its required checkpoint. See
// docs/testing/schema-per-test.md. Do not cancel that cleanup at the old 30s
// heap-schema deadline now that each clone also contains native leaf indexes.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(140);

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
    let ordinary_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_class WHERE relnamespace = current_schema()::regnamespace AND relkind = 'r' AND NOT relispartition AND relname IN ('event', 'execution_history', 'worker_history', 'sensor_process_history', 'audit_event')",
    )
    .fetch_one(database.pool())
    .await
    .expect("count cloned ordinary tables");
    assert_eq!(ordinary_tables, 2);
    let partitioned_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_class WHERE relnamespace = current_schema()::regnamespace AND relkind = 'p' AND relname IN ('event', 'execution_history', 'audit_event')",
    ).fetch_one(database.pool()).await.expect("count cloned native parents");
    assert_eq!(partitioned_tables, 3);
    let extensions: Vec<String> =
        sqlx::query_scalar("SELECT extname FROM pg_extension ORDER BY extname")
            .fetch_all(database.pool())
            .await
            .expect("read cloned extensions");
    assert!(!extensions.iter().any(|name| name == "timescaledb"));

    timeout(CLEANUP_DEADLINE, database.cleanup())
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

    // Drop joins its cleanup thread. A completed panicking task must leave no
    // database behind; polling here would hide an incorrectly detached cleanup.
    assert!(!database_exists(&config.database.url, &database_name).await);
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
    let cleanup_result = timeout(CLEANUP_DEADLINE, database.cleanup()).await;
    let elapsed = started.elapsed();
    holder.abort();
    let _ = holder.await;
    let cleanup_result = cleanup_result.expect("cleanup remained bounded");
    if let Err(error) = cleanup_result {
        assert!(
            error
                .to_string()
                .ends_with(": pool close timed out after 5s"),
            "unexpected cleanup error: {error}"
        );
    }
    assert!(elapsed < CLEANUP_DEADLINE);
    assert!(!database_exists(&config.database.url, &database_name).await);
}
