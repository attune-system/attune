//! Owned PostgreSQL protocol-boundary regressions. Requires SESSION_TEST_URL
//! pointing at an exclusively owned disposable PostgreSQL server (SSL disabled).
#[cfg(test)]
mod relay;
#[cfg(test)]
mod tests {
    use sqlx::{postgres::PgPoolOptions, Connection, Executor, PgConnection};
    use std::time::Duration;

    use crate::relay::relay;

    async fn cancelled_begin(custom: bool) {
        let url = std::env::var("SESSION_TEST_URL").expect("owned SESSION_TEST_URL required");
        let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let target = format!("{}:{}", opts.get_host(), opts.get_port());
        let (port, seen, release, task) = relay(target, b'T', 1).await;
        let mut conn = PgConnection::connect_with(&opts.port(port).host("127.0.0.1"))
            .await
            .unwrap();
        {
            let begin = async {
                if custom {
                    conn.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
                        .await
                } else {
                    conn.begin().await
                }
            };
            tokio::pin!(begin);
            tokio::select! {
                _ = seen => (),
                result = &mut begin => panic!("BEGIN completed before gate: {}", result.is_ok()),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("relay deadline"),
            }
            // Drop the future before releasing server acknowledgement.
        }
        release.send(()).unwrap();
        conn.ping().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        let mut observer = PgConnection::connect(&url).await.unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM pg_stat_activity WHERE pid = $1")
            .bind(pid)
            .fetch_one(&mut observer)
            .await
            .unwrap();
        assert_eq!(
            state, "idle",
            "cancelled startup left server transaction open"
        );
        let table = format!("session_probe_{port}");
        observer
            .execute(format!("CREATE TABLE {table} (value integer)").as_str())
            .await
            .unwrap();
        let mut tx = conn.begin().await.unwrap();
        tx.execute(format!("INSERT INTO {table} VALUES (1)").as_str())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut observer)
            .await
            .unwrap();
        observer
            .execute(format!("DROP TABLE {table}").as_str())
            .await
            .unwrap();
        assert_eq!(count, 1, "later commit must be independently visible");
        observer.close().await.unwrap();
        conn.close().await.unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_ordinary_begin() {
        cancelled_begin(false).await;
    }

    #[tokio::test]
    async fn cancelled_custom_begin() {
        cancelled_begin(true).await;
    }

    #[tokio::test]
    async fn raw_transactions_are_quarantined() {
        let url = std::env::var("SESSION_TEST_URL").expect("owned SESSION_TEST_URL required");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        conn.execute("BEGIN").await.unwrap();
        drop(conn);
        let next: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(pid, next, "raw open transaction must be disposed");
        pool.close().await;
    }
    #[tokio::test]
    async fn healthy_nested_transactions_and_ping_remain_usable() {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let mut outer = conn.begin().await.unwrap();
        outer.ping().await.unwrap();
        assert!(outer.begin_with("BEGIN").await.is_err());
        let inner = outer.begin().await.unwrap();
        inner.rollback().await.unwrap();
        outer.commit().await.unwrap();
        drop(conn);
        let next: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pid, next, "healthy sessions should be reused");
        pool.close().await;
    }

    #[tokio::test]
    async fn aborted_raw_transactions_are_quarantined() {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        conn.execute("BEGIN").await.unwrap();
        assert!(conn.execute("SELECT 1/0").await.is_err());
        drop(conn);
        let next: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(pid, next);
        pool.close().await;
    }

    async fn interrupted_operation(operation: &str) {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let (status, occurrence) = if operation == "statement" {
            (b'T', 2)
        } else {
            (b'I', 2)
        };
        // Idle #1 is the authentication acknowledgement, Idle #2 is completion.
        let (port, seen, release, task) = relay(
            format!("{}:{}", opts.get_host(), opts.get_port()),
            status,
            occurrence,
        )
        .await;
        let mut conn = PgConnection::connect_with(&opts.host("127.0.0.1").port(port))
            .await
            .unwrap();
        let mut tx = conn.begin().await.unwrap();
        {
            let future = async move {
                match operation {
                    "statement" => {
                        tx.execute("SELECT 42").await.unwrap();
                    }
                    "commit" => tx.commit().await.unwrap(),
                    "rollback" => tx.rollback().await.unwrap(),
                    _ => unreachable!(),
                }
            };
            tokio::pin!(future);
            tokio::select! {
                _ = seen => (),
                _ = &mut future => panic!("completed before gate"),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("relay deadline"),
            }
        }
        release.send(()).unwrap();
        conn.ping().await.unwrap();
        let mut observer = PgConnection::connect(&url).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM pg_stat_activity WHERE pid=$1")
            .bind(pid)
            .fetch_one(&mut observer)
            .await
            .unwrap();
        assert_eq!(state, "idle");
        conn.close().await.unwrap();
        observer.close().await.unwrap();
        task.await.unwrap();
    }
    #[tokio::test]
    async fn cancelled_statement() {
        interrupted_operation("statement").await;
    }
    #[tokio::test]
    async fn cancelled_commit() {
        interrupted_operation("commit").await;
    }
    #[tokio::test]
    async fn cancelled_rollback() {
        interrupted_operation("rollback").await;
    }

    async fn cancelled_pool_startup(custom: bool) {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let (port, seen, release, task) =
            relay(format!("{}:{}", opts.get_host(), opts.get_port()), b'T', 1).await;
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts.clone().host("127.0.0.1").port(port))
            .await
            .unwrap();
        {
            let future = async {
                if custom {
                    pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
                        .await
                } else {
                    pool.begin().await
                }
            };
            tokio::pin!(future);
            tokio::select! {
                _ = seen => (),
                result = &mut future => panic!("completed before gate: {}", result.is_ok()),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("relay deadline"),
            }
        }
        // Replacements may connect directly; the original session remains gated.
        pool.set_connect_options(opts);
        release.send(()).unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let mut observer = PgConnection::connect(&url).await.unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM pg_stat_activity WHERE pid=$1")
            .bind(pid)
            .fetch_one(&mut observer)
            .await
            .unwrap();
        assert_eq!(state, "idle");
        observer.close().await.unwrap();
        conn.close().await.unwrap();
        pool.close().await;
        task.await.unwrap();
    }
    #[tokio::test]
    async fn pool_owned_cancelled_begin() {
        cancelled_pool_startup(false).await;
    }
    #[tokio::test]
    async fn pool_owned_cancelled_custom_begin() {
        cancelled_pool_startup(true).await;
    }
    #[tokio::test]
    async fn cancelled_nested_startup_preserves_caller_boundary() {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let (port, seen, release, task) =
            relay(format!("{}:{}", opts.get_host(), opts.get_port()), b'T', 2).await;
        let mut conn = PgConnection::connect_with(&opts.host("127.0.0.1").port(port))
            .await
            .unwrap();
        let mut outer = conn.begin().await.unwrap();
        {
            let begin = outer.begin();
            tokio::pin!(begin);
            tokio::select! {
                _ = seen => (),
                result = &mut begin => panic!("completed before gate: {}", result.is_ok()),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("relay deadline"),
            }
        }
        release.send(()).unwrap();
        outer.ping().await.unwrap();
        // Caller still owns a real transaction, not an accidentally rolled-back
        // boundary. SET LOCAL can only take effect within that boundary.
        outer
            .execute("SET LOCAL application_name = 'nested-caller'")
            .await
            .unwrap();
        let name: String = sqlx::query_scalar("SHOW application_name")
            .fetch_one(&mut *outer)
            .await
            .unwrap();
        assert_eq!(name, "nested-caller");
        outer.rollback().await.unwrap();
        assert!(
            conn.prepare_for_pool().await.is_err(),
            "uncertain nested ownership stays quarantined"
        );
        conn.close().await.unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn failed_custom_startup_cleans_before_next_use() {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let mut conn = PgConnection::connect(&url).await.unwrap();
        assert!(conn.begin_with("BEGIN; SELECT 1/0").await.is_err());
        conn.ping().await.unwrap();
        conn.prepare_for_pool().await.unwrap();
        conn.close().await.unwrap();
    }

    #[tokio::test]
    async fn release_hook_cannot_hide_an_open_transaction() {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .after_release(|conn, _| {
                Box::pin(async move {
                    conn.execute("BEGIN").await?;
                    Ok(true)
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let mut next = pool.acquire().await.unwrap();
        let next_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *next)
            .await
            .unwrap();
        assert_ne!(pid, next_pid);
        next.close().await.unwrap();
        pool.close().await;
    }
    #[tokio::test]
    async fn interrupted_pool_cleanup_disposes_owned_socket() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(tokio::sync::Notify::new());
        let a = armed.clone();
        let o = observed.clone();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .after_release(move |_, _| {
                let a = a.clone();
                let o = o.clone();
                Box::pin(async move {
                    if a.swap(false, Ordering::SeqCst) {
                        o.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Ok(true)
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        conn.execute("BEGIN").await.unwrap();
        armed.store(true, Ordering::SeqCst);
        {
            let cleanup = conn.return_to_pool();
            tokio::pin!(cleanup);
            tokio::select! {
                _ = observed.notified() => (),
                _ = &mut cleanup => panic!("cleanup should remain suspended"),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("cleanup deadline"),
            }
        }
        let next: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(pid, next);
        drop(conn);
        pool.close().await;
    }
    #[tokio::test]
    async fn failed_nested_startup_does_not_consume_caller_depth() {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let mut conn = PgConnection::connect(&url).await.unwrap();
        let mut outer = conn.begin().await.unwrap();
        assert!(outer.execute("SELECT 1/0").await.is_err());
        assert!(outer.begin().await.is_err());
        assert!(outer.is_in_transaction());
        outer.rollback().await.unwrap();
        assert!(conn.prepare_for_pool().await.is_err());
        conn.close().await.unwrap();
    }
    async fn cancelled_nested_completion(commit: bool) {
        let url = std::env::var("SESSION_TEST_URL").unwrap();
        let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let (port, seen, release, task) =
            relay(format!("{}:{}", opts.get_host(), opts.get_port()), b'T', 3).await;
        let mut conn = PgConnection::connect_with(&opts.host("127.0.0.1").port(port))
            .await
            .unwrap();
        let mut outer = conn.begin().await.unwrap();
        let inner = outer.begin().await.unwrap();
        {
            let finish = async move {
                if commit {
                    inner.commit().await
                } else {
                    inner.rollback().await
                }
            };
            tokio::pin!(finish);
            tokio::select! {
                _ = seen => (),
                result = &mut finish => panic!("completed before gate: {}", result.is_ok()),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("relay deadline"),
            }
        }
        release.send(()).unwrap();
        outer.ping().await.unwrap();
        outer
            .execute("SET LOCAL application_name='outer-survived'")
            .await
            .unwrap();
        let name: String = sqlx::query_scalar("SHOW application_name")
            .fetch_one(&mut *outer)
            .await
            .unwrap();
        assert_eq!(name, "outer-survived");
        outer.rollback().await.unwrap();
        assert!(conn.prepare_for_pool().await.is_err());
        conn.close().await.unwrap();
        task.await.unwrap();
    }
    #[tokio::test]
    async fn cancelled_nested_commit() {
        cancelled_nested_completion(true).await;
    }
    #[tokio::test]
    async fn cancelled_nested_rollback() {
        cancelled_nested_completion(false).await;
    }
    async fn untracked_caller_boundary(custom: bool) {
        let url = std::env::var("SESSION_TEST_URL").expect("owned SESSION_TEST_URL required");
        let mut conn = PgConnection::connect(&url).await.unwrap();
        let mut observer = PgConnection::connect(&url).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        let table = format!("untracked_caller_{pid}");
        observer
            .execute(format!("CREATE TABLE {table}(value integer)").as_str())
            .await
            .unwrap();
        conn.execute(format!("BEGIN; INSERT INTO {table} VALUES(1)").as_str())
            .await
            .unwrap();
        {
            let denied = if custom {
                conn.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
                    .await
            } else {
                conn.begin().await
            };
            assert!(
                denied.is_err(),
                "tracked BEGIN must not adopt a caller's raw boundary"
            );
        }
        let state: String = sqlx::query_scalar("SELECT state FROM pg_stat_activity WHERE pid=$1")
            .bind(pid)
            .fetch_one(&mut observer)
            .await
            .unwrap();
        assert_eq!(
            state, "idle in transaction",
            "caller must retain its original boundary"
        );
        conn.execute("COMMIT").await.unwrap();
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut observer)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "rejected nested startup must not roll back caller writes"
        );
        observer
            .execute(format!("DROP TABLE {table}").as_str())
            .await
            .unwrap();
        conn.close().await.unwrap();
        observer.close().await.unwrap();
    }

    #[tokio::test]
    async fn ordinary_begin_preserves_untracked_caller_boundary() {
        untracked_caller_boundary(false).await;
    }
    #[tokio::test]
    async fn custom_begin_preserves_untracked_caller_boundary() {
        untracked_caller_boundary(true).await;
    }
}
