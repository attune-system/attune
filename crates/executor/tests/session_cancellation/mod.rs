// Real checkpoint owners, not a replacement transaction/publication loop.
// Each relay accepts one owned connection and gates a server acknowledgement.
mod checkpoint_cancellation {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        sync::oneshot,
    };

    #[derive(Debug)]
    enum Gate {
        Lock,
        Publication,
    }
    type Relay = (
        u16,
        oneshot::Receiver<()>,
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    );

    async fn relay(target: String, gate: Gate) -> Relay {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen_tx, seen) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server = TcpStream::connect(target).await.unwrap();
            // This relay forwards individual protocol frames. Nagle buffering
            // combined with delayed ACKs otherwise adds latency to each small
            // exchange and can consume the source-preparation deadline.
            client.set_nodelay(true).unwrap();
            server.set_nodelay(true).unwrap();
            let (mut cr, mut cw) = client.split();
            let (mut sr, mut sw) = server.split();
            let armed = Arc::new(AtomicBool::new(false));
            let up_armed = armed.clone();
            let is_pg = matches!(gate, Gate::Lock);
            let up = async {
                if !is_pg {
                    let _ = tokio::io::copy(&mut cr, &mut sw).await;
                    return;
                }
                let len = cr.read_u32().await.unwrap();
                let mut startup = vec![0; len as usize - 4];
                cr.read_exact(&mut startup).await.unwrap();
                sw.write_all(&len.to_be_bytes()).await.unwrap();
                sw.write_all(&startup).await.unwrap();
                while let Ok(kind) = cr.read_u8().await {
                    let Ok(len) = cr.read_u32().await else { break };
                    let mut body = vec![0; len as usize - 4];
                    if cr.read_exact(&mut body).await.is_err() {
                        break;
                    }
                    if matches!(kind, b'Q' | b'P')
                        && body
                            .windows(b"pg_advisory_lock".len())
                            .any(|part| part == b"pg_advisory_lock")
                    {
                        up_armed.store(true, Ordering::SeqCst);
                    }
                    let mut frame = vec![kind];
                    frame.extend_from_slice(&len.to_be_bytes());
                    frame.extend_from_slice(&body);
                    if sw.write_all(&frame).await.is_err() {
                        break;
                    }
                }
            };
            let down = async {
                let mut gate = Some((seen_tx, release_rx));
                let mut command_complete = false;
                loop {
                    let mut header = vec![0; if is_pg { 5 } else { 7 }];
                    if sr.read_exact(&mut header).await.is_err() {
                        break;
                    }
                    let len =
                        u32::from_be_bytes(header[header.len() - 4..].try_into().unwrap()) as usize;
                    let mut body = vec![0; if is_pg { len - 4 } else { len + 1 }];
                    if sr.read_exact(&mut body).await.is_err() {
                        break;
                    }
                    if is_pg && header[0] == b'C' && armed.load(Ordering::SeqCst) {
                        command_complete = true;
                    }
                    let matches = if is_pg {
                        header[0] == b'Z' && command_complete
                    } else {
                        header[0] == 1 && body.starts_with(&[0, 60, 0, 80]) // Basic.Ack publisher confirmation
                    };
                    if matches {
                        if let Some((seen, release)) = gate.take() {
                            let _ = seen.send(());
                            let _ = release.await;
                        }
                    }
                    header.extend_from_slice(&body);
                    if cw.write_all(&header).await.is_err() {
                        break;
                    }
                }
            };
            tokio::select! { _ = up => (), _ = down => () }
        });
        (port, seen, release, task)
    }

    #[derive(Debug, Clone, Copy)]
    enum Path {
        Cache,
        InitialWait,
        Wait,
    }
    async fn assert_cancelled(path: Path, publication: bool) {
        let cache = if matches!(path, Path::Cache) {
            Some(CachePartitionFixture::create().await)
        } else {
            None
        };
        let inquiry = if cache.is_none() {
            Some(InquirySchedulerFixture::create().await)
        } else {
            None
        };
        let database = cache
            .as_ref()
            .map(|f| &f.database)
            .unwrap_or_else(|| &inquiry.as_ref().unwrap().database);
        let observer = database.pool();
        if let Some(fixture) = &cache {
            seed_cache(fixture).await;
        }
        let wait = if matches!(path, Path::Wait) {
            let fixture = inquiry.as_ref().unwrap();
            fixture.activate_guarded().await;
            Some(fixture.wait("guarded").await)
        } else {
            None
        };
        let options = observer.connect_options();
        let base = std::env::var("ATTUNE_TEST_CACHE_BROKER_URL").unwrap();
        let (port, mut seen, release, relay_task) = if publication {
            relay(
                base.rsplit_once('@').unwrap().1.to_owned(),
                Gate::Publication,
            )
            .await
        } else {
            relay(
                format!("{}:{}", options.get_host(), options.get_port()),
                Gate::Lock,
            )
            .await
        };
        let broker_base = if publication {
            format!("{}@127.0.0.1:{port}", base.rsplit_once('@').unwrap().0)
        } else {
            base
        };
        let broker = CacheBrokerFixture::try_connect_at(&broker_base, "cache-cancellation")
            .await
            .unwrap();
        let options = if publication {
            options.as_ref().clone()
        } else {
            options
                .as_ref()
                .clone()
                .host("127.0.0.1")
                .port(port)
                .ssl_mode(sqlx::postgres::PgSslMode::Disable)
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
        // A waiting inquiry emits no child. Resolve its real source before the
        // publication gate so the production checkpoint publishes a child.
        if matches!(path, Path::Wait) || publication && matches!(path, Path::InitialWait) {
            let fixture = inquiry.as_ref().unwrap();
            InquiryRepository::respond_pending(
                observer,
                fixture.inquiry.id,
                serde_json::json!({"approved":true}),
                fixture.parent.executor.unwrap(),
                None,
            )
            .await
            .unwrap()
            .expect("pending source inquiry");
        }
        let result = {
            let work = async {
                match path {
                    Path::Cache => {
                        let fixture = cache.as_ref().unwrap();
                        ExecutionScheduler::checkpoint_native_cache(
                            &pool,
                            &broker.publisher,
                            &fixture.parent,
                            "consume",
                            Some(CACHE_PARTITION_INPUT_KEY),
                            None,
                        )
                        .await
                    }
                    Path::InitialWait => {
                        let fixture = inquiry.as_ref().unwrap();
                        ExecutionScheduler::checkpoint_initial_native_wait(
                            &pool,
                            &broker.publisher,
                            &fixture.parent,
                            fixture.workflow_execution_id,
                            "guarded",
                            Some(INQUIRY_SCHEDULER_INPUT_KEY),
                            Some("request"),
                        )
                        .await
                    }
                    Path::Wait => {
                        ExecutionScheduler::checkpoint_native_wait(
                            &pool,
                            &broker.publisher,
                            wait.as_ref().unwrap(),
                            Some(INQUIRY_SCHEDULER_INPUT_KEY),
                        )
                        .await
                    }
                }
            };
            tokio::pin!(work);
            tokio::select! {
                gate = &mut seen => {
                    if gate.is_err() {
                        Err("acknowledgement relay ended".to_owned())
                    } else {
                        let locked: bool = sqlx::query_scalar(
                            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted)")
                            .bind(backend).fetch_one(observer).await.unwrap();
                        let idle: bool = sqlx::query_scalar(
                            "SELECT xact_start IS NULL FROM pg_stat_activity WHERE pid=$1")
                            .bind(backend).fetch_one(observer).await.unwrap();
                        if !locked || idle != publication {
                            Err(format!("checkpoint boundary did not retain its expected lock/transaction: locked={locked}, idle={idle}"))
                        } else { Ok(()) }
                    }
                },
                result = &mut work => Err(format!("checkpoint finished before cancellation gate: {result:?}")),
                _ = tokio::time::sleep(Duration::from_secs(30)) => Err("checkpoint gate deadline".to_owned()),
            }
        }; // cancellation drops the actual checkpoint's SessionLease
        let _ = release.send(());
        let gone = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let alive: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1)",
                )
                .bind(backend)
                .fetch_one(observer)
                .await
                .unwrap();
                if !alive {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        pool.close().await;
        broker.close().await;
        tokio::time::timeout(Duration::from_secs(10), relay_task)
            .await
            .unwrap()
            .unwrap();
        if let Some(fixture) = cache {
            fixture.database.cleanup().await.unwrap();
        }
        if let Some(fixture) = inquiry {
            fixture.database.cleanup().await.unwrap();
        }
        result.unwrap_or_else(|error| {
            panic!("{path:?} checkpoint at publication={publication} must reach its gate: {error}")
        });
        gone.expect("cancelled checkpoint backend and its session locks must terminate");
    }

    #[tokio::test]
    #[ignore = "requires the owned RabbitMQ cache protocol fixture"]
    async fn checkpoint_cache_cancellation_at_lock_ack_and_publication() {
        assert_cancelled(Path::Cache, false).await;
        assert_cancelled(Path::Cache, true).await;
    }
    #[tokio::test]
    #[ignore = "requires the owned RabbitMQ cache protocol fixture"]
    async fn checkpoint_initial_wait_cancellation_at_lock_ack_and_publication() {
        assert_cancelled(Path::InitialWait, false).await;
        assert_cancelled(Path::InitialWait, true).await;
    }
    #[tokio::test]
    #[ignore = "requires the owned RabbitMQ cache protocol fixture"]
    async fn checkpoint_wait_cancellation_at_lock_ack_and_publication() {
        assert_cancelled(Path::Wait, false).await;
        assert_cancelled(Path::Wait, true).await;
    }
}
