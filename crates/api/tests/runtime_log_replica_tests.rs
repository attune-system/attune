mod helpers;

use std::{
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use attune_api::{postgres_listener, AppState, Server};
use attune_common::{
    auth::jwt::{generate_execution_token, generate_worker_token, JwtConfig},
    blob_store::{
        BlobBody, BlobReader, BlobStore, BlobStoreError, ByteRange, FilesystemBlobStore, ObjectKey,
        ProviderVersion, StoredObject,
    },
    config::{BlobStorageConfig, Config},
    models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, ExecutionStatus,
        LogStreamBackend, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        log_stream::LogStreamRepository,
        storage_maintenance::StorageMaintenanceRepository,
        Create,
    },
    test_database::TestDatabase,
};
use eventsource_stream::{Event, Eventsource};
use futures::{Stream, StreamExt};
use helpers::init_test_env;
use serde::Serialize;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct StoreCounts {
    puts: AtomicU64,
    gets: AtomicU64,
    heads: AtomicU64,
    deletes: AtomicU64,
    delay_ms: AtomicU64,
}

struct CountingBlobStore {
    inner: FilesystemBlobStore,
    counts: Arc<StoreCounts>,
}

impl fmt::Debug for CountingBlobStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CountingBlobStore").finish()
    }
}

impl CountingBlobStore {
    fn new(root: &std::path::Path, counts: Arc<StoreCounts>) -> Result<Self> {
        Ok(Self {
            inner: FilesystemBlobStore::new(root)?,
            counts,
        })
    }

    async fn delay(&self) {
        let delay = self.counts.delay_ms.load(Ordering::Relaxed);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    }
}

#[async_trait]
impl BlobStore for CountingBlobStore {
    async fn preflight(&self) -> std::result::Result<(), BlobStoreError> {
        self.inner.preflight().await
    }

    async fn put(
        &self,
        key: &ObjectKey,
        body: BlobBody,
        expected_sha256: [u8; 32],
    ) -> std::result::Result<StoredObject, BlobStoreError> {
        self.counts.puts.fetch_add(1, Ordering::Relaxed);
        self.delay().await;
        self.inner.put(key, body, expected_sha256).await
    }

    async fn get(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        range: Option<ByteRange>,
    ) -> std::result::Result<BlobReader, BlobStoreError> {
        self.counts.gets.fetch_add(1, Ordering::Relaxed);
        self.delay().await;
        self.inner.get(key, version, range).await
    }

    async fn get_pinned(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        object_size: u64,
        object_sha256: [u8; 32],
        range: Option<ByteRange>,
    ) -> std::result::Result<BlobReader, BlobStoreError> {
        self.counts.gets.fetch_add(1, Ordering::Relaxed);
        self.delay().await;
        self.inner
            .get_pinned(key, version, object_size, object_sha256, range)
            .await
    }

    async fn head(
        &self,
        key: &ObjectKey,
    ) -> std::result::Result<Option<StoredObject>, BlobStoreError> {
        self.counts.heads.fetch_add(1, Ordering::Relaxed);
        self.inner.head(key).await
    }

    async fn delete(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
    ) -> std::result::Result<(), BlobStoreError> {
        self.counts.deletes.fetch_add(1, Ordering::Relaxed);
        self.inner.delete(key, version).await
    }
}

struct Replica {
    url: String,
    state: Arc<AppState>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
    notifications: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Replica {
    async fn stop(mut self) -> Result<()> {
        self.state.execution_log_streams.begin_shutdown();
        tokio::time::timeout(Duration::from_secs(2), &mut self.server).await???;
        if let Some(task) = self.notifications.take() {
            task.abort();
        }
        Ok(())
    }
}

struct Harness {
    database: TestDatabase,
    replicas: Vec<Replica>,
    counts: Arc<StoreCounts>,
    _root: tempfile::TempDir,
    worker_token: String,
}

impl Harness {
    async fn start(notification_listeners: &[bool]) -> Result<Self> {
        init_test_env();
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path)?;
        let database = TestDatabase::create(&config.database).await?;
        config.database.schema = Some(database.schema().to_string());
        config.server.shutdown_grace_period = 1;
        config.server.execution_log_stream_global_limit = 100;
        config.server.execution_log_stream_per_identity_limit = 100;
        let root = tempfile::tempdir()?;
        config.artifacts_dir = root.path().join("volume").to_string_lossy().into_owned();
        config.storage = BlobStorageConfig::Filesystem {
            root: root.path().join("unused-config-store"),
        };
        let counts = Arc::new(StoreCounts::default());
        let store: Arc<dyn BlobStore> = Arc::new(CountingBlobStore::new(
            &root.path().join("objects"),
            counts.clone(),
        )?);
        let jwt = JwtConfig {
            secret: config.security.jwt_secret.clone().unwrap(),
            access_token_expiration: 300,
            refresh_token_expiration: 3600,
        };
        let worker_token = generate_worker_token(1, "replica-test", &jwt, None)?;
        let mut replicas = Vec::new();
        for listen_for_notifications in notification_listeners {
            let state = Arc::new(AppState::new_with_audit_and_blob_store(
                database.pool().clone(),
                config.clone(),
                attune_common::audit::AuditEmitter::noop(),
                store.clone(),
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let server = tokio::spawn(Server::new(state.clone()).run_with_listener(listener));
            let notifications = listen_for_notifications.then(|| {
                tokio::spawn(postgres_listener::start_postgres_listener(
                    database.pool().clone(),
                    state.broadcast_tx.clone(),
                    state.log_stream_wakeups.clone(),
                ))
            });
            replicas.push(Replica {
                url: format!("http://{address}"),
                state,
                server,
                notifications,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(Self {
            database,
            replicas,
            counts,
            _root: root,
            worker_token,
        })
    }

    async fn fixture(&self, backend: LogStreamBackend) -> Result<LogFixture> {
        let action_ref = format!("replica_{}.run", uuid::Uuid::new_v4().simple());
        let execution = ExecutionRepository::create(
            self.database.pool(),
            CreateExecutionInput {
                action: None,
                action_ref: action_ref.clone(),
                config: None,
                env_vars: None,
                parent: None,
                enforcement: None,
                executor: None,
                permission_set_refs: Vec::new(),
                artifact_retention_policy: None,
                artifact_retention_limit: None,
                worker_selector: None,
                worker_tolerations: None,
                worker_affinity: None,
                worker: None,
                status: ExecutionStatus::Running,
                trace_tag: None,
                result: None,
                workflow_task: None,
                timeout_seconds: None,
            },
        )
        .await?;
        let artifact_ref = format!("{action_ref}.stdout.log");
        let artifact = ArtifactRepository::create(
            self.database.pool(),
            CreateArtifactInput {
                r#ref: artifact_ref.clone(),
                scope: OwnerType::Action,
                owner: action_ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: Some("text/plain".to_string()),
                data: None,
            },
        )
        .await?;
        let version = ArtifactVersionRepository::create_log_pending(
            self.database.pool(),
            artifact.id,
            &artifact_ref,
            backend,
            "text/plain".to_string(),
            Some(execution.id),
            None,
            Some("replica-test".to_string()),
        )
        .await?;
        let stream = LogStreamRepository::create_with_backend(
            self.database.pool(),
            version.id,
            backend,
            64 * 1024,
            100,
        )
        .await?;
        let jwt = JwtConfig {
            secret: "test-secret-for-testing-only-not-secure".to_string(),
            access_token_expiration: 300,
            refresh_token_expiration: 3600,
        };
        let execution_token = generate_execution_token(1, execution.id, &action_ref, &jwt, None)?;
        Ok(LogFixture {
            execution_id: execution.id,
            artifact_id: artifact.id,
            version_id: version.id,
            stream_id: stream.id,
            file_path: version.file_path.unwrap(),
            execution_token,
        })
    }

    async fn stop(mut self) -> Result<()> {
        for replica in self.replicas.drain(..) {
            replica.stop().await?;
        }
        self.database.cleanup().await?;
        Ok(())
    }
}

struct LogFixture {
    execution_id: i64,
    artifact_id: i64,
    version_id: i64,
    stream_id: i64,
    file_path: String,
    execution_token: String,
}

async fn open_log_stream(
    client: &reqwest::Client,
    replica: &Replica,
    fixture: &LogFixture,
    offset: Option<u64>,
) -> Result<
    impl Stream<Item = std::result::Result<Event, eventsource_stream::EventStreamError<reqwest::Error>>>,
> {
    let mut request = client
        .get(format!(
            "{}/api/v1/executions/{}/logs/stdout/stream",
            replica.url, fixture.execution_id
        ))
        .bearer_auth(&fixture.execution_token);
    if let Some(offset) = offset {
        request = request.header("Last-Event-ID", offset);
    }
    let response = request.send().await?.error_for_status()?;
    Ok(response.bytes_stream().eventsource())
}

async fn next_event<S, E>(stream: &mut S, name: &str, wait: Duration) -> Result<Event>
where
    S: Stream<Item = std::result::Result<Event, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::time::timeout(wait, async {
        loop {
            let event = stream.next().await.ok_or("SSE stream ended")??;
            if event.event == name {
                return Ok(event);
            }
        }
    })
    .await?
}

async fn next_data_event<S, E>(stream: &mut S, wait: Duration) -> Result<Event>
where
    S: Stream<Item = std::result::Result<Event, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::time::timeout(wait, async {
        loop {
            let event = stream.next().await.ok_or("SSE stream ended")??;
            if matches!(event.event.as_str(), "content" | "append") {
                return Ok(event);
            }
        }
    })
    .await?
}

async fn put_segment(
    client: &reqwest::Client,
    replica: &Replica,
    fixture: &LogFixture,
    sequence: i64,
    bytes: &'static [u8],
    worker_token: &str,
) -> Result<reqwest::StatusCode> {
    Ok(client
        .put(format!(
            "{}/api/v1/internal/logs/{}/segments/{sequence}",
            replica.url, fixture.version_id
        ))
        .bearer_auth(worker_token)
        .body(bytes)
        .send()
        .await?
        .status())
}

async fn seal(
    client: &reqwest::Client,
    replica: &Replica,
    fixture: &LogFixture,
    truncated: bool,
    worker_token: &str,
) -> Result<reqwest::StatusCode> {
    Ok(client
        .post(format!(
            "{}/api/v1/internal/logs/{}/seal?truncated={truncated}",
            replica.url, fixture.version_id
        ))
        .bearer_auth(worker_token)
        .send()
        .await?
        .status())
}

fn metric(metrics: &str, name: &str) -> u64 {
    metrics
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn object_upload_and_reconnect_cross_api_replicas() -> Result<()> {
    let harness = Harness::start(&[true, true]).await?;
    let fixture = harness.fixture(LogStreamBackend::ObjectSegments).await?;
    let client = reqwest::Client::new();
    let mut reader =
        Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, None).await?);

    assert_eq!(
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            0,
            b"first",
            &harness.worker_token
        )
        .await?,
        reqwest::StatusCode::CREATED
    );
    let first = next_data_event(&mut reader, Duration::from_secs(3)).await?;
    assert_eq!(first.data, "first");
    assert_eq!(first.id, "5");
    drop(reader);

    assert_eq!(
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            1,
            b" second",
            &harness.worker_token
        )
        .await?,
        reqwest::StatusCode::CREATED
    );
    assert_eq!(
        seal(
            &client,
            &harness.replicas[0],
            &fixture,
            false,
            &harness.worker_token
        )
        .await?,
        reqwest::StatusCode::OK
    );
    let mut resumed =
        Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, Some(5)).await?);
    assert_eq!(
        next_event(&mut resumed, "content", Duration::from_secs(3))
            .await?
            .data,
        " second"
    );
    next_event(&mut resumed, "done", Duration::from_secs(3)).await?;
    let metrics = client
        .get(format!("{}/metrics", harness.replicas[1].url))
        .send()
        .await?
        .text()
        .await?;
    assert!(
        metric(
            &metrics,
            "attune_execution_log_stream_object_store_reads_total"
        ) >= 2
    );
    harness.stop().await
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn object_delay_duplicate_requests_and_terminal_race_are_idempotent() -> Result<()> {
    let harness = Harness::start(&[true, true]).await?;
    harness.counts.delay_ms.store(75, Ordering::Relaxed);
    let fixture = harness.fixture(LogStreamBackend::ObjectSegments).await?;
    let client = reqwest::Client::new();
    let started = Instant::now();
    let (left, right) = tokio::join!(
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            0,
            b"duplicate",
            &harness.worker_token
        ),
        put_segment(
            &client,
            &harness.replicas[1],
            &fixture,
            0,
            b"duplicate",
            &harness.worker_token
        ),
    );
    let statuses = [left?, right?];
    assert!(started.elapsed() >= Duration::from_millis(75));
    assert!(statuses.iter().all(reqwest::StatusCode::is_success));
    assert_eq!(
        LogStreamRepository::segments(harness.database.pool(), fixture.stream_id)
            .await?
            .len(),
        1
    );

    let mut reader =
        Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, None).await?);
    let terminal = sqlx::query("UPDATE execution SET status = 'completed' WHERE id = $1")
        .bind(fixture.execution_id)
        .execute(harness.database.pool());
    let final_segment = put_segment(
        &client,
        &harness.replicas[0],
        &fixture,
        1,
        b" terminal",
        &harness.worker_token,
    );
    let (terminal, status) = tokio::join!(terminal, final_segment);
    terminal?;
    assert_eq!(status?, reqwest::StatusCode::CREATED);
    assert_eq!(
        seal(
            &client,
            &harness.replicas[0],
            &fixture,
            false,
            &harness.worker_token
        )
        .await?,
        reqwest::StatusCode::OK
    );
    let mut content = String::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(4), reader.next())
            .await?
            .ok_or("stream ended")??;
        match event.event.as_str() {
            "content" | "append" => content.push_str(&event.data),
            "done" => break,
            "error" => return Err(format!("terminal race returned {}", event.data).into()),
            _ => {}
        }
    }
    assert_eq!(content, "duplicate terminal");
    harness.stop().await
}

#[tokio::test]
#[ignore = "integration test - requires database; waits for reconciliation"]
async fn object_reader_recovers_when_replica_misses_notifications() -> Result<()> {
    let harness = Harness::start(&[true, false]).await?;
    let fixture = harness.fixture(LogStreamBackend::ObjectSegments).await?;
    let client = reqwest::Client::new();
    let mut reader =
        Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, None).await?);
    put_segment(
        &client,
        &harness.replicas[0],
        &fixture,
        0,
        b"reconciled",
        &harness.worker_token,
    )
    .await?;
    seal(
        &client,
        &harness.replicas[0],
        &fixture,
        false,
        &harness.worker_token,
    )
    .await?;

    assert_eq!(
        next_data_event(&mut reader, Duration::from_secs(18))
            .await?
            .data,
        "reconciled"
    );
    next_event(&mut reader, "done", Duration::from_secs(2)).await?;
    harness.stop().await
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "integration test - requires database"]
async fn shared_volume_cross_replica_truncation_writer_loss_and_retention() -> Result<()> {
    use attune_common::artifact_transport::{ArtifactFileTransport, VolumeTransport};

    let harness = Harness::start(&[true, true]).await?;
    let fixture = harness.fixture(LogStreamBackend::SharedFile).await?;
    let writer = VolumeTransport::new(&harness.replicas[0].state.config.artifacts_dir);
    let other_pod = VolumeTransport::new(&harness.replicas[1].state.config.artifacts_dir);
    writer
        .append_log_file(&fixture.file_path, b"shared bytes")
        .await?;
    let mut direct = other_pod.open_reader(&fixture.file_path, 0).await?;
    let mut visible = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut direct, &mut visible).await?;
    assert_eq!(visible, "shared bytes");

    let client = reqwest::Client::new();
    let mut reader =
        Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, None).await?);
    assert_eq!(
        next_data_event(&mut reader, Duration::from_secs(2))
            .await?
            .data,
        "shared bytes"
    );
    assert_eq!(
        seal(
            &client,
            &harness.replicas[1],
            &fixture,
            true,
            &harness.worker_token
        )
        .await?,
        reqwest::StatusCode::OK
    );
    next_event(&mut reader, "done", Duration::from_secs(2)).await?;
    let (truncated, sealed): (bool, bool) =
        sqlx::query_as("SELECT truncated, sealed FROM log_stream WHERE id = $1")
            .bind(fixture.stream_id)
            .fetch_one(harness.database.pool())
            .await?;
    assert!(truncated && sealed);

    let abandoned = harness.fixture(LogStreamBackend::SharedFile).await?;
    writer
        .append_log_file(&abandoned.file_path, b"unsealed")
        .await?;
    sqlx::query("UPDATE execution SET status = 'abandoned' WHERE id = $1")
        .bind(abandoned.execution_id)
        .execute(harness.database.pool())
        .await?;
    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(abandoned.version_id)
    .execute(harness.database.pool())
    .await?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
    sqlx::query("UPDATE execution SET status = 'completed' WHERE id = $1")
        .bind(fixture.execution_id)
        .execute(harness.database.pool())
        .await?;
    sqlx::query(
        "UPDATE artifact_version SET body_updated = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(fixture.version_id)
    .execute(harness.database.pool())
    .await?;
    let candidates = StorageMaintenanceRepository::abandoned_shared_log_pending(
        harness.database.pool(),
        cutoff,
        10,
    )
    .await?;
    assert!(!candidates
        .iter()
        .any(|candidate| candidate.id == fixture.version_id));
    assert!(candidates
        .iter()
        .any(|candidate| candidate.id == abandoned.version_id));
    assert!(
        StorageMaintenanceRepository::claim_abandoned_shared_log_pending(
            harness.database.pool(),
            abandoned.version_id,
            cutoff,
        )
        .await?
    );
    assert!(
        other_pod
            .delete_abandoned_log_file(&abandoned.file_path)
            .await?
    );
    assert!(
        StorageMaintenanceRepository::delete_cleanup_claimed(
            harness.database.pool(),
            abandoned.version_id,
        )
        .await?
    );

    assert_eq!(
        ArtifactVersionRepository::count_by_artifact(harness.database.pool(), fixture.artifact_id)
            .await?,
        1,
        "sealed shared log remains protected by retention"
    );
    harness.stop().await
}

#[derive(Serialize)]
struct LoadReport {
    schema_version: u8,
    streams: usize,
    segments_per_stream: usize,
    elapsed_ms: u128,
    reconnect_latency_ms_p50: u128,
    reconnect_latency_ms_p95: u128,
    tail_database_queries: u64,
    database_queries_per_second: f64,
    object_puts: u64,
    object_gets: u64,
    object_heads: u64,
    active_streams_after_run: u64,
    db_pool_connections: u32,
    db_pool_active_connections: u32,
}

#[tokio::test]
#[ignore = "opt-in bounded load test; set ATTUNE_RUN_LOG_STREAM_LOAD=1"]
async fn bounded_runtime_log_load_report() -> Result<()> {
    if std::env::var("ATTUNE_RUN_LOG_STREAM_LOAD").as_deref() != Ok("1") {
        eprintln!("skipped: set ATTUNE_RUN_LOG_STREAM_LOAD=1 to run the bounded load test");
        return Ok(());
    }
    let streams = std::env::var("ATTUNE_LOG_LOAD_STREAMS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_usize)
        .clamp(1, 200);
    let harness = Harness::start(&[true, true]).await?;
    let client = reqwest::Client::new();
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(streams);
    for _ in 0..streams {
        let fixture = harness.fixture(LogStreamBackend::ObjectSegments).await?;
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            0,
            b"load-a",
            &harness.worker_token,
        )
        .await?;
        let reconnect_started = Instant::now();
        let mut reader =
            Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, Some(6)).await?);
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            1,
            b"load-b",
            &harness.worker_token,
        )
        .await?;
        seal(
            &client,
            &harness.replicas[0],
            &fixture,
            false,
            &harness.worker_token,
        )
        .await?;
        next_data_event(&mut reader, Duration::from_secs(3)).await?;
        next_event(&mut reader, "done", Duration::from_secs(3)).await?;
        latencies.push(reconnect_started.elapsed().as_millis());
    }
    let elapsed = started.elapsed();
    latencies.sort_unstable();
    let metrics = client
        .get(format!("{}/metrics", harness.replicas[1].url))
        .send()
        .await?
        .text()
        .await?;
    let queries = metric(
        &metrics,
        "attune_execution_log_stream_tail_database_queries_total",
    );
    let pool = harness.database.pool();
    let pool_size = pool.size();
    let report = LoadReport {
        schema_version: 1,
        streams,
        segments_per_stream: 2,
        elapsed_ms: elapsed.as_millis(),
        reconnect_latency_ms_p50: latencies[latencies.len() / 2],
        reconnect_latency_ms_p95: latencies[(latencies.len() * 95 / 100).min(latencies.len() - 1)],
        tail_database_queries: queries,
        database_queries_per_second: queries as f64 / elapsed.as_secs_f64(),
        object_puts: harness.counts.puts.load(Ordering::Relaxed),
        object_gets: harness.counts.gets.load(Ordering::Relaxed),
        object_heads: harness.counts.heads.load(Ordering::Relaxed),
        active_streams_after_run: metric(&metrics, "attune_execution_log_streams_active"),
        db_pool_connections: pool_size,
        db_pool_active_connections: pool_size - pool.num_idle() as u32,
    };
    println!("{}", serde_json::to_string(&report)?);
    harness.stop().await
}
