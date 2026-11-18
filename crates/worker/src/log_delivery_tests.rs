use super::*;
use attune_common::{
    artifact_transport::VolumeTransport,
    config::{Config, LogDeliveryConfig},
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, LogStreamBackend, OwnerType,
        RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        file_evidence_retention::FileEvidenceRetentionRepository as Graph,
        Create,
    },
    test_database::TestDatabase,
};
use sqlx::Connection;

struct Fixture {
    manager: Arc<LogDeliveryManager>,
    database: TestDatabase,
    directory: tempfile::TempDir,
    control_stop: CancellationToken,
    control_server: tokio::task::JoinHandle<()>,
    control_requests: Arc<Notify>,
}
impl Fixture {
    async fn new() -> Self {
        attune_common::auth::install_crypto_provider();
        let mut config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        config.database.max_connections = 2;
        config.database.min_connections = 0;
        config.database.connect_timeout = 1;
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let directory = tempfile::tempdir().unwrap();
        let tokens = Arc::new(WorkerTokenProvider::new(
            1,
            "4",
            attune_common::auth::jwt::JwtConfig {
                secret: "log-delivery-test-signing-key".into(),
                access_token_expiration: 60,
                refresh_token_expiration: 60,
            },
        ));
        use attune_common::repositories::runtime::{CreateWorkerInput, WorkerRepository};
        let worker = WorkerRepository::register_incarnation(
            database.pool(),
            CreateWorkerInput {
                name: format!("custody-{}", Uuid::new_v4()),
                worker_type: attune_common::models::WorkerType::Local,
                runtime: None,
                host: None,
                port: None,
                status: None,
                capabilities: Some(
                    serde_json::json!({"log_delivery_instance": tokens.instance_id()}),
                ),
                meta: None,
            },
            attune_common::models::WorkerRole::Action,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        tokens.set_worker_id(worker.id.to_string());
        let pool = database.pool().clone();
        let instance = tokens.instance_id();
        let expected = tokens.token().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        // Transport fixture uses the canonical repository control decision;
        // authorization/rejection coverage belongs to the real API tests.
        let control_requests = Arc::new(Notify::new());
        let observed = control_requests.clone();
        let app = axum::Router::new().route(
            "/api/v1/internal/log-delivery/control",
            axum::routing::post(
                move |headers: axum::http::HeaderMap,
                      axum::Json(request): axum::Json<
                    attune_common::log_delivery::ControlRequest,
                >| {
                    let pool = pool.clone();
                    let expected = expected.clone();
                    let observed = observed.clone();
                    async move {
                        use axum::response::IntoResponse;
                        if headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            != Some(format!("Bearer {expected}").as_str())
                        {
                            return axum::http::StatusCode::UNAUTHORIZED.into_response();
                        }
                        observed.notify_one();
                        use attune_common::repositories::log_delivery::ControlDecision;
                        match LogDeliveryRepository::reconcile_control(
                            &pool,
                            worker.id,
                            instance,
                            &request,
                            chrono::Utc::now().timestamp() + 60,
                        )
                        .await
                        {
                            Ok(ControlDecision::Settled) => {
                                axum::Json(attune_common::log_delivery::ControlResponse::Settled)
                                    .into_response()
                            }
                            Ok(ControlDecision::Waiting) => {
                                axum::Json(attune_common::log_delivery::ControlResponse::Waiting)
                                    .into_response()
                            }
                            _ => axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
                        }
                    }
                },
            ),
        );
        let control_stop = CancellationToken::new();
        let stopped = control_stop.clone();
        let control_server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
                .unwrap();
        });
        let manager = LogDeliveryManager::open(
            database.pool().clone(),
            directory.path().join("spool"),
            worker.id,
            tokens.instance_id(),
            tokens,
            api,
            LogDeliveryConfig {
                max_finalizers: 1,
                max_spool_bytes: 100_000,
                ..Default::default()
            },
            1024,
            1024,
        )
        .await
        .unwrap();
        Self {
            manager,
            database,
            directory,
            control_stop,
            control_server,
            control_requests,
        }
    }
    fn writer(&self) -> SpoolingLogWriter {
        let capture = Arc::new(Mutex::new(Capture {
            segments: vec![SpoolSegment {
                sequence: 0,
                bytes: b"private log".to_vec(),
            }],
            bytes: 11,
            ready: None,
            finalizing: true,
        }));
        let changed = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let manager = self.manager.clone();
        let slot = manager.slots.clone().try_acquire_owned().unwrap();
        let writer = SpoolingLogWriter {
            manager: manager.clone(),
            execution: 1,
            version: 9_000_000_001,
            stream: 9_000_000_002,
            attempt: Uuid::new_v4(),
            capture: capture.clone(),
            changed: changed.clone(),
            cancel: cancel.clone(),
            handed_off: false,
        };
        let transport = Arc::new(VolumeTransport::new(
            self.directory.path().to_str().unwrap(),
        ));
        self.manager.tasks.spawn(async move {
            manager
                .live(9_000_000_001, capture, changed, cancel, transport, slot)
                .await
        });
        writer
    }
    async fn wait_for_spool(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.manager.spool.versions().await.unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("settlement must publish custody before DB preparation");
    }
    async fn finish(self) {
        self.manager.tasks.close();
        tokio::time::timeout(Duration::from_secs(10), self.manager.tasks.wait())
            .await
            .expect("custody tasks must settle after DB recovery");
        assert!(self.manager.spool.versions().await.unwrap().is_empty());
        assert_eq!(self.manager.slots.available_permits(), 1);
        drop(self.manager);
        self.control_stop.cancel();
        self.control_server.await.unwrap();
        self.database.cleanup().await.unwrap();
    }
    async fn finish_retained(self) {
        // No exact CP evidence exists for this deliberately invalid source.
        // Deadline/rollback alone cannot authorize deleting encrypted custody.
        self.manager.stop.cancel();
        self.manager.tasks.close();
        tokio::time::timeout(Duration::from_secs(10), self.manager.tasks.wait())
            .await
            .unwrap();
        assert!(!self.manager.spool.versions().await.unwrap().is_empty());
        drop(self.manager);
        self.control_stop.cancel();
        self.control_server.await.unwrap();
        self.database.cleanup().await.unwrap();
    }
    async fn entry(&self) -> SpoolDelivery {
        // Cleanup must also work after independently retained source rows have
        // disappeared. Only version/stream have FKs; no live authority is minted.
        let artifact = ArtifactRepository::create(
            self.database.pool(),
            CreateArtifactInput {
                r#ref: "test.custody".into(),
                scope: OwnerType::System,
                owner: "".into(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: None,
                data: None,
            },
        )
        .await
        .unwrap();
        let version = ArtifactVersionRepository::create_log_pending(
            self.database.pool(),
            artifact.id,
            &artifact.r#ref,
            LogStreamBackend::ObjectSegments,
            "text/plain".into(),
            Some(1),
            None,
            None,
        )
        .await
        .unwrap();
        let stream = LogStreamRepository::create_with_backend(
            self.database.pool(),
            version.id,
            LogStreamBackend::ObjectSegments,
            1024,
            500,
        )
        .await
        .unwrap();
        SpoolDelivery {
            execution: 1,
            version: version.id,
            stream: stream.id,
            worker: self.manager.worker,
            attempt: Uuid::new_v4(),
            source_instance: self.manager.instance,
            custody: Uuid::new_v4(),
            finalize_by: chrono::Utc::now() + chrono::Duration::seconds(3),
            truncated: false,
            authenticated_manifest: None,
            segments: vec![SpoolSegment {
                sequence: 0,
                bytes: b"private log".to_vec(),
            }],
        }
    }
    async fn insert_intent(&self, tx: &mut sqlx::PgConnection, entry: &SpoolDelivery) {
        Graph::lock_capture(tx).await.unwrap();
        sqlx::query("INSERT INTO log_delivery_intent(execution,artifact_version,stream,worker,source_instance,delivery_instance,attempt,owner,custody,segments,total_bytes,sha256,truncated,finalize_by) VALUES($1,$2,$3,$4,$5,$5,$6,1,$7,$8,11,$9,false,$10)")
            .bind(entry.execution).bind(entry.version).bind(entry.stream).bind(entry.worker).bind(entry.source_instance)
            .bind(entry.attempt).bind(entry.custody).bind(serde_json::json!([{"sequence":0,"size_bytes":11,"sha256":hash(b"private log")}]))
            .bind(hash(b"private log")).bind(entry.finalize_by).execute(tx).await.unwrap();
    }
    fn deliver(&self, entry: SpoolDelivery) {
        let manager = self.manager.clone();
        let slot = manager.slots.clone().try_acquire_owned().unwrap();
        self.manager.tasks.spawn(async move {
            let _slot = slot;
            manager.deliver(entry).await;
        });
    }
}

#[tokio::test]
async fn failed_preparation_without_retained_proof_preserves_custody() {
    let fixture = Fixture::new().await;
    // Missing source proof cannot become an absence acknowledgement.
    assert!(fixture.writer().seal(false, None).await.is_err());
    fixture.finish_retained().await;
}

#[tokio::test]
async fn cancelled_caller_cannot_abandon_persisted_preparation() {
    let fixture = Fixture::new().await;
    let first = fixture.database.pool().acquire().await.unwrap();
    let second = fixture.database.pool().acquire().await.unwrap();
    let writer = fixture.writer();
    let caller = tokio::spawn(writer.seal(false, None));
    fixture.wait_for_spool().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(first);
    drop(second);
    fixture.finish_retained().await;
}

#[tokio::test]
async fn uncertain_commit_is_fenced_before_orphan_reclamation() {
    let fixture = Fixture::new().await;
    let entry = fixture.entry().await;
    fixture.manager.spool.save(entry.clone()).await.unwrap();
    let mut producer = fixture.database.pool().begin().await.unwrap();
    fixture.insert_intent(&mut producer, &entry).await;
    fixture.deliver(entry.clone());
    let mut monitor = sqlx::PgConnection::connect(fixture.database.database_url())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE NOT granted AND locktype='advisory' AND database=(SELECT oid FROM pg_database WHERE datname=current_database()))").fetch_one(&mut monitor).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("absence reconciliation must wait for the producer transaction");
    assert!(fixture.manager.spool.load(entry.version).await.is_ok());
    sqlx::query("SELECT pg_sleep(GREATEST(0,EXTRACT(EPOCH FROM ($1::timestamptz-clock_timestamp())))::double precision)")
        .bind(entry.finalize_by).execute(&mut *producer).await.unwrap();
    // The canonical commit succeeds while the original producer never resumes
    // to acknowledge it. Reconciliation must discover it, then expire safely.
    producer.commit().await.unwrap();
    monitor.close().await.unwrap();
    fixture.manager.tasks.close();
    tokio::time::timeout(Duration::from_secs(10), fixture.manager.tasks.wait())
        .await
        .unwrap();
    let intent = LogDeliveryRepository::find(fixture.database.pool(), entry.version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.state, "incomplete");
    fixture.finish().await;
}

#[tokio::test]
async fn database_outage_across_expiry_does_not_strand_capacity() {
    let fixture = Fixture::new().await;
    let entry = fixture.entry().await;
    fixture.manager.spool.save(entry.clone()).await.unwrap();
    let mut tx = fixture.database.pool().begin().await.unwrap();
    fixture.insert_intent(&mut tx, &entry).await;
    tx.commit().await.unwrap();
    let first = fixture.database.pool().acquire().await.unwrap();
    let second = fixture.database.pool().acquire().await.unwrap();
    fixture.deliver(entry.clone());
    fixture.manager.tasks.close();
    // Real acquisition timeouts cross the original three-second deadline. The
    // task must retain responsibility instead of exiting on the first error.
    assert!(
        tokio::time::timeout(Duration::from_secs(5), fixture.manager.tasks.wait())
            .await
            .is_err()
    );
    assert!(fixture.manager.spool.load(entry.version).await.is_ok());
    assert_eq!(fixture.manager.slots.available_permits(), 0);
    drop(first);
    drop(second);
    tokio::time::timeout(Duration::from_secs(10), fixture.manager.tasks.wait())
        .await
        .unwrap();
    let intent = LogDeliveryRepository::find(fixture.database.pool(), entry.version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.state, "incomplete");
    assert_eq!(intent.failure.as_deref(), Some("deadline"));
    fixture.finish().await;
}

#[tokio::test]
async fn cancelled_manager_initialization_retains_lease_until_custody_owners_stop(
) -> anyhow::Result<()> {
    let fixture = Fixture::new().await;
    let mut first = fixture.entry().await;
    first.finalize_by = chrono::Utc::now() + chrono::Duration::minutes(5);
    let mut second = first.clone();
    second.version += 1;
    second.custody = Uuid::new_v4();
    fixture.manager.spool.save(first.clone()).await?;
    fixture.manager.spool.save(second.clone()).await?;
    fixture.manager.stop_and_join().await;
    let Fixture {
        manager,
        database,
        directory,
        control_stop,
        control_server,
        control_requests,
    } = fixture;
    let pool = database.pool().clone();
    let worker = manager.worker;
    let instance = manager.instance;
    let tokens = manager.tokens.clone();
    let api = manager.api.clone();
    let root = directory.path().join("spool");
    let key = std::fs::read(root.join("custody.key"))?;
    drop(manager);
    let recovering_root = root.clone();
    let recovering = tokio::spawn(async move {
        LogDeliveryManager::open(
            pool,
            recovering_root,
            worker,
            instance,
            tokens,
            api,
            LogDeliveryConfig {
                max_finalizers: 1,
                max_spool_bytes: 100_000,
                ..Default::default()
            },
            1024,
            1024,
        )
        .await
    });
    let admitted = async {
        // Actual first recovery transport, with one occupied slot. The second
        // retained entry keeps open() pending; no fixed sleep models this state.
        tokio::time::timeout(Duration::from_secs(10), control_requests.notified()).await?;
        anyhow::ensure!(
            !recovering.is_finished(),
            "recovery was not held by its retained finalizer"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    recovering.abort();
    let cancelled = recovering.await;
    let checked = async {
        admitted?;
        anyhow::ensure!(
            cancelled.is_err_and(|error| error.is_cancelled()),
            "initialization was not cancelled"
        );
        let lease = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("owner.lock"))?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if lease.try_lock().is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(lease);
        let spool = LogDeliverySpool::open(root.clone(), worker, 100_000, 69_632, 1024).await?;
        anyhow::ensure!(
            spool.versions().await?.len() == 2,
            "cancellation discarded retained custody"
        );
        anyhow::ensure!(
            spool.load(first.version).await? == first
                && spool.load(second.version).await? == second,
            "cancellation changed retained bindings or payloads"
        );
        anyhow::ensure!(
            std::fs::read(root.join("custody.key"))? == key,
            "restart rotated custody key"
        );
        drop(spool);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    control_stop.cancel();
    control_server.await?;
    database.cleanup().await?;
    checked
}
