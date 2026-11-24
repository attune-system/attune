fn completion_test_config(
    database: &TestDatabase,
    scratch: &tempfile::TempDir,
    url: &str,
) -> Result<Config> {
    let mut config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    config.database.url = database.database_url().into();
    config.database.schema = Some(database.schema().into());
    config.database.max_connections = 8;
    config.artifacts_dir = scratch.path().join("artifacts").display().to_string();
    config.packs_base_dir = scratch.path().join("packs").display().to_string();
    config.runtime_envs_dir = scratch.path().join("runtimes").display().to_string();
    config
        .message_queue
        .as_mut()
        .context("test message queue config")?
        .url = url.to_string();
    Ok(config)
}

#[tokio::test]
#[ignore = "requires a disposable database and RabbitMQ vhost"]
async fn test_service_can_stop_while_reconnecting_during_startup() -> Result<()> {
    use tokio::net::{TcpListener, TcpStream};

    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    let database = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let scratch = tempfile::tempdir()?;
    let url = std::env::var("ATTUNE_TEST_RABBITMQ_URL")?;
    let topology = TestMqTopology::new(&Uuid::new_v4().simple().to_string());
    let cleanup = Connection::connect(&url).await?;

    let mut proxy_url: lapin::uri::AMQPUri = url
        .parse()
        .map_err(|_| anyhow!("invalid RabbitMQ fixture URL"))?;
    let backend =
        TcpStream::connect((proxy_url.authority.host.as_str(), proxy_url.authority.port)).await?;
    let socket = Arc::new(TcpListener::bind("127.0.0.1:0").await?);
    proxy_url.authority.host = "127.0.0.1".into();
    proxy_url.authority.port = socket.local_addr()?.port();
    let mut relay = JoinSet::new();
    relay.spawn({
        let socket = socket.clone();
        async move {
            let (mut client, _) = socket.accept().await?;
            let mut backend = backend;
            tokio::io::copy_bidirectional(&mut client, &mut backend).await
        }
    });

    let result: Result<()> = async {
        let config = completion_test_config(&database, &scratch, &proxy_url.to_string())?;
        let service = ExecutorService::new_with_mq_config(
            config,
            topology.config.clone(),
            topology.consumer_tag_prefix.clone(),
        )
        .await?;
        // Cut the established connection. Keep the relay socket bound without
        // accepting so reconnect cannot finish its AMQP handshake.
        relay.shutdown().await;
        timeout(Duration::from_secs(5), async {
            while service.inner.mq_connection.is_healthy().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("observe the disconnected broker")?;

        let mut start = Box::pin(service.start());
        anyhow::ensure!(futures::poll!(start.as_mut()).is_pending());
        service.stop().await?;
        timeout(Duration::from_secs(5), start)
            .await
            .context("cancel startup reconnect")??;
        anyhow::ensure!(
            service.pool().is_closed(),
            "startup shutdown left the database pool open"
        );
        Ok(())
    }
    .await;
    relay.shutdown().await;
    topology.delete_owned(&cleanup).await?;
    cleanup.close().await?;
    database.cleanup().await?;
    result
}

#[tokio::test]
#[ignore = "requires a disposable database and RabbitMQ vhost"]
async fn test_manager_completion_retry_holds_key_in_real_listener() -> Result<()> {
    use attune_common::mq::{ExecutionCompletedPayload, MessageType};
    use attune_common::repositories::manager_assignment::MANAGER_COMPLETION_SOURCE;

    let mut config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    config.database.max_connections = 8;
    let database = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let url = std::env::var("ATTUNE_TEST_RABBITMQ_URL")?;
    let topology = TestMqTopology::new(&Uuid::new_v4().simple().to_string());
    let broker = Connection::connect(&url).await?;
    broker.setup_common_infrastructure(&topology.config).await?;
    broker
        .setup_executor_infrastructure(&topology.config)
        .await?;
    let mut tasks = JoinSet::new();

    let result = timeout(TEST_OPERATION_TIMEOUT, async {
        let pool = database.pool();
        sqlx::raw_sql(
            "CREATE TABLE completion_test_gate (enabled boolean NOT NULL);
             INSERT INTO completion_test_gate VALUES (true);
             CREATE SEQUENCE completion_test_attempts;
             CREATE FUNCTION completion_test_fail() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
                 IF NEW.kind = 'append' AND position('Task ''first''' in convert_from(NEW.payload, 'UTF8')) > 0
                    AND EXISTS (SELECT 1 FROM completion_test_gate WHERE enabled) THEN
                     PERFORM nextval('completion_test_attempts');
                     RAISE serialization_failure USING MESSAGE = 'fixture transient completion failure';
                 END IF;
                 RETURN NEW;
             END; $$;
             CREATE TRIGGER completion_test_fail BEFORE INSERT ON workflow_log_outbox
             FOR EACH ROW EXECUTE FUNCTION completion_test_fail();",
        ).execute(pool).await?;
        let definition: i64 = sqlx::query_scalar(
            "WITH fixture_pack AS (
                 INSERT INTO pack(ref,label,version) VALUES ('completion_test','Completion test','1.0.0') RETURNING id
             ) INSERT INTO workflow_definition(ref,pack,pack_ref,label,version,definition)
             SELECT 'completion_test.flow', id, 'completion_test', 'Fixture', '1.0.0', '{}' FROM fixture_pack
             RETURNING id",
        ).fetch_one(pool).await?;
        let workflows: Vec<(i64,i64)> = sqlx::query_as(
            "WITH parents AS (
                 INSERT INTO execution(action_ref,status)
                 SELECT 'completion_test.flow', 'completed' FROM generate_series(1,2) RETURNING id
             ) INSERT INTO workflow_execution(execution,workflow_def,task_graph,status)
             SELECT id,$1,'{}','completed' FROM parents RETURNING id,execution",
        ).bind(definition).fetch_all(pool).await?;
        let instance = Uuid::new_v4();
        let session: i64 = sqlx::query_scalar(
            "INSERT INTO manager_session(role,pool,name,instance,enrollment_digest,expires_at)
             VALUES ('action','fixture','manager',$1,repeat('a',64),now()+interval '1 hour') RETURNING id",
        ).bind(instance).fetch_one(pool).await?;

        let queue = &topology.config.rabbitmq.queues.execution_completed.name;
        let channel = broker.create_channel().await?;
        let mut events = Vec::new();
        for (name, (workflow, parent)) in [("first", workflows[0]), ("sibling", workflows[0]), ("independent", workflows[1])] {
            let message = Uuid::new_v4();
            let (execution, completed_at): (i64, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
                "WITH child AS (
                     INSERT INTO execution(action_ref,parent,workflow_task,status)
                     VALUES ('completion_test.action',$1,$2,'cancelled') RETURNING id,updated
                 ), assignment AS (
                     INSERT INTO manager_execution_assignment
                       (execution,worker,manager_session,instance,scheduled_at,pack_release,
                        pack_release_digest,deadline,closed_at,final_status)
                     SELECT id,1,$3,$4,updated,1,repeat('b',64),updated,updated,'cancelled' FROM child RETURNING id
                 ) INSERT INTO manager_execution_completion
                       (assignment,message_id,execution,action,action_ref,status,completed_at)
                 SELECT a.id,$5,c.id,1,'completion_test.action','cancelled',c.updated
                 FROM child c CROSS JOIN assignment a RETURNING execution,completed_at",
            ).bind(parent).bind(serde_json::json!({"workflow_execution":workflow,"task_name":name,
                "retry_count":0,"max_retries":0,"timed_out":false}))
                .bind(session).bind(instance).bind(message).fetch_one(pool).await?;
            let mut envelope = MessageEnvelope::new(MessageType::ExecutionCompleted, ExecutionCompletedPayload {
                execution_id: execution, action_id: 1, action_ref: "completion_test.action".into(),
                status: "Cancelled".into(), result: None, completed_at,
            }).with_source(MANAGER_COMPLETION_SOURCE);
            envelope.message_id = message;
            channel.basic_publish("".into(),queue.as_str().into(),Default::default(),&envelope.to_bytes()?,Default::default()).await?.await?;
            events.push(message);
        }

        let consumer = Arc::new(Consumer::new(&broker, attune_common::mq::ConsumerConfig {
            queue: queue.clone(), tag: topology.consumer_tag_prefix.clone(), prefetch_count: 10,
            auto_ack: false, exclusive: false,
        }).await?);
        let publisher = Arc::new(Publisher::new(&broker, attune_common::mq::PublisherConfig {
            exchange: topology.config.rabbitmq.exchanges.executions.name.clone(),
            confirm_publish: true, timeout_secs: 5,
        }).await?);
        let scratch = tempfile::tempdir()?;
        let listener = CompletionListener::new(
            pool.clone(), consumer.clone(), publisher,
            Arc::new(ExecutionQueueManager::with_db_pool(QueueConfig::default(), pool.clone())),
            scratch.path().display().to_string(), None, Arc::new(SchedulerMetadataCaches::new()), 8192, 100,
        );
        tasks.spawn(async move { listener.start(NonZeroUsize::new(2).unwrap()).await });

        let processed = |id| async move {
            loop {
                if let Some(ts) = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
                    "SELECT processed_at FROM manager_execution_completion WHERE message_id=$1",
                ).bind(id).fetch_one(pool).await? { return Ok::<_, sqlx::Error>(ts); }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        while !sqlx::query_scalar::<_, bool>("SELECT is_called FROM completion_test_attempts").fetch_one(pool).await? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        processed(events[2]).await?;
        let premature: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM manager_execution_completion WHERE message_id=ANY($1) AND processed_at IS NOT NULL)",
        ).bind(&events[..2]).fetch_one(pool).await?;
        anyhow::ensure!(!premature, "a same-workflow successor passed the failed completion");

        sqlx::query("UPDATE completion_test_gate SET enabled=false").execute(pool).await?;
        let first = processed(events[0]).await?;
        let sibling = processed(events[1]).await?;
        anyhow::ensure!(first <= sibling, "the sibling's receipt committed before the failed delivery retried");
        consumer.request_stop();
        tasks.join_next().await.context("missing completion listener")???;
        Ok::<_, anyhow::Error>(())
    }).await;
    tasks.shutdown().await;
    topology.delete_owned(&broker).await?;
    broker.close().await?;
    database.cleanup().await?;
    result?
}
