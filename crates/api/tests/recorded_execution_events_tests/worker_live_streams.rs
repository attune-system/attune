//! Connected reader tests using F's real allocator, permit and transport. No
//! process execution is launched, and no final manifest is written through SQL.
use super::live_streams::{admit_producer, admit_producer_with_input, next_data};
use super::*;
use attune_common::{
    artifact_transport::{ArtifactFileTransport, VolumeTransport},
    models::{LogStreamBackend, RetentionPolicyType, WorkerStatus, WorkerType},
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository},
        key::{CreateKeyInput, KeyRepository},
        runtime::{CreateWorkerInput, WorkerRepository},
        runtime_read::RuntimeReadRepository as Reads,
        worker_delivery::{DeliveryContext, WorkerDeliveryRepository},
        FindById, FindByRef,
    },
    runtime_log_completion::FrameTotals,
};
use axum::{body::Body, http::Request};
use futures::TryStreamExt;
use std::sync::Arc;
use tower::ServiceExt;

pub(super) struct WorkerLog {
    pub(super) execution: Id,
    process_token: String,
    context: DeliveryContext,
    pub(super) version: Id,
    pub(super) artifact: Id,
    path: String,
    transport: Arc<dyn ArtifactFileTransport>,
    frames: u64,
    totals: Option<FrameTotals>,
    _server: RouterServer,
}

struct RouterServer(tokio::task::JoinHandle<()>);
impl Drop for RouterServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct CancelPolls(Vec<tokio::task::AbortHandle>);
impl Drop for CancelPolls {
    fn drop(&mut self) {
        for handle in &self.0 {
            handle.abort();
        }
    }
}

impl WorkerLog {
    async fn allocate_with_input(
        fixture: &Fixture,
        backend: LogStreamBackend,
    ) -> Result<(Self, Id)> {
        let encryption = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        let key = KeyRepository::create(
            &fixture.ctx.pool,
            CreateKeyInput {
                local_ref: format!("accepted_live_{}", uuid::Uuid::new_v4().simple()),
                owner_type: attune_common::models::OwnerType::Pack,
                owner_identity: None,
                owner_pack: Some(fixture.pack_id),
                owner_pack_ref: Some("recorded_emitter".into()),
                owner_action: None,
                owner_action_ref: None,
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Accepted live input".into(),
                encrypted: true,
                encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(encryption)),
                value: attune_common::crypto::encrypt_json(
                    &json!("accepted-input-value"),
                    encryption,
                )?,
            },
        )
        .await?;
        grant_human(
            fixture,
            vec![json!({"resource":"keys","actions":["read","decrypt"],"constraints":{"ids":[key.id]}})],
        )
        .await?;
        let (execution, token) = admit_producer_with_input(fixture, Some(&key.r#ref)).await?;
        let mut conn = fixture.ctx.pool.acquire().await?;
        let origin = attune_common::key_access::key_origin(&key);
        for action in [
            attune_common::rbac::Action::Read,
            attune_common::rbac::Action::Decrypt,
        ] {
            assert!(attune_common::repositories::execution_admission::ExecutionAdmissionRepository::input_key_origin_allowed(&mut conn, execution, &origin, action).await?);
        }
        drop(conn);
        Ok((
            Self::for_execution(fixture, execution, token, backend).await?,
            key.id,
        ))
    }
    pub(super) async fn allocate(fixture: &Fixture, backend: LogStreamBackend) -> Result<Self> {
        let (execution, token) = admit_producer(fixture).await?;
        Self::for_execution(fixture, execution, token, backend).await
    }

    pub(super) async fn for_execution(
        fixture: &Fixture,
        execution: Id,
        process_token: String,
        backend: LogStreamBackend,
    ) -> Result<Self> {
        let worker = WorkerRepository::create(
            &fixture.ctx.pool,
            CreateWorkerInput {
                name: format!("live-delivery-{execution}"),
                worker_type: WorkerType::Local,
                runtime: None,
                host: None,
                port: None,
                status: Some(WorkerStatus::Active),
                capabilities: None,
                meta: None,
            },
        )
        .await?;
        ExecutionRepository::update(
            &fixture.ctx.pool,
            execution,
            UpdateExecutionInput {
                worker: Some(worker.id),
                ..Default::default()
            },
        )
        .await?;
        let execution_row = ExecutionRepository::find_by_id(&fixture.ctx.pool, execution)
            .await?
            .unwrap();
        let context = attune_worker::completion::register(
            &fixture.ctx.pool,
            &execution_row,
            worker.id,
            uuid::Uuid::new_v4(),
        )
        .await?
        .unwrap();
        let (path, version) = attune_worker::completion::allocate_log(
            &fixture.ctx.pool,
            &execution_row,
            &context,
            "stdout",
            backend,
            RetentionPolicyType::Versions,
            5,
            256 * 1024,
            100,
            8 * 1024 * 1024,
        )
        .await?;
        let row = ArtifactVersionRepository::find_by_id(&fixture.ctx.pool, version)
            .await?
            .unwrap();
        let root = ArtifactRepository::find_by_id(&fixture.ctx.pool, row.artifact)
            .await?
            .unwrap();
        assert_eq!(
            root.r#ref,
            attune_common::worker_delivery::log_reference(&execution_row, "stdout")?
        );
        assert_eq!(
            root.visibility,
            attune_common::models::ArtifactVisibility::Private
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = fixture.ctx.app.clone();
        let server = RouterServer(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));
        let underlying = Arc::new(VolumeTransport::new(
            &fixture.ctx.state.config.artifacts_dir,
        ));
        let transport = attune_worker::completion::CompletionTransport::for_version(
            &fixture.ctx.pool,
            version,
            context.worker,
            context.worker_instance,
            underlying,
            &url,
            &fixture.ctx.state.jwt_config,
        )
        .await?;
        let mut conn = fixture.ctx.pool.acquire().await?;
        let permit = WorkerDeliveryRepository::for_version(&mut conn, version)
            .await?
            .unwrap();
        assert_eq!(permit.context, context.id);
        assert!(permit.log_stream.is_some());
        drop(conn);
        if backend == LogStreamBackend::SharedFile {
            transport.write_file(&path, &[], Some("text/plain")).await?;
        }
        Ok(Self {
            execution,
            process_token,
            context,
            version,
            artifact: row.artifact,
            path,
            transport,
            frames: 0,
            totals: Some(FrameTotals::default()),
            _server: server,
        })
    }

    pub(super) async fn append(&mut self, fixture: &Fixture, plaintext: &[u8]) -> Result<()> {
        let frame = attune_common::runtime_read_log::encode(
            fixture
                .ctx
                .state
                .config
                .security
                .encryption_key
                .as_deref()
                .unwrap(),
            self.execution,
            self.version,
            self.frames,
            plaintext,
        )?;
        let stream =
            attune_common::repositories::log_stream::LogStreamRepository::find_by_artifact_version(
                &fixture.ctx.pool,
                self.version,
            )
            .await?
            .unwrap();
        if stream.backend == LogStreamBackend::SharedFile {
            self.transport.append_log_file(&self.path, &frame).await?;
        } else {
            // Two segments for one frame exercise independent segment/frame sequences.
            let split = frame.len() / 2;
            self.transport
                .commit_log_segment(self.version, stream.next_sequence, &frame[..split])
                .await?;
            self.transport
                .commit_log_segment(self.version, stream.next_sequence + 1, &frame[split..])
                .await?;
        }
        self.totals
            .as_mut()
            .unwrap()
            .record(&frame, plaintext.len())?;
        self.frames += 1;
        Ok(())
    }

    fn proof(&mut self, fixture: &Fixture) -> Result<String> {
        Ok(self
            .totals
            .take()
            .unwrap()
            .manifest(self.execution, self.version)?
            .authenticate(
                fixture
                    .ctx
                    .state
                    .config
                    .security
                    .encryption_key
                    .as_deref()
                    .unwrap(),
            )?)
    }

    pub(super) async fn seal(&mut self, fixture: &Fixture) -> Result<()> {
        let proof = self.proof(fixture)?;
        let expected = attune_common::runtime_log_completion::FinalLogManifest::open(
            fixture
                .ctx
                .state
                .config
                .security
                .encryption_key
                .as_deref()
                .unwrap(),
            &proof,
        )?;
        let mut tx = fixture.ctx.pool.begin().await?;
        Reads::close_execution(&mut tx, self.execution).await?;
        tx.commit().await?;
        ExecutionRepository::update(
            &fixture.ctx.pool,
            self.execution,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
        // F's WorkerDelivery credential, not the now-closed process token.
        self.transport
            .seal_log_stream_with_manifest(self.version, false, &proof)
            .await?;
        let mut conn = fixture.ctx.pool.acquire().await?;
        let completion = attune_common::repositories::runtime_log_completion::RuntimeLogCompletionRepository::find(&mut conn, self.version).await?.unwrap();
        assert_eq!(completion.attempt, self.context.attempt);
        assert_eq!(completion.worker_instance, self.context.worker_instance);
        assert_eq!(completion.frame_count, self.frames as i64);
        assert_eq!(completion.plain_bytes, expected.plaintext_bytes as i64);
        assert_eq!(completion.cipher_bytes, expected.ciphertext_bytes as i64);
        assert_eq!(completion.cipher_sha256, expected.ciphertext_sha256);
        assert_eq!(completion.authenticated_manifest, proof);
        let version = ArtifactVersionRepository::find_by_id(&mut *conn, self.version)
            .await?
            .unwrap();
        assert_eq!(
            version.body_state,
            Some(attune_common::models::ArtifactBodyState::Ready)
        );
        assert_eq!(
            version.meta.as_ref().unwrap()["encrypted_log_manifest"],
            proof
        );
        self.totals = None;
        Ok(())
    }

    async fn reader(
        &self,
        fixture: &Fixture,
        token: &str,
        artifact_route: bool,
    ) -> Result<axum::body::BodyDataStream> {
        let uri = if artifact_route {
            format!("/api/v1/artifacts/{}/stream", self.artifact)
        } else {
            format!("/api/v1/executions/{}/logs/stdout/stream", self.execution)
        };
        let response = fixture
            .ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        Ok(response.into_body().into_data_stream())
    }

    async fn acquire_key(&self, fixture: &Fixture) -> Result<Id> {
        let encryption = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        let key = KeyRepository::create(
            &fixture.ctx.pool,
            CreateKeyInput {
                local_ref: format!("live_worker_{}", self.execution),
                owner_type: attune_common::models::OwnerType::Pack,
                owner_identity: None,
                owner_pack: Some(fixture.pack_id),
                owner_pack_ref: Some("recorded_emitter".into()),
                owner_action: None,
                owner_action_ref: None,
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Worker live source".into(),
                encrypted: true,
                encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(encryption)),
                value: attune_common::crypto::encrypt_json(
                    &json!("worker-late-key-value"),
                    encryption,
                )?,
            },
        )
        .await?;
        let response = fixture
            .ctx
            .get(
                &format!("/api/v1/keys/{}", key.r#ref),
                Some(&self.process_token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await?["data"]["value"],
            "worker-late-key-value"
        );
        Ok(key.id)
    }

    async fn acquire_generic(&self, fixture: &Fixture) -> Result<Id> {
        let created = fixture
            .ctx
            .post(
                "/api/v1/artifacts",
                json!({
                    "ref":format!("recorded_emitter.live_generic_{}", self.execution),
                    "scope":"action","owner":"recorded_emitter.emit","type":"progress",
                    "data":{"message":"generic-frontier-value"}
                }),
                fixture.ctx.token.as_deref(),
            )
            .await?;
        assert_eq!(created.status(), StatusCode::CREATED);
        let artifact = created.json::<Value>().await?["data"]["id"]
            .as_i64()
            .unwrap();
        let response = fixture
            .ctx
            .get(
                &format!("/api/v1/artifacts/{artifact}"),
                Some(&self.process_token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .json::<Value>()
            .await?
            .to_string()
            .contains("generic-frontier-value"));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM execution_runtime_read_origin o JOIN runtime_authority_admission a ON a.id=o.admission WHERE a.owner_kind='execution' AND a.owner_id=$1 AND o.file_sink_version IS NULL AND o.origin->'target'->>'artifact'=$2::text")
            .bind(self.execution).bind(artifact).fetch_one(&fixture.ctx.pool).await?;
        assert_eq!(
            count, 1,
            "the producer acquired through the real generic route"
        );
        Ok(artifact)
    }
}

async fn live_worker_sequence(backend: LogStreamBackend, artifact_route: bool) -> Result<()> {
    let fixture = worker_fixture().await?;
    let (mut log, input_key) = WorkerLog::allocate_with_input(&fixture, backend).await?;
    log.append(&fixture, b"worker-first-frame").await?;
    let mut body = log.reader(&fixture, &fixture.token, artifact_route).await?;
    assert!(next_data(&mut body).await?.contains("worker-first-frame"));
    let before: Vec<(Id, Value)> = sqlx::query_as("SELECT id,origin FROM execution_runtime_read_origin WHERE admission=$1 AND file_sink_version IS NULL ORDER BY id")
        .bind(fixture.credential.admission_id).fetch_all(&fixture.ctx.pool).await?;
    assert!(before
        .last()
        .unwrap()
        .1
        .to_string()
        .contains(&format!("\"key_id\":{input_key}")));
    let key = log.acquire_key(&fixture).await?;
    log.append(&fixture, b"worker-late-key-value").await?;
    assert!(next_data(&mut body)
        .await?
        .contains("worker-late-key-value"));
    let after: Vec<(Id, Value)> = sqlx::query_as("SELECT id,origin FROM execution_runtime_read_origin WHERE admission=$1 AND file_sink_version IS NULL ORDER BY id")
        .bind(fixture.credential.admission_id).fetch_all(&fixture.ctx.pool).await?;
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(&after[..before.len()], &before);
    assert!(after
        .last()
        .unwrap()
        .1
        .to_string()
        .contains(&format!("\"key_id\":{key}")));
    log.acquire_generic(&fixture).await?;
    log.append(&fixture, b"generic-frontier-value").await?;
    assert!(next_data(&mut body)
        .await?
        .contains("generic-frontier-value"));
    let generic: Vec<(Id, Value)> = sqlx::query_as("SELECT id,origin FROM execution_runtime_read_origin WHERE admission=$1 AND file_sink_version IS NULL ORDER BY id")
        .bind(fixture.credential.admission_id).fetch_all(&fixture.ctx.pool).await?;
    assert_eq!(generic.len(), after.len() + 1);
    assert_eq!(&generic[..after.len()], &after);
    assert!(generic
        .last()
        .unwrap()
        .1
        .to_string()
        .contains("execution_runtime_read"));
    log.seal(&fixture).await?;
    assert!(next_data(&mut body).await?.contains("event: done"));
    assert!(body.try_next().await?.is_none());
    drop(body);
    drop(log);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn worker_shared_append_and_seal_feed_artifact_and_execution_live_readers() -> Result<()> {
    live_worker_sequence(LogStreamBackend::SharedFile, false).await?;
    live_worker_sequence(LogStreamBackend::SharedFile, true).await
}

#[tokio::test]
async fn worker_object_append_and_seal_feed_artifact_and_execution_live_readers() -> Result<()> {
    live_worker_sequence(LogStreamBackend::ObjectSegments, false).await?;
    live_worker_sequence(LogStreamBackend::ObjectSegments, true).await
}

#[tokio::test]
async fn human_worker_live_reader_rechecks_freeze_after_the_decoder_wait() -> Result<()> {
    for backend in [
        LogStreamBackend::SharedFile,
        LogStreamBackend::ObjectSegments,
    ] {
        let fixture = worker_fixture().await?;
        let mut log = WorkerLog::allocate(&fixture, backend).await?;
        grant_human(&fixture, exact_log_grants(&log)).await?;
        log.append(&fixture, b"human-visible-prefix").await?;
        let mut body = log
            .reader(&fixture, fixture.ctx.token.as_deref().unwrap(), false)
            .await?;
        assert!(next_data(&mut body).await?.contains("human-visible-prefix"));
        IdentityRepository::update(
            &fixture.ctx.pool,
            fixture.requester,
            UpdateIdentityInput {
                frozen: Some(true),
                ..Default::default()
            },
        )
        .await?;
        log.append(&fixture, b"human-revoked-must-not-escape")
            .await?;
        let event = next_data(&mut body).await?;
        assert!(event.contains("event: error"));
        assert!(!event.contains("human-revoked-must-not-escape"));
        drop(body);
        drop(log);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn worker_corrupt_manifest_is_rejected_by_seal_route_and_never_becomes_done() -> Result<()> {
    for backend in [
        LogStreamBackend::SharedFile,
        LogStreamBackend::ObjectSegments,
    ] {
        let fixture = worker_fixture().await?;
        let mut log = WorkerLog::allocate(&fixture, backend).await?;
        log.append(&fixture, b"authenticated-prefix").await?;
        let mut body = log.reader(&fixture, &fixture.token, false).await?;
        assert!(next_data(&mut body).await?.contains("authenticated-prefix"));
        let mut proof = log.proof(&fixture)?.into_bytes();
        proof[0] = if proof[0] == b'A' { b'B' } else { b'A' };
        assert!(log
            .transport
            .seal_log_stream_with_manifest(log.version, false, std::str::from_utf8(&proof)?)
            .await
            .is_err());
        let version = ArtifactVersionRepository::find_by_id(&fixture.ctx.pool, log.version)
            .await?
            .unwrap();
        assert_eq!(
            version.body_state,
            Some(attune_common::models::ArtifactBodyState::Pending)
        );
        // No successful seal exists. Close the reader's registry so its next
        // idle poll returns an error, rather than leaving an unbounded test tail.
        let mut tx = fixture.ctx.pool.begin().await?;
        Reads::close_execution(&mut tx, fixture.credential.execution_id).await?;
        tx.commit().await?;
        let event = next_data(&mut body).await?;
        assert!(event.contains("event: error"));
        assert!(!event.contains("event: done"));
        drop(body);
        drop(log);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

pub(super) async fn worker_fixture() -> Result<Fixture> {
    let fixture = fixture_with_additional_grants(false, false, &["read", "decrypt"], &[], &[
        json!({"resource":"artifacts","actions":["read","decrypt"],"constraints":{"owner_refs":["recorded_emitter.emit"]}}),
        json!({"resource":"executions","actions":["read","decrypt"]}),
    ]).await?;
    // Setup authority belongs to the human author, not the independently
    // admitted producer or consumer's selected API grants.
    grant_human(
        &fixture,
        vec![json!({"resource":"artifacts","actions":["create"],"constraints":{"owner_types":["action"],"owner_refs":["recorded_emitter.emit"]}})],
    )
    .await?;
    Ok(fixture)
}

pub(super) async fn grant_human(fixture: &Fixture, grants: Vec<Value>) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let set = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("fixture.worker_setup_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: Value::Array(grants),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(fixture.requester)
        .await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

/// Capture a new independent admission. Neither the fixture's existing reader
/// admission nor the producer's selected grants are rewritten or unioned.
pub(super) async fn fixed_reader(
    fixture: &Fixture,
    grants: Vec<Value>,
    lifetime: Option<chrono::Duration>,
) -> Result<(RuntimeCredentialBinding, String, Id)> {
    // Issuing a fixed approval requires the human issuer to cover these exact
    // resources. This is setup authority, not the reader's runtime grant source.
    // Keep it separate from the independent recorded admission below.
    grant_human(fixture, grants.clone()).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let original =
        RuntimeAuthorityRepository::execution_admission(&mut tx, fixture.credential.execution_id)
            .await?
            .unwrap();
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap();
    let grants = grants
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<Vec<attune_common::rbac::Grant>, _>>()?;
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        grants.clone(),
        lifetime.map(|duration| chrono::Utc::now() + duration),
    )
    .await?;
    // This is a fixed effective approval, not authored permission YAML. The
    // canonical external constructor retains its exact IDs and approval link.
    let set = attune_common::runtime_authority::AdmittedPermissionSet::external(
        "recorded_emitter.fixed_live_reader",
        &policy.policy.owner,
        &grants,
        &[approval.approval()?],
    )?;
    let mut next = policy.policy.clone();
    next.invocations
        .iter_mut()
        .find(|invocation| invocation.binding == original.authority.binding)
        .unwrap()
        .permission_sets
        .insert(set.reference.clone(), set);
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &next).await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    let refs = vec!["recorded_emitter.fixed_live_reader".to_owned()];
    let selected = policy.policy.select(
        policy.id,
        &original.authority.binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let pin = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(pin.executable.action.id),
            action_ref: pin.executable.action.r#ref.clone(),
            executor: Some(fixture.requester),
            config: Some(json!({})),
            permission_set_refs: refs.clone(),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
        &pin,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &pin).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
        .await?;
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: pin.executable.action.r#ref.clone(),
        attempt: selected.attempt,
    };
    tx.commit().await?;
    let token = generate_admitted_execution_token(
        &credential,
        &refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    Ok((credential, token, approval.id))
}

pub(super) fn exact_log_grants(log: &WorkerLog) -> Vec<Value> {
    vec![
        json!({"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[log.execution]}}),
        json!({"resource":"artifacts","actions":["read","decrypt"],"constraints":{"ids":[log.artifact]}}),
    ]
}

async fn foreign_producer(fixture: &Fixture) -> Result<(Id, String)> {
    use attune_common::repositories::pack::PackRepository;
    let pack = PackRepository::find_by_ref(&fixture.ctx.pool, "foreign_events")
        .await?
        .unwrap();
    let action = create_test_action(
        &fixture.ctx.pool,
        pack.id,
        &pack.r#ref,
        "foreign_events.live_produce",
    )
    .await?;
    activate_test_pack_release_with_projections(
        &fixture.ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id).await?;
    let owner = RuntimeAuthorityRepository::find_owner(&mut tx, pack.id)
        .await?
        .unwrap();
    let pin = ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, &action.r#ref).await?;
    let definition: PermissionSetDefinition = serde_json::from_value(json!({
        "ref":"foreign_events.live_producer", "grants":[{"resource":"keys","actions":["read","decrypt"]}]
    }))?;
    let set = definition.compile(&owner, &[])?;
    let binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        owner,
        release_id: pin.release.id,
        release_digest: pin.release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: vec![InvocationPolicy {
            binding: binding.clone(),
            metadata_revision: None,
            foreign_workflow_policy: None,
            source_id: None,
            dispatch_template_digest: None,
            target_release_id: pin.release.id,
            target_release_digest: pin.release.digest.clone(),
            permission_sets: BTreeMap::from([(set.reference.clone(), set)]),
            standard_access: false,
        }],
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    let refs = vec!["foreign_events.live_producer".to_owned()];
    let selected = policy.policy.select(
        policy.id,
        &binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(fixture.requester),
            config: Some(json!({})),
            status: ExecutionStatus::Running,
            permission_set_refs: refs.clone(),
            ..Default::default()
        },
        &pin,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &pin).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
        .await?;
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    let credential = RuntimeCredentialBinding {
        execution_id: execution.id,
        admission_id: admission.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: action.r#ref,
        attempt: selected.attempt,
    };
    tx.commit().await?;
    Ok((
        execution.id,
        generate_admitted_execution_token(
            &credential,
            &refs,
            &fixture.ctx.state.jwt_config,
            Some(360),
        )?,
    ))
}

#[tokio::test]
async fn foreign_worker_log_uses_fixed_execution_artifact_and_key_origin_approval() -> Result<()> {
    for backend in [
        LogStreamBackend::SharedFile,
        LogStreamBackend::ObjectSegments,
    ] {
        let fixture = worker_fixture().await?;
        let (producer, token) = foreign_producer(&fixture).await?;
        let mut log = WorkerLog::for_execution(&fixture, producer, token, backend).await?;
        let foreign = attune_common::repositories::pack::PackRepository::find_by_ref(
            &fixture.ctx.pool,
            "foreign_events",
        )
        .await?
        .unwrap();
        let encryption = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        let key = KeyRepository::create(
            &fixture.ctx.pool,
            CreateKeyInput {
                local_ref: "fixed_foreign_live_source".into(),
                owner_type: attune_common::models::OwnerType::Pack,
                owner_identity: None,
                owner_pack: Some(foreign.id),
                owner_pack_ref: Some(foreign.r#ref),
                owner_action: None,
                owner_action_ref: None,
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Fixed foreign source".into(),
                encrypted: true,
                encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(encryption)),
                value: attune_common::crypto::encrypt_json(
                    &json!("fixed-foreign-source-value"),
                    encryption,
                )?,
            },
        )
        .await?;
        let mut grants = exact_log_grants(&log);
        grants.push(
            json!({"resource":"keys","actions":["read","decrypt"],"constraints":{"ids":[key.id]}}),
        );
        let (reader, token, approval) = fixed_reader(&fixture, grants, None).await?;
        let mut conn = fixture.ctx.pool.acquire().await?;
        let producer_admission =
            RuntimeAuthorityRepository::execution_admission(&mut conn, producer)
                .await?
                .unwrap();
        assert_ne!(
            reader.sponsor_identity_id,
            producer_admission.authority.owner.sponsor_identity_id
        );
        drop(conn);
        log.append(&fixture, b"foreign-positive-frame").await?;
        let mut body = log.reader(&fixture, &token, false).await?;
        assert!(next_data(&mut body)
            .await?
            .contains("foreign-positive-frame"));
        let rows: Vec<Value> = sqlx::query_scalar("SELECT origin FROM execution_runtime_read_origin WHERE admission=$1 AND file_sink_version IS NULL")
            .bind(reader.admission_id).fetch_all(&fixture.ctx.pool).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["target"]["execution"], producer);
        assert_eq!(rows[0]["target"]["artifact"], log.artifact);
        let read = fixture
            .ctx
            .get(
                &format!("/api/v1/keys/{}", key.r#ref),
                Some(&log.process_token),
            )
            .await?;
        assert_eq!(read.status(), StatusCode::OK);
        assert_eq!(
            read.json::<Value>().await?["data"]["value"],
            "fixed-foreign-source-value"
        );
        log.append(&fixture, b"fixed-foreign-source-value").await?;
        assert!(next_data(&mut body)
            .await?
            .contains("fixed-foreign-source-value"));
        let acquired: Vec<Value> = sqlx::query_scalar("SELECT origin FROM execution_runtime_read_origin WHERE admission=$1 AND file_sink_version IS NULL ORDER BY id")
            .bind(reader.admission_id).fetch_all(&fixture.ctx.pool).await?;
        assert_eq!(acquired.len(), 2);
        assert_eq!(acquired[0], rows[0]);
        assert!(acquired[1]
            .to_string()
            .contains(&format!("\"key_id\":{key}", key = key.id)));
        log.seal(&fixture).await?;
        assert!(next_data(&mut body).await?.contains("event: done"));
        let mut tx = fixture.ctx.pool.begin().await?;
        RuntimeAuthorityRepository::revoke_approval(&mut tx, approval).await?;
        tx.commit().await?;
        drop(body);
        drop(log);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn same_sponsor_reader_revocation_does_not_revoke_independent_worker_producer() -> Result<()>
{
    let fixture = worker_fixture().await?;
    let mut log = WorkerLog::allocate(&fixture, LogStreamBackend::ObjectSegments).await?;
    let (reader, token, _) = fixed_reader(&fixture, exact_log_grants(&log), None).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let producer = RuntimeAuthorityRepository::execution_admission(&mut tx, log.execution)
        .await?
        .unwrap();
    assert_eq!(
        reader.sponsor_identity_id,
        producer.authority.owner.sponsor_identity_id
    );
    assert_ne!(reader.admission_id, producer.id);
    tx.commit().await?;
    log.append(&fixture, b"independent-reader-first").await?;
    let mut body = log.reader(&fixture, &token, false).await?;
    assert!(next_data(&mut body)
        .await?
        .contains("independent-reader-first"));
    let mut revoke = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut revoke, reader.admission_id).await?;
    revoke.commit().await?;
    log.append(&fixture, b"reader-revoked-worker-still-authorized")
        .await?;
    let event = next_data(&mut body).await?;
    assert!(event.contains("event: error"));
    assert!(!event.contains("reader-revoked-worker-still-authorized"));
    log.seal(&fixture).await?;
    drop(body);
    drop(log);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn reciprocal_worker_readers_wait_on_low_capture_first_and_closure_denies_only_its_consumer(
) -> Result<()> {
    let fixture = worker_fixture().await?;
    let mut first = WorkerLog::allocate(&fixture, LogStreamBackend::ObjectSegments).await?;
    let mut second = WorkerLog::allocate(&fixture, LogStreamBackend::ObjectSegments).await?;
    first.append(&fixture, b"first-origin-prefix").await?;
    second.append(&fixture, b"second-origin-prefix").await?;
    let mut read_first = first.reader(&fixture, &second.process_token, false).await?;
    assert!(next_data(&mut read_first)
        .await?
        .contains("first-origin-prefix"));
    let mut read_second = second.reader(&fixture, &first.process_token, false).await?;
    assert!(next_data(&mut read_second)
        .await?
        .contains("second-origin-prefix"));
    first
        .append(&fixture, b"closed-producer-still-readable")
        .await?;
    second
        .append(&fixture, b"closed-consumer-must-not-escape")
        .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let a = RuntimeAuthorityRepository::execution_admission(&mut tx, first.execution)
        .await?
        .unwrap();
    let b = RuntimeAuthorityRepository::execution_admission(&mut tx, second.execution)
        .await?
        .unwrap();
    let low = a.id.min(b.id);
    let high = a.id.max(b.id);
    assert_eq!(
        low, a.id,
        "fixture insertion order defines the consumer being closed"
    );
    Reads::lock_open(&mut tx, low).await?;
    let next_first = tokio::spawn(async move { next_data(&mut read_first).await.unwrap() });
    let next_second = tokio::spawn(async move { next_data(&mut read_second).await.unwrap() });
    let _polls = CancelPolls(vec![next_first.abort_handle(), next_second.abort_handle()]);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND cardinality(pg_blocking_pids(pid))>0 AND query LIKE '%execution_runtime_read_fence%' AND pid<>pg_backend_pid()")
                .fetch_one(&fixture.ctx.pool).await.unwrap();
            if waiting >= 2 { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    // If either reader took its own high-ID fence first, NOWAIT fails here.
    let mut inspect = fixture.ctx.pool.begin().await?;
    let available: Id = sqlx::query_scalar(
        "SELECT admission FROM execution_runtime_read_fence WHERE admission=$1 FOR UPDATE NOWAIT",
    )
    .bind(high)
    .fetch_one(&mut *inspect)
    .await?;
    assert_eq!(available, high);
    inspect.rollback().await?;
    Reads::close_execution(&mut tx, first.execution).await?;
    tx.commit().await?;
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), next_first).await??;
    assert!(event.contains("closed-producer-still-readable"));
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), next_second).await??;
    assert!(event.contains("event: error"));
    assert!(!event.contains("closed-consumer-must-not-escape"));
    drop(first);
    drop(second);
    fixture.ctx.cleanup().await?;
    Ok(())
}

async fn expiry_after_decoder_wait(empty_plaintext_eof: bool) -> Result<()> {
    use super::live_decoder_gate::{DecoderGate, WaitAt};
    let mut fixture = worker_fixture().await?;
    let mut log = WorkerLog::allocate(&fixture, LogStreamBackend::ObjectSegments).await?;
    log.append(
        &fixture,
        if empty_plaintext_eof {
            b""
        } else {
            b"expired-decoded-value-must-not-escape"
        },
    )
    .await?;
    log.seal(&fixture).await?;
    let (reader, token, approval) = fixed_reader(
        &fixture,
        exact_log_grants(&log),
        Some(chrono::Duration::seconds(5)),
    )
    .await?;
    let stream =
        attune_common::repositories::log_stream::LogStreamRepository::find_by_artifact_version(
            &fixture.ctx.pool,
            log.version,
        )
        .await?
        .unwrap();
    let segment =
        attune_common::repositories::log_stream::LogStreamRepository::find_segment_by_sequence(
            &fixture.ctx.pool,
            stream.id,
            if empty_plaintext_eof { 1 } else { 0 },
        )
        .await?
        .unwrap();
    let arrived = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let gate = Arc::new(DecoderGate {
        inner: fixture.ctx.state.blob_store.clone(),
        key: segment.object_key,
        at: if empty_plaintext_eof {
            WaitAt::Eof
        } else {
            WaitAt::FirstRead
        },
        arrived: arrived.clone(),
        release: release.clone(),
    });
    let mut state = fixture.ctx.state.as_ref().clone();
    state.blob_store = gate;
    fixture.ctx.state = Arc::new(state);
    fixture.ctx.app = attune_api::server::Server::new(fixture.ctx.state.clone()).router();
    let mut body = log.reader(&fixture, &token, false).await?;
    let next = tokio::spawn(async move { next_data(&mut body).await.unwrap() });
    let _poll = CancelPolls(vec![next.abort_handle()]);
    tokio::time::timeout(std::time::Duration::from_secs(10), arrived.acquire())
        .await??
        .forget();
    assert!(
        !next.is_finished(),
        "actual decoder I/O is held, not a synthetic authorized bool"
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp() >= expires_at FROM runtime_external_approval WHERE id=$1",
            )
            .bind(approval)
            .fetch_one(&fixture.ctx.pool)
            .await
            .unwrap();
            if expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    release.add_permits(1);
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), next).await??;
    assert!(event.contains("event: error"));
    assert!(!event.contains("expired-decoded-value-must-not-escape"));
    assert!(!event.contains("event: done"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM execution_runtime_read_origin WHERE admission=$1 AND file_sink_version IS NULL")
        .bind(reader.admission_id).fetch_one(&fixture.ctx.pool).await?;
    assert_eq!(
        count, 1,
        "expiry cannot capture another origin-set revision"
    );
    drop(log);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn worker_live_chunk_expiry_follows_the_actual_blob_decoder_wait() -> Result<()> {
    expiry_after_decoder_wait(false).await
}

#[tokio::test]
async fn worker_live_empty_plaintext_eof_expiry_follows_the_actual_blob_eof_wait() -> Result<()> {
    expiry_after_decoder_wait(true).await
}

#[tokio::test]
async fn worker_live_input_origin_deletion_denies_the_next_frame_on_both_backends() -> Result<()> {
    for backend in [
        LogStreamBackend::SharedFile,
        LogStreamBackend::ObjectSegments,
    ] {
        let fixture = worker_fixture().await?;
        let (mut log, input_key) = WorkerLog::allocate_with_input(&fixture, backend).await?;
        log.append(&fixture, b"accepted-input-prefix").await?;
        let mut body = log.reader(&fixture, &fixture.token, false).await?;
        assert!(next_data(&mut body)
            .await?
            .contains("accepted-input-prefix"));
        assert!(KeyRepository::delete(&fixture.ctx.pool, input_key).await?);
        log.append(&fixture, b"revoked-input-must-not-escape")
            .await?;
        let event = next_data(&mut body).await?;
        assert!(event.contains("event: error"));
        assert!(!event.contains("revoked-input-must-not-escape"));
        assert!(!event.contains("event: done"));
        drop(body);
        drop(log);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn worker_live_fixed_reader_cannot_borrow_producers_later_generic_source_grants() -> Result<()>
{
    let fixture = worker_fixture().await?;
    let mut log = WorkerLog::allocate(&fixture, LogStreamBackend::ObjectSegments).await?;
    let (_, reader_token, _) = fixed_reader(&fixture, exact_log_grants(&log), None).await?;
    log.append(&fixture, b"fixed-reader-prefix").await?;
    let mut body = log.reader(&fixture, &reader_token, false).await?;
    assert!(next_data(&mut body).await?.contains("fixed-reader-prefix"));
    log.acquire_generic(&fixture).await?;
    log.append(&fixture, b"unapproved-generic-value-must-not-escape")
        .await?;
    let event = next_data(&mut body).await?;
    assert!(event.contains("event: error"));
    assert!(!event.contains("unapproved-generic-value-must-not-escape"));
    assert!(!event.contains("event: done"));
    drop(body);
    drop(log);
    fixture.ctx.cleanup().await?;
    Ok(())
}
