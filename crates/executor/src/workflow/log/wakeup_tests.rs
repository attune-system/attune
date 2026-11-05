mod wakeup_tests {
    use super::*;
    use attune_common::db::ConnectionIdentity;
    use tokio::sync::broadcast;

    async fn subscribe(
        pool: &PgPool,
        owner: Uuid,
    ) -> (
        OutboxWakeups,
        broadcast::Sender<()>,
        broadcast::Receiver<()>,
    ) {
        let mut wakeups = OutboxWakeups::new(pool, owner);
        let (tx, mut rx) = broadcast::channel(1);
        tokio::time::timeout(Duration::from_secs(5), async {
            while wakeups.listener.is_none() {
                assert!(wakeups.wait(false, &mut rx).await);
            }
        })
        .await
        .expect("LISTEN registration");
        (wakeups, tx, rx)
    }

    async fn receive(wakeups: &mut OutboxWakeups, rx: &mut broadcast::Receiver<()>) {
        assert!(
            tokio::time::timeout(Duration::from_secs(3), wakeups.wait(false, rx))
                .await
                .expect("notification must arrive before the 4–5 second fallback")
        );
    }

    async fn listener_pid(pool: &PgPool, owner: Uuid) -> i32 {
        sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE application_name = $1")
            .bind(
                ConnectionIdentity::new("executor", owner.to_string(), "workflow-log-wakeup")
                    .application_name(),
            )
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn listener_gone(pool: &PgPool, owner: Uuid) {
        let name = ConnectionIdentity::new("executor", owner.to_string(), "workflow-log-wakeup")
            .application_name();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1)",
                )
                .bind(&name)
                .fetch_one(pool)
                .await
                .unwrap();
                if !exists {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned listener socket closed");
    }

    #[tokio::test]
    async fn notification_is_commit_bound_and_survives_insert_before_wait() {
        let (database, workflow) = test_workflow().await;
        let pool = database.pool();
        let (mut wakeups, _tx, mut rx) = subscribe(pool, Uuid::new_v4()).await;
        let mut transaction = pool.begin().await.unwrap();
        WorkflowLogOutboxRepository::enqueue_append(&mut transaction, workflow, b"rollback")
            .await
            .unwrap();
        assert!(tokio::time::timeout(
            Duration::from_millis(150),
            wakeups.listener.as_mut().unwrap().try_recv()
        )
        .await
        .is_err());
        transaction.rollback().await.unwrap();
        assert!(matches!(
            WorkflowLogOutboxRepository::claim_next_with_state(pool, Uuid::new_v4(), CLAIM_LEASE)
                .await
                .unwrap(),
            WorkflowLogClaim::Empty
        ));
        enqueue(pool, workflow, b"committed before waiter").await;
        receive(&mut wakeups, &mut rx).await;
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        assert!(
            WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500)
                .dispatch_once()
                .await
                .unwrap()
        );
        assert_eq!(transport.operations(), vec!["append:0"]);
        wakeups.close().await;
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn live_heads_keep_short_polls_through_lease_expiry_and_future_retry() {
        let (database, workflow) = test_workflow().await;
        let pool = database.pool();
        let (mut wakeups, _tx, mut rx) = subscribe(pool, Uuid::new_v4()).await;
        enqueue(pool, workflow, b"lease and retry").await;
        receive(&mut wakeups, &mut rx).await;
        let first =
            WorkflowLogOutboxRepository::claim_next(pool, Uuid::new_v4(), Duration::from_secs(30))
                .await
                .unwrap()
                .unwrap();
        // Claims themselves must not emit a wakeup to all competing executors.
        assert!(tokio::time::timeout(
            Duration::from_millis(100),
            wakeups.listener.as_mut().unwrap().try_recv()
        )
        .await
        .is_err());
        assert!(matches!(
            WorkflowLogOutboxRepository::claim_next_with_state(pool, Uuid::new_v4(), CLAIM_LEASE)
                .await
                .unwrap(),
            WorkflowLogClaim::Pending
        ));
        // Advance the owned fixture past expiry without emitting a hint. This
        // tests the periodic path without racing a short lease against CI load.
        sqlx::query(
            "ALTER TABLE workflow_log_outbox DISABLE TRIGGER workflow_log_outbox_ready_notify",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("UPDATE workflow_log_outbox SET claim_expires_at = clock_timestamp() - interval '1 second' WHERE id=$1")
            .bind(first.id).execute(pool).await.unwrap();
        sqlx::query(
            "ALTER TABLE workflow_log_outbox ENABLE TRIGGER workflow_log_outbox_ready_notify",
        )
        .execute(pool)
        .await
        .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                assert!(wakeups.wait(true, &mut rx).await);
                match WorkflowLogOutboxRepository::claim_next_with_state(
                    pool,
                    Uuid::new_v4(),
                    CLAIM_LEASE,
                )
                .await
                .unwrap()
                {
                    WorkflowLogClaim::Claimed(record) => break record,
                    WorkflowLogClaim::Pending => {}
                    WorkflowLogClaim::Empty => panic!("leased head must remain pending"),
                }
            }
        })
        .await
        .expect("lease expiry without an external wakeup");
        assert_eq!(first.id, second.id);
        assert_eq!(second.attempt_count, 2);
        assert!(WorkflowLogOutboxRepository::release_after_failure(
            pool,
            second.id,
            second.claimed_by,
            Utc::now() + chrono::Duration::hours(1),
            "owned retry"
        )
        .await
        .unwrap());
        receive(&mut wakeups, &mut rx).await;
        assert!(matches!(
            WorkflowLogOutboxRepository::claim_next_with_state(pool, Uuid::new_v4(), CLAIM_LEASE)
                .await
                .unwrap(),
            WorkflowLogClaim::Pending
        ));
        sqlx::query(
            "ALTER TABLE workflow_log_outbox DISABLE TRIGGER workflow_log_outbox_ready_notify",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("UPDATE workflow_log_outbox SET available_at = clock_timestamp() - interval '1 second' WHERE id=$1")
            .bind(second.id).execute(pool).await.unwrap();
        sqlx::query(
            "ALTER TABLE workflow_log_outbox ENABLE TRIGGER workflow_log_outbox_ready_notify",
        )
        .execute(pool)
        .await
        .unwrap();
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        let dispatcher = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match dispatcher.dispatch_next().await.unwrap() {
                    DispatchOutcome::Progress => break,
                    DispatchOutcome::Pending => {
                        assert!(wakeups.wait(true, &mut rx).await);
                    }
                    DispatchOutcome::Empty => panic!("delayed retry must remain pending"),
                }
            }
        })
        .await
        .expect("retry deadline without a later notification");
        assert_eq!(transport.operations(), vec!["append:0"]);
        wakeups.close().await;
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn reconnect_reconciles_work_enqueued_without_a_listener() {
        let (database, workflow) = test_workflow().await;
        let pool = database.pool();
        let owner = Uuid::new_v4();
        let (mut wakeups, _tx, mut rx) = subscribe(pool, owner).await;
        let pid = listener_pid(pool, owner).await;
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT pg_terminate_backend($1)")
                .bind(pid)
                .fetch_one(pool)
                .await
                .unwrap()
        );
        assert!(wakeups.wait(false, &mut rx).await);
        assert!(wakeups.listener.is_none());
        enqueue(pool, workflow, b"lost notification").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while wakeups.listener.is_none() {
                assert!(wakeups.wait(false, &mut rx).await);
            }
        })
        .await
        .expect("reconnect and registration");
        assert_ne!(pid, listener_pid(pool, owner).await);
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        assert!(
            WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500)
                .dispatch_once()
                .await
                .unwrap()
        );
        assert_eq!(transport.operations(), vec!["append:0"]);
        wakeups.close().await;
        listener_gone(pool, owner).await;
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn missing_notification_has_bounded_empty_fallback() {
        let (database, workflow) = test_workflow().await;
        let pool = database.pool();
        let (mut wakeups, _tx, mut rx) = subscribe(pool, Uuid::new_v4()).await;
        // Exclusive fixture: simulate a lost notification without changing work.
        sqlx::query(
            "ALTER TABLE workflow_log_outbox DISABLE TRIGGER workflow_log_outbox_ready_notify",
        )
        .execute(pool)
        .await
        .unwrap();
        enqueue(pool, workflow, b"fallback").await;
        let began = tokio::time::Instant::now();
        assert!(
            tokio::time::timeout(Duration::from_secs(7), wakeups.wait(false, &mut rx))
                .await
                .unwrap()
        );
        assert!(began.elapsed() >= Duration::from_secs(4));
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        assert!(
            WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500)
                .dispatch_once()
                .await
                .unwrap()
        );
        assert_eq!(transport.operations(), vec!["append:0"]);
        wakeups.close().await;
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn failed_heads_are_idle_until_explicitly_requeued() {
        let (database, workflow) = test_workflow().await;
        let pool = database.pool();
        let (mut wakeups, _tx, mut rx) = subscribe(pool, Uuid::new_v4()).await;
        enqueue(pool, workflow, b"failed").await;
        receive(&mut wakeups, &mut rx).await;
        let claim = WorkflowLogOutboxRepository::claim_next(pool, Uuid::new_v4(), CLAIM_LEASE)
            .await
            .unwrap()
            .unwrap();
        assert!(WorkflowLogOutboxRepository::mark_permanently_failed(
            pool,
            claim.id,
            claim.claimed_by,
            "owned failure",
            "write"
        )
        .await
        .unwrap());
        assert!(matches!(
            WorkflowLogOutboxRepository::claim_next_with_state(pool, Uuid::new_v4(), CLAIM_LEASE)
                .await
                .unwrap(),
            WorkflowLogClaim::Empty
        ));
        WorkflowLogOutboxRepository::retry_failed(pool, claim.id)
            .await
            .unwrap();
        receive(&mut wakeups, &mut rx).await;
        assert!(matches!(
            WorkflowLogOutboxRepository::claim_next_with_state(pool, Uuid::new_v4(), CLAIM_LEASE)
                .await
                .unwrap(),
            WorkflowLogClaim::Claimed(_)
        ));
        wakeups.close().await;
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn aborting_wait_closes_the_dedicated_listener() {
        let (database, _) = test_workflow().await;
        let pool = database.pool();
        let owner = Uuid::new_v4();
        let (mut wakeups, _tx, mut rx) = subscribe(pool, owner).await;
        let handle = tokio::spawn(async move {
            wakeups.wait(false, &mut rx).await;
            wakeups.close().await;
        });
        // The listener exists before abort; cancellation must not detach a task.
        listener_pid(pool, owner).await;
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());
        listener_gone(pool, owner).await;
        database.cleanup().await.unwrap();
    }
    #[tokio::test]
    async fn two_idle_dispatchers_wake_and_deliver_in_order_once() {
        let (database, workflow) = test_workflow().await;
        let pool = database.pool();
        // Count empty claim statements without requiring pg_stat_statements.
        // Statement triggers fire even when the fenced UPDATE affects no rows.
        sqlx::raw_sql("CREATE TABLE owned_idle_claim_probe (calls bigint NOT NULL); INSERT INTO owned_idle_claim_probe VALUES (0); CREATE FUNCTION owned_idle_claim_probe() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN UPDATE owned_idle_claim_probe SET calls=calls+1; RETURN NULL; END; $$; CREATE TRIGGER owned_idle_claim_probe AFTER UPDATE ON workflow_log_outbox FOR EACH STATEMENT EXECUTE FUNCTION owned_idle_claim_probe()")
            .execute(pool).await.unwrap();
        let transport = Arc::new(RecordingTransport::new(pool.clone(), 0));
        let first = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        let second = WorkflowLogDispatcher::new(pool.clone(), transport.clone(), 1024, 500);
        let owners = [first.owner, second.owner];
        let (shutdown, rx) = broadcast::channel(1);
        let other_rx = shutdown.subscribe();
        let mut handles = [
            tokio::spawn(first.start(rx)),
            tokio::spawn(second.start(other_rx)),
        ];
        // Bound and retain teardown even if readiness or delivery deadlines expire.
        let result = tokio::time::timeout(Duration::from_secs(40), async {
            for owner in owners {
                let name = ConnectionIdentity::new("executor", owner.to_string(), "workflow-log-wakeup").application_name();
                loop {
                    let ready: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND state='idle' AND query LIKE 'LISTEN%')")
                        .bind(&name).fetch_one(pool).await.unwrap();
                    if ready { break; }
                    tokio::task::yield_now().await;
                }
            }
            let before: i64 = sqlx::query_scalar("SELECT calls FROM owned_idle_claim_probe").fetch_one(pool).await.unwrap();
            assert!(before >= 2, "both dispatchers must have attempted work");
            let began = tokio::time::Instant::now();
            // Fixed observation duration, not a readiness sleep.
            tokio::time::sleep(Duration::from_secs(10)).await;
            let after: i64 = sqlx::query_scalar("SELECT calls FROM owned_idle_claim_probe").fetch_one(pool).await.unwrap();
            let claims = after - before;
            println!("OUTBOX_IDLE_PROBE={}", serde_json::json!({"replicas": 2, "claims": claims, "elapsed_ms": began.elapsed().as_millis()}));
            assert!(claims <= 10, "empty claims must be bounded, observed {claims}");
            let started = transport.commit_started.notified();
            for _ in 0..8 { enqueue(pool, workflow, b"wake both owners").await; }
            assert!(enqueue_seal(pool, workflow).await);
            tokio::time::timeout(Duration::from_secs(3), started).await.expect("prompt first append after commit");
            loop {
                let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow_log_outbox WHERE workflow_execution=$1 AND delivered_at IS NULL")
                    .bind(workflow).fetch_one(pool).await.unwrap();
                if pending == 0 { break; }
                tokio::task::yield_now().await;
            }
        }).await;
        shutdown.send(()).unwrap();
        for handle in &mut handles {
            wait_for_dispatcher_shutdown(handle)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        for owner in owners {
            listener_gone(pool, owner).await;
        }
        let operations = transport.operations();
        database.cleanup().await.unwrap();
        result.expect("idle executors must wake and drain the committed FIFO");
        let mut expected: Vec<_> = (0..8).map(|i| format!("append:{i}")).collect();
        expected.push("seal".to_string());
        assert_eq!(operations, expected);
    }

    #[tokio::test]
    async fn unavailable_listener_does_not_block_polling_or_shutdown() {
        // Own the socket but never speak PostgreSQL: LISTEN setup remains stuck
        // while ordinary reconciliation and shutdown must stay responsive.
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let options = sqlx::postgres::PgConnectOptions::new()
            .host("127.0.0.1")
            .port(socket.local_addr().unwrap().port())
            .username("owned_probe")
            .database("owned_probe");
        let service = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(options);
        let mut wakeups = OutboxWakeups::new(&service, Uuid::new_v4());
        let (tx, mut rx) = broadcast::channel(1);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), wakeups.wait(false, &mut rx))
                .await
                .expect("polling continues while connection setup is stalled")
        );
        assert!(wakeups.listener.is_none());
        // Simulate work resuming before the subscription finishes. The partial
        // attempt must be cancelled instead of remaining unread during delivery.
        wakeups.drain_ready();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), wakeups.wait(false, &mut rx))
                .await
                .unwrap()
        );
        tx.send(()).unwrap();
        assert!(
            !tokio::time::timeout(Duration::from_secs(1), wakeups.wait(false, &mut rx))
                .await
                .unwrap()
        );
        let closed = tokio::time::timeout(Duration::from_secs(3), wakeups.close()).await;
        drop(socket);
        service.close().await;
        closed.expect("cancelled connection setup must release its owned pool");
    }
}
