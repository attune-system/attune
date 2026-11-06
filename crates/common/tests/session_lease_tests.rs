mod helpers;
#[path = "../../../tests/sqlx-session/src/relay.rs"]
mod relay;
use attune_common::db::SessionLease;
use sqlx::{postgres::PgPoolOptions, Executor};
use std::time::Duration;

#[tokio::test]
async fn cancellation_before_lock_ack_disposes_session() {
    let db = helpers::create_test_pool().await.unwrap();
    let opts: sqlx::postgres::PgConnectOptions = db.database_url().parse().unwrap();
    let (port, seen, release, task) =
        relay::relay(format!("{}:{}", opts.get_host(), opts.get_port()), b'T', 2).await;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            opts.host("127.0.0.1")
                .port(port)
                .ssl_mode(sqlx::postgres::PgSslMode::Disable),
        )
        .await
        .unwrap();
    {
        let work = async {
            let mut lease = SessionLease::acquire(&pool).await.unwrap();
            lease.begin_transaction().await.unwrap();
            lease
                .execute("SELECT pg_advisory_lock(918273645)")
                .await
                .unwrap();
            panic!("lock acknowledgement gate must hold");
        };
        tokio::pin!(work);
        tokio::select! {
            _ = seen => (),
            _ = &mut work => panic!("premature completion"),
            _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("lock gate deadline"),
        }
    }
    let _ = release.send(());
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let locks: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND locktype='advisory' AND objid=918273645",
            ).fetch_one(&*db).await.unwrap();
            if locks == 0 { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("cancelled lock-owning backend must terminate");
    pool.close().await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancellation_during_post_commit_publication_disposes_session_lock() {
    let db = helpers::create_test_pool().await.unwrap();
    let (ready, observed) = tokio::sync::oneshot::channel();
    let pool = (*db).clone();
    let task = tokio::spawn(async move {
        let mut lease = SessionLease::acquire(&pool).await.unwrap();
        lease.begin_transaction().await.unwrap();
        lease
            .execute("SELECT pg_advisory_lock(918273646)")
            .await
            .unwrap();
        lease.commit_transaction().await.unwrap();
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    tokio::time::timeout(Duration::from_secs(10), observed)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let locks: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND locktype='advisory' AND objid=918273646",
            )
            .fetch_one(&*db)
            .await
            .unwrap();
            if locks == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn acknowledged_unlock_preserves_healthy_session_reuse() {
    let db = helpers::create_test_pool().await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(db.connect_options().as_ref().clone())
        .await
        .unwrap();
    let mut lease = SessionLease::acquire(&pool).await.unwrap();
    let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lease)
        .await
        .unwrap();
    lease
        .execute("SELECT pg_advisory_lock(918273648)")
        .await
        .unwrap();
    lease.begin_transaction().await.unwrap();
    {
        let mut nested = sqlx::Connection::begin(&mut *lease).await.unwrap();
        nested.execute("SELECT 1").await.unwrap();
        nested.commit().await.unwrap();
    }
    lease.commit_transaction().await.unwrap();
    lease.begin_transaction().await.unwrap();
    lease.execute("SELECT 1").await.unwrap();
    lease.rollback_transaction().await.unwrap();
    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(918273648)")
        .fetch_one(&mut *lease)
        .await
        .unwrap();
    assert!(unlocked);
    lease.release_clean();
    let next: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        backend, next,
        "acknowledged cleanup must permit healthy reuse"
    );
    pool.close().await;
    db.cleanup().await.unwrap();
}
