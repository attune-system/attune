mod helpers;

use std::{
    env, fmt,
    fs::OpenOptions,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use attune_api::{postgres_listener, AppState, Server};
use attune_common::{
    auth::jwt::{generate_execution_token, generate_worker_token, JwtConfig},
    blob_store::{
        BlobBody, BlobReader, BlobStore, BlobStoreError, ByteRange, ObjectKey, ProviderVersion,
        S3BlobStore, StoredObject,
    },
    config::{BlobStorageConfig, Config},
    db::Database,
    models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, ExecutionStatus,
        LogStreamBackend, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        log_stream::LogStreamRepository,
        maintenance::MaintenanceRepository,
        storage_maintenance::StorageMaintenanceRepository,
        Create,
    },
    test_database::TestDatabase,
};
use eventsource_stream::{Event, Eventsource};
use futures::{future::join_all, Stream, StreamExt, TryStreamExt};
use helpers::init_test_env;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct StoreCounts {
    puts: AtomicU64,
    gets: AtomicU64,
    heads: AtomicU64,
    deletes: AtomicU64,
}

#[derive(Default)]
struct StoreControl {
    response_delay_ms: AtomicU64,
    fail_after_successful_put: AtomicBool,
}

struct CountingBlobStore {
    inner: S3BlobStore,
    counts: Arc<StoreCounts>,
    control: Arc<StoreControl>,
}

impl fmt::Debug for CountingBlobStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CountingBlobStore").finish()
    }
}

impl CountingBlobStore {
    fn new(
        config: &S3TestConfig,
        prefix: &str,
        counts: Arc<StoreCounts>,
        control: Arc<StoreControl>,
    ) -> Result<Self> {
        Ok(Self {
            inner: S3BlobStore::new(
                &config.bucket,
                &config.region,
                prefix,
                Some(&config.endpoint),
                None,
            )?,
            counts,
            control,
        })
    }

    async fn delay(&self) {
        let delay = self.control.response_delay_ms.load(Ordering::Relaxed);
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
        let result = self.inner.put(key, body, expected_sha256).await;
        self.delay().await;
        if result.is_ok()
            && self
                .control
                .fail_after_successful_put
                .swap(false, Ordering::SeqCst)
        {
            return Err(BlobStoreError::Backend(
                "injected ambiguous response after successful S3 PUT".to_string(),
            ));
        }
        result
    }

    async fn get(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        range: Option<ByteRange>,
    ) -> std::result::Result<BlobReader, BlobStoreError> {
        self.counts.gets.fetch_add(1, Ordering::Relaxed);
        let result = self.inner.get(key, version, range).await;
        self.delay().await;
        result
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
        let result = self
            .inner
            .get_pinned(key, version, object_size, object_sha256, range)
            .await;
        self.delay().await;
        result
    }

    async fn head(
        &self,
        key: &ObjectKey,
    ) -> std::result::Result<Option<StoredObject>, BlobStoreError> {
        self.counts.heads.fetch_add(1, Ordering::Relaxed);
        let result = self.inner.head(key).await;
        self.delay().await;
        result
    }

    async fn delete(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
    ) -> std::result::Result<(), BlobStoreError> {
        self.counts.deletes.fetch_add(1, Ordering::Relaxed);
        let result = self.inner.delete(key, version).await;
        self.delay().await;
        result
    }
}

struct S3TestConfig {
    endpoint: String,
    bucket: String,
    region: String,
}

impl S3TestConfig {
    fn from_env() -> Result<Self> {
        for name in [
            "ATTUNE_TEST_S3_ENDPOINT",
            "ATTUNE_TEST_S3_BUCKET",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
        ] {
            if env::var_os(name).is_none() {
                return Err(format!(
                    "{name} is required; run `make runtime-log-test-storage-up` first"
                )
                .into());
            }
        }
        Ok(Self {
            endpoint: env::var("ATTUNE_TEST_S3_ENDPOINT")?,
            bucket: env::var("ATTUNE_TEST_S3_BUCKET")?,
            region: env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
        })
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
    controls: Vec<Arc<StoreControl>>,
    _root: tempfile::TempDir,
    worker_token: String,
    jwt: JwtConfig,
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
        let s3 = S3TestConfig::from_env()?;
        let prefix = format!("runtime-log-tests/{}", uuid::Uuid::new_v4().simple());
        config.storage = BlobStorageConfig::S3 {
            bucket: s3.bucket.clone(),
            region: s3.region.clone(),
            prefix: prefix.clone(),
            endpoint: Some(s3.endpoint.clone()),
            kms_key: None,
        };
        let counts = Arc::new(StoreCounts::default());
        let jwt = JwtConfig {
            secret: config.security.jwt_secret.clone().unwrap(),
            access_token_expiration: 300,
            refresh_token_expiration: 3600,
        };
        let worker_token = generate_worker_token(1, "replica-test", &jwt, None)?;
        let mut replicas = Vec::new();
        let mut controls = Vec::new();
        for listen_for_notifications in notification_listeners {
            let database = Database::new(&config.database).await?;
            let control = Arc::new(StoreControl::default());
            let store: Arc<dyn BlobStore> = Arc::new(CountingBlobStore::new(
                &s3,
                &prefix,
                counts.clone(),
                control.clone(),
            )?);
            store.preflight().await?;
            let state = Arc::new(AppState::new_with_audit_and_blob_store(
                database.pool().clone(),
                config.clone(),
                attune_common::audit::AuditEmitter::noop(),
                store,
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let server = tokio::spawn(Server::new(state.clone()).run_with_listener(listener));
            let notifications = if *listen_for_notifications {
                Some(
                    postgres_listener::spawn_postgres_listener(
                        state.db.clone(),
                        state.broadcast_tx.clone(),
                        state.log_stream_wakeups.clone(),
                    )
                    .await?,
                )
            } else {
                None
            };
            replicas.push(Replica {
                url: format!("http://{address}"),
                state,
                server,
                notifications,
            });
            controls.push(control);
        }
        Ok(Self {
            database,
            replicas,
            counts,
            controls,
            _root: root,
            worker_token,
            jwt,
        })
    }

    async fn fixture(&self, backend: LogStreamBackend) -> Result<LogFixture> {
        let action_ref = format!("replica_{}.run", uuid::Uuid::new_v4().simple());
        let execution = self.create_execution(&action_ref).await?;
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
        self.fixture_version(
            backend,
            &action_ref,
            &artifact_ref,
            artifact.id,
            execution.id,
        )
        .await
    }

    async fn next_version(&self, fixture: &LogFixture) -> Result<LogFixture> {
        let execution = self.create_execution(&fixture.action_ref).await?;
        self.fixture_version(
            LogStreamBackend::SharedFile,
            &fixture.action_ref,
            &fixture.artifact_ref,
            fixture.artifact_id,
            execution.id,
        )
        .await
    }

    async fn create_execution(
        &self,
        action_ref: &str,
    ) -> Result<attune_common::models::execution::Execution> {
        Ok(ExecutionRepository::create(
            self.database.pool(),
            CreateExecutionInput {
                action: None,
                action_ref: action_ref.to_string(),
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
        .await?)
    }

    async fn fixture_version(
        &self,
        backend: LogStreamBackend,
        action_ref: &str,
        artifact_ref: &str,
        artifact_id: i64,
        execution_id: i64,
    ) -> Result<LogFixture> {
        let version = ArtifactVersionRepository::create_log_pending(
            self.database.pool(),
            artifact_id,
            artifact_ref,
            backend,
            "text/plain".to_string(),
            Some(execution_id),
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
        let execution_token =
            generate_execution_token(1, execution_id, action_ref, &self.jwt, None)?;
        Ok(LogFixture {
            execution_id,
            artifact_id,
            version_id: version.id,
            stream_id: stream.id,
            file_path: version.file_path.unwrap(),
            action_ref: action_ref.to_string(),
            artifact_ref: artifact_ref.to_string(),
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
    action_ref: String,
    artifact_ref: String,
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
#[ignore = "integration test - requires PostgreSQL and versioned MinIO"]
async fn object_minio_upload_reconnect_and_pinned_reads() -> Result<()> {
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

    let segment = LogStreamRepository::segments(harness.database.pool(), fixture.stream_id)
        .await?
        .remove(0);
    let key = ObjectKey::new(segment.object_key)?;
    let version = ProviderVersion::from_stored(segment.provider_version)?;
    assert!(version.as_stored().starts_with("v:"));
    let digest: [u8; 32] = hex::decode(segment.sha256)?.try_into().unwrap();
    let bytes = harness.replicas[1]
        .state
        .blob_store
        .get_pinned(&key, &version, segment.size_bytes as u64, digest, None)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .concat();
    assert_eq!(bytes, b"first");
    let wrong_version = ProviderVersion::from_stored("v:not-the-recorded-minio-version")?;
    let wrong = harness.replicas[1]
        .state
        .blob_store
        .get_pinned(
            &key,
            &wrong_version,
            segment.size_bytes as u64,
            digest,
            None,
        )
        .await;
    match wrong {
        Err(_) => {}
        Ok(reader) => assert!(reader.try_collect::<Vec<_>>().await.is_err()),
    }
    harness.stop().await
}

#[tokio::test]
#[ignore = "integration test - requires PostgreSQL and versioned MinIO"]
async fn object_minio_duplicate_ambiguous_and_finalize_orderings() -> Result<()> {
    let harness = Harness::start(&[true, true]).await?;
    let fixture = harness.fixture(LogStreamBackend::ObjectSegments).await?;
    let client = reqwest::Client::new();
    harness.controls[0]
        .response_delay_ms
        .store(75, Ordering::Relaxed);
    harness.controls[0]
        .fail_after_successful_put
        .store(true, Ordering::SeqCst);
    let started = Instant::now();
    assert_eq!(
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            0,
            b"duplicate",
            &harness.worker_token,
        )
        .await?,
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(started.elapsed() >= Duration::from_millis(75));
    harness.controls[0]
        .response_delay_ms
        .store(0, Ordering::Relaxed);

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
    assert!(statuses.iter().all(reqwest::StatusCode::is_success));
    assert_eq!(
        LogStreamRepository::segments(harness.database.pool(), fixture.stream_id)
            .await?
            .len(),
        1
    );
    assert_eq!(
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            1,
            b" append-before-seal",
            &harness.worker_token,
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
    assert_eq!(
        put_segment(
            &client,
            &harness.replicas[1],
            &fixture,
            1,
            b" append-before-seal",
            &harness.worker_token,
        )
        .await?,
        reqwest::StatusCode::OK
    );

    let seal_first = harness.fixture(LogStreamBackend::ObjectSegments).await?;
    assert_eq!(
        seal(
            &client,
            &harness.replicas[1],
            &seal_first,
            false,
            &harness.worker_token,
        )
        .await?,
        reqwest::StatusCode::OK
    );
    assert_eq!(
        put_segment(
            &client,
            &harness.replicas[0],
            &seal_first,
            0,
            b"seal-before-append",
            &harness.worker_token,
        )
        .await?,
        reqwest::StatusCode::CONFLICT
    );
    assert!(harness.counts.puts.load(Ordering::Relaxed) >= 3);
    assert!(harness.counts.heads.load(Ordering::Relaxed) >= 2);
    harness.stop().await
}

#[tokio::test]
#[ignore = "integration test - requires PostgreSQL and versioned MinIO; waits for reconciliation"]
async fn object_minio_reader_recovers_missed_notifications_and_terminal() -> Result<()> {
    let harness = Harness::start(&[true, false]).await?;
    let fixture = harness.fixture(LogStreamBackend::ObjectSegments).await?;
    let client = reqwest::Client::new();
    let mut reader =
        Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, None).await?);
    sqlx::query("UPDATE execution SET status = 'completed' WHERE id = $1")
        .bind(fixture.execution_id)
        .execute(harness.database.pool())
        .await?;
    assert_eq!(
        put_segment(
            &client,
            &harness.replicas[0],
            &fixture,
            0,
            b"reconciled",
            &harness.worker_token,
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
            &harness.worker_token,
        )
        .await?,
        reqwest::StatusCode::OK
    );

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
struct LockedVolumeChild {
    child: Child,
    ready: PathBuf,
}

#[cfg(unix)]
impl LockedVolumeChild {
    async fn spawn(base_dir: &str, file_path: &str, content: &str) -> Result<Self> {
        let ready = Path::new(base_dir).join(format!(
            ".runtime-log-child-{}.ready",
            uuid::Uuid::new_v4().simple()
        ));
        let child = Command::new(env::current_exe()?)
            .args([
                "--exact",
                "volume_transport_child_holds_lock",
                "--ignored",
                "--nocapture",
            ])
            .env("ATTUNE_VOLUME_CHILD_MODE", "hold-lock")
            .env("ATTUNE_VOLUME_CHILD_BASE", base_dir)
            .env("ATTUNE_VOLUME_CHILD_PATH", file_path)
            .env("ATTUNE_VOLUME_CHILD_CONTENT", content)
            .env("ATTUNE_VOLUME_CHILD_READY", &ready)
            .stdout(Stdio::null())
            .spawn()?;
        let mut child = Self { child, ready };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if tokio::fs::try_exists(&child.ready).await? {
                return Ok(child);
            }
            if let Some(status) = child.child.try_wait()? {
                return Err(format!("volume helper exited before locking: {status}").into());
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for volume helper lock".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn terminate(&mut self) -> Result<()> {
        self.child.kill()?;
        self.child.wait()?;
        let _ = std::fs::remove_file(&self.ready);
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for LockedVolumeChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.ready);
    }
}

#[cfg(unix)]
#[test]
#[ignore = "helper process for shared-volume integration test"]
fn volume_transport_child_holds_lock() -> Result<()> {
    if env::var("ATTUNE_VOLUME_CHILD_MODE").as_deref() != Ok("hold-lock") {
        return Ok(());
    }
    use attune_common::artifact_transport::{ArtifactFileTransport, VolumeTransport};

    let base = env::var("ATTUNE_VOLUME_CHILD_BASE")?;
    let file_path = env::var("ATTUNE_VOLUME_CHILD_PATH")?;
    let content = env::var("ATTUNE_VOLUME_CHILD_CONTENT")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime
        .block_on(VolumeTransport::new(&base).append_log_file(&file_path, content.as_bytes()))?;
    let full_path = Path::new(&base).join(&file_path);
    let file = OpenOptions::new().read(true).write(true).open(&full_path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    std::fs::write(env::var("ATTUNE_VOLUME_CHILD_READY")?, b"locked")?;
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "integration test - requires PostgreSQL, versioned MinIO, and Unix flock"]
async fn shared_volume_cross_process_locking_writer_loss_and_retention() -> Result<()> {
    use attune_common::artifact_transport::{ArtifactFileTransport, VolumeTransport};

    let harness = Harness::start(&[true, true]).await?;
    let fixture = harness.fixture(LogStreamBackend::SharedFile).await?;
    let base_dir = &harness.replicas[0].state.config.artifacts_dir;
    let writer = VolumeTransport::new(base_dir);
    let other_pod = VolumeTransport::new(&harness.replicas[1].state.config.artifacts_dir);
    let mut writer_process =
        LockedVolumeChild::spawn(base_dir, &fixture.file_path, "cross-process bytes").await?;
    let mut direct = other_pod.open_reader(&fixture.file_path, 0).await?;
    let mut visible = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut direct, &mut visible).await?;
    assert_eq!(visible, "cross-process bytes");

    let client = reqwest::Client::new();
    let mut reader =
        Box::pin(open_log_stream(&client, &harness.replicas[1], &fixture, None).await?);
    assert_eq!(
        next_data_event(&mut reader, Duration::from_secs(2))
            .await?
            .data,
        "cross-process bytes"
    );
    let seal_status = {
        let seal_request = seal(
            &client,
            &harness.replicas[1],
            &fixture,
            true,
            &harness.worker_token,
        );
        tokio::pin!(seal_request);
        tokio::select! {
            result = &mut seal_request => {
                return Err(format!("seal bypassed the child flock: {:?}", result?).into());
            }
            _ = tokio::time::sleep(Duration::from_millis(150)) => {}
        }
        writer_process.terminate()?;
        seal_request.await?
    };
    assert_eq!(seal_status, reqwest::StatusCode::OK);
    next_event(&mut reader, "done", Duration::from_secs(2)).await?;
    drop(reader);
    let (truncated, sealed): (bool, bool) =
        sqlx::query_as("SELECT truncated, sealed FROM log_stream WHERE id = $1")
            .bind(fixture.stream_id)
            .fetch_one(harness.database.pool())
            .await?;
    assert!(truncated && sealed);

    let abandoned = harness.fixture(LogStreamBackend::SharedFile).await?;
    let mut abandoned_process =
        LockedVolumeChild::spawn(base_dir, &abandoned.file_path, "unsealed").await?;
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
        !other_pod
            .delete_abandoned_log_file(&abandoned.file_path)
            .await?
    );
    abandoned_process.terminate()?;
    let retry_candidates = StorageMaintenanceRepository::abandoned_shared_log_pending(
        harness.database.pool(),
        cutoff,
        10,
    )
    .await?;
    assert!(retry_candidates
        .iter()
        .any(|candidate| candidate.id == abandoned.version_id));
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

    let retained = harness.next_version(&fixture).await?;
    writer
        .append_log_file(&retained.file_path, b"retained version")
        .await?;
    assert_eq!(
        seal(
            &client,
            &harness.replicas[0],
            &retained,
            false,
            &harness.worker_token,
        )
        .await?,
        reqwest::StatusCode::OK
    );
    let expired =
        MaintenanceRepository::find_expired_artifact_versions(harness.database.pool(), 100).await?;
    assert!(expired
        .iter()
        .any(|version| version.id == fixture.version_id));
    other_pod.delete_file(&fixture.file_path).await?;
    assert!(MaintenanceRepository::delete_artifact_version(
        harness.database.pool(),
        fixture.version_id,
    )
    .await?);
    assert!(!other_pod.file_exists(&fixture.file_path).await?);
    assert!(other_pod.file_exists(&retained.file_path).await?);
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
    postgres_statement_calls_delta: u64,
    postgres_statement_calls_per_second: f64,
    s3_puts: u64,
    s3_gets: u64,
    s3_heads: u64,
    peak_aggregate_pool_connections: u32,
    peak_aggregate_active_pool_connections: u32,
    peak_active_streams: u64,
    leaked_active_streams: u64,
}

async fn postgres_statement_calls(pool: &sqlx::PgPool) -> Result<u64> {
    let calls: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(calls), 0)::BIGINT FROM pg_stat_statements \
         WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database()) \
         AND query NOT LIKE '%pg_stat_statements%'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| format!(
        "pg_stat_statements is required for the load report; configure shared_preload_libraries and CREATE EXTENSION: {error}"
    ))?;
    Ok(calls.max(0) as u64)
}

async fn sample_runtime_peaks(
    states: Vec<Arc<AppState>>,
    stop: CancellationToken,
) -> (u32, u32, u64) {
    let mut peak_open = 0;
    let mut peak_active = 0;
    let mut peak_streams = 0;
    loop {
        peak_open = peak_open.max(states.iter().map(|state| state.db.size()).sum());
        peak_active = peak_active.max(
            states
                .iter()
                .map(|state| state.db.size().saturating_sub(state.db.num_idle() as u32))
                .sum(),
        );
        peak_streams = peak_streams.max(
            states
                .iter()
                .map(|state| {
                    metric(
                        &state.execution_log_streams.render_metrics(),
                        "attune_execution_log_streams_active",
                    )
                })
                .sum(),
        );
        tokio::select! {
            _ = stop.cancelled() => return (peak_open, peak_active, peak_streams),
            _ = tokio::time::sleep(Duration::from_millis(5)) => {}
        }
    }
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
    let mut fixtures = Vec::with_capacity(streams);
    for _ in 0..streams {
        fixtures.push(harness.fixture(LogStreamBackend::ObjectSegments).await?);
    }
    let calls_before = postgres_statement_calls(harness.database.pool()).await?;
    let stop_sampler = CancellationToken::new();
    let sampler = tokio::spawn(sample_runtime_peaks(
        harness
            .replicas
            .iter()
            .map(|replica| replica.state.clone())
            .collect(),
        stop_sampler.clone(),
    ));
    let started = Instant::now();
    let runs = fixtures.iter().map(|fixture| {
        let client = &client;
        let harness = &harness;
        async move {
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
            Ok::<u128, Box<dyn std::error::Error>>(reconnect_started.elapsed().as_millis())
        }
    });
    let mut latencies = join_all(runs)
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let elapsed = started.elapsed();
    stop_sampler.cancel();
    let (peak_open, peak_active, peak_streams) = sampler.await?;
    let calls_after = postgres_statement_calls(harness.database.pool()).await?;
    latencies.sort_unstable();
    let leaked_streams: u64 = harness
        .replicas
        .iter()
        .map(|replica| {
            metric(
                &replica.state.execution_log_streams.render_metrics(),
                "attune_execution_log_streams_active",
            )
        })
        .sum();
    let statement_calls = calls_after.saturating_sub(calls_before);
    let report = LoadReport {
        schema_version: 2,
        streams,
        segments_per_stream: 2,
        elapsed_ms: elapsed.as_millis(),
        reconnect_latency_ms_p50: latencies[latencies.len() / 2],
        reconnect_latency_ms_p95: latencies[(latencies.len() * 95 / 100).min(latencies.len() - 1)],
        postgres_statement_calls_delta: statement_calls,
        postgres_statement_calls_per_second: statement_calls as f64 / elapsed.as_secs_f64(),
        s3_puts: harness.counts.puts.load(Ordering::Relaxed),
        s3_gets: harness.counts.gets.load(Ordering::Relaxed),
        s3_heads: harness.counts.heads.load(Ordering::Relaxed),
        peak_aggregate_pool_connections: peak_open,
        peak_aggregate_active_pool_connections: peak_active,
        peak_active_streams: peak_streams,
        leaked_active_streams: leaked_streams,
    };
    println!("{}", serde_json::to_string(&report)?);
    harness.stop().await
}
