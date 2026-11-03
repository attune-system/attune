//! Reader integration fixtures publish F-compatible frames/proofs into real
//! shared streams. They do not replace F's separately tested Worker seal permit.
use super::*;
use attune_common::{
    models::{
        enums::LogStreamBackend, ArtifactClassification, ArtifactType, ArtifactVisibility,
        OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        key::{CreateKeyInput, KeyRepository},
        log_stream::LogStreamRepository,
        runtime_read::RuntimeReadRepository as Reads,
        FindById,
    },
    runtime_log_completion::FrameTotals,
    runtime_read_log::{encode, ENCODING},
};
use futures::TryStreamExt;
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;

struct PendingLog {
    producer: Id,
    token: String,
    artifact: Id,
    version: Id,
    stream: Id,
    path: std::path::PathBuf,
    frames: u64,
    ciphertext: u64,
    totals: Option<FrameTotals>,
}

pub(super) async fn admit_producer(fixture: &Fixture) -> Result<(Id, String)> {
    admit_producer_with_input(fixture, None).await
}

pub(super) async fn admit_producer_with_input(
    fixture: &Fixture,
    input_key: Option<&str>,
) -> Result<(Id, String)> {
    let mut tx = fixture.ctx.pool.begin().await?;
    let original =
        RuntimeAuthorityRepository::execution_admission(&mut tx, fixture.credential.execution_id)
            .await?
            .unwrap();
    let revision = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap();
    let executable = ExecutableSnapshotRepository::resolve_for_action_ref(
        &mut *tx,
        &fixture.credential.action_ref,
    )
    .await?;
    let definition: PermissionSetDefinition = serde_json::from_value(
        json!({"ref":"recorded_emitter.live_producer", "grants":[
            {"resource":"keys","actions":["read","decrypt"]},
            {"resource":"artifacts","actions":["read","decrypt"],"constraints":{"owner_refs":["recorded_emitter.emit"]}},
            {"resource":"executions","actions":["read","decrypt"]}
        ]}),
    )?;
    let set = definition.compile(&revision.policy.owner, &[])?;
    let mut next = revision.policy.clone();
    next.invocations
        .iter_mut()
        .find(|invocation| invocation.binding == original.authority.binding)
        .unwrap()
        .permission_sets
        .insert(set.reference.clone(), set);
    let revision = RuntimeAuthorityRepository::capture_policy(&mut tx, &next).await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, revision.id).await?;
    let refs = vec!["recorded_emitter.live_producer".to_owned()];
    let selected = revision.policy.select(
        revision.id,
        &original.authority.binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let producer = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(executable.executable.action.id),
            action_ref: fixture.credential.action_ref.clone(),
            executor: Some(fixture.requester),
            config: Some(json!({})),
            permission_set_refs: refs.clone(),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
        &executable,
    )
    .await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &executable).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, producer.id, &binding)
        .await?;
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(producer.id),
        &selected,
    )
    .await?;
    let credential = RuntimeCredentialBinding {
        execution_id: producer.id,
        admission_id: admission.id,
        sponsor_identity_id: selected.owner.sponsor_identity_id,
        action_ref: fixture.credential.action_ref.clone(),
        attempt: selected.attempt,
    };
    if let Some(reference) = input_key {
        use attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository;
        // This is accepted input-purpose capture, separate from selected API
        // grants. Resolve with B's native contributor helper before publication.
        RecordedInputAuthorityRepository::capture_human(
            &mut tx,
            "execution",
            producer.id,
            fixture.requester,
        )
        .await?;
        let authority = RecordedInputAuthorityRepository::load(&mut tx, "execution", producer.id)
            .await?
            .unwrap();
        let keys = RecordedInputAuthorityRepository::resolve_keys(
            &mut tx,
            &[authority],
            &[reference.to_owned()],
            fixture.ctx.state.config.security.encryption_key.as_deref(),
        )
        .await?;
        let input = keys.get(reference).unwrap();
        let provenance =
            attune_common::secret_provenance::SecretProvenance::key(input.origin.clone());
        let prepared = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/accepted_input".into(),
                value: input.value.clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&provenance)?),
            }],
            fixture
                .ctx
                .state
                .config
                .security
                .encryption_key
                .as_deref()
                .unwrap(),
        )?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            &mut tx,
            ENTITY_EXECUTION_CONFIG,
            producer.id,
            &prepared,
        )
        .await?;
    }
    tx.commit().await?;
    let token = generate_admitted_execution_token(
        &credential,
        &refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    Ok((producer.id, token))
}

impl PendingLog {
    async fn create(fixture: &Fixture) -> Result<Self> {
        let (producer, token) = admit_producer(fixture).await?;
        let execution = ExecutionRepository::find_by_id(&fixture.ctx.pool, producer)
            .await?
            .unwrap();
        let reference = attune_common::worker_delivery::log_reference(&execution, "stdout")?;
        let artifact = ArtifactRepository::create(
            &fixture.ctx.pool,
            CreateArtifactInput {
                r#ref: reference,
                scope: OwnerType::Action,
                owner: fixture.credential.action_ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Public,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 5,
                name: None,
                description: None,
                content_type: Some("text/plain".into()),
                data: None,
            },
        )
        .await?;
        let version = ArtifactVersionRepository::create_log_pending(
            &fixture.ctx.pool,
            artifact.id,
            &artifact.r#ref,
            LogStreamBackend::SharedFile,
            "text/plain".into(),
            Some(producer),
            Some(json!({"log_encoding":ENCODING})),
            None,
        )
        .await?;
        let stream = LogStreamRepository::create_with_backend(
            &fixture.ctx.pool,
            version.id,
            LogStreamBackend::SharedFile,
            65536,
            100,
        )
        .await?;
        let path = std::path::Path::new(&fixture.ctx.state.config.artifacts_dir)
            .join(version.file_path.unwrap());
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        tokio::fs::write(&path, []).await?;
        Ok(Self {
            producer,
            token,
            artifact: artifact.id,
            version: version.id,
            stream: stream.id,
            path,
            frames: 0,
            ciphertext: 0,
            totals: Some(FrameTotals::default()),
        })
    }

    async fn append(&mut self, fixture: &Fixture, plaintext: &[u8]) -> Result<()> {
        let key = fixture
            .ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .unwrap();
        let frame = encode(key, self.producer, self.version, self.frames, plaintext)?;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(&frame).await?;
        file.flush().await?;
        self.totals
            .as_mut()
            .unwrap()
            .record(&frame, plaintext.len())?;
        self.frames += 1;
        self.ciphertext += frame.len() as u64;
        Ok(())
    }

    async fn seal(&mut self, fixture: &Fixture) -> Result<()> {
        let manifest = self
            .totals
            .take()
            .unwrap()
            .manifest(self.producer, self.version)?;
        let proof = manifest.authenticate(
            fixture
                .ctx
                .state
                .config
                .security
                .encryption_key
                .as_deref()
                .unwrap(),
        )?;
        let mut tx = fixture.ctx.pool.begin().await?;
        Reads::close_execution(&mut tx, self.producer).await?;
        sqlx::query("UPDATE artifact_version SET body_state='ready',size_bytes=$2,sha256=$3,meta=$4 WHERE id=$1")
            .bind(self.version).bind(self.ciphertext as i64).bind(&manifest.ciphertext_sha256)
            .bind(json!({"log_encoding":ENCODING,"encrypted_log_manifest":proof})).execute(&mut *tx).await?;
        LogStreamRepository::seal_shared_file(&mut tx, self.stream, self.ciphertext as i64, false)
            .await?;
        ExecutionRepository::update(
            &mut *tx,
            self.producer,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn response(
        &self,
        fixture: &Fixture,
        execution_route: bool,
    ) -> Result<axum::response::Response> {
        let path = if execution_route {
            format!("/api/v1/executions/{}/logs/stdout/stream", self.producer)
        } else {
            format!("/api/v1/artifacts/{}/stream", self.artifact)
        };
        let request = axum::http::Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {}", fixture.token))
            .body(axum::body::Body::empty())?;
        Ok(fixture.ctx.app.clone().oneshot(request).await.unwrap())
    }
}

pub(super) async fn fixture_for_live(keys: &[&str]) -> Result<Fixture> {
    fixture_with_additional_grants(
        false,
        false,
        keys,
        &[],
        &[
            json!({"resource":"artifacts","actions":["read","decrypt"]}),
            json!({"resource":"executions","actions":["read","decrypt"]}),
        ],
    )
    .await
}

pub(super) async fn next_data(body: &mut axum::body::BodyDataStream) -> Result<String> {
    Ok(
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let bytes = body
                    .try_next()
                    .await
                    .unwrap()
                    .expect("stream ended before its next data event");
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if text.contains("event: content")
                    || text.contains("event: append")
                    || text.contains("event: done")
                    || text.contains("event: error")
                    || text.starts_with("data:")
                {
                    return text;
                }
            }
        })
        .await?,
    )
}

async fn late_key(fixture: &Fixture, log: &PendingLog) -> Result<Id> {
    let encryption_key = fixture
        .ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap();
    let key = KeyRepository::create_encrypted_in_pool(
        &fixture.ctx.pool,
        CreateKeyInput {
            local_ref: "live_late_key".into(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(fixture.pack_id),
            owner_pack_ref: Some("recorded_emitter".into()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Late SDK key".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: json!("late-key-value"),
        },
        encryption_key,
    )
    .await?;
    let response = fixture
        .ctx
        .get(&format!("/api/v1/keys/{}", key.r#ref), Some(&log.token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await?["data"]["value"],
        "late-key-value"
    );
    Ok(key.id)
}

async fn revisions(fixture: &Fixture) -> Result<Vec<(Id, Value, String)>> {
    Ok(sqlx::query_as("SELECT id,origin,origin_digest FROM execution_runtime_read_origin WHERE admission=$1 AND file_sink_version IS NULL AND origin->'target'->>'kind'='live_log' ORDER BY id")
        .bind(fixture.credential.admission_id).fetch_all(&fixture.ctx.pool).await?)
}

async fn live_late_sdk_case(execution_route: bool) -> Result<()> {
    let fixture = fixture_for_live(&["read", "decrypt"]).await?;
    let mut log = PendingLog::create(&fixture).await?;
    log.append(&fixture, b"first-frame").await?;
    let response = log.response(&fixture, execution_route).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(next_data(&mut body).await?.contains("first-frame"));
    let before = revisions(&fixture).await?;
    assert_eq!(before.len(), 1);
    // The next source capability is acquired through the real producer HTTP SDK,
    // after the first frame has already reached the independently admitted reader.
    let key = late_key(&fixture, &log).await?;
    log.append(&fixture, b"late-key-value").await?;
    assert!(next_data(&mut body).await?.contains("late-key-value"));
    let after = revisions(&fixture).await?;
    assert_eq!(after.len(), 2);
    assert_eq!(
        after[0], before[0],
        "earlier revision must never acquire wider parents"
    );
    assert!(after[1]
        .1
        .to_string()
        .contains(&format!("\"key_id\":{key}")));
    log.seal(&fixture).await?;
    assert!(next_data(&mut body).await?.contains("event: done"));
    assert!(body.try_next().await?.is_none());
    assert_eq!(
        revisions(&fixture).await?.len(),
        2,
        "unchanged frontiers must deduplicate across frames and final proof"
    );
    let closed = fixture
        .ctx
        .get(
            "/api/v1/keys/recorded_emitter.live_late_key",
            Some(&log.token),
        )
        .await?;
    assert_eq!(
        closed.status(),
        StatusCode::UNAUTHORIZED,
        "closed producer gets no terminal process grace"
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn artifact_live_tail_captures_late_sdk_frontier_revisions_and_verifies_seal() -> Result<()> {
    live_late_sdk_case(false).await
}

#[tokio::test]
async fn execution_live_tail_captures_late_sdk_frontier_revisions_and_verifies_seal() -> Result<()>
{
    live_late_sdk_case(true).await
}

#[tokio::test]
async fn live_tail_denies_late_sdk_key_when_reader_lacks_decrypt_without_widening_earlier_revision(
) -> Result<()> {
    let fixture = fixture_for_live(&["read"]).await?;
    let mut log = PendingLog::create(&fixture).await?;
    log.append(&fixture, b"safe-before-sdk").await?;
    let response = log.response(&fixture, true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(next_data(&mut body).await?.contains("safe-before-sdk"));
    let before = revisions(&fixture).await?;
    late_key(&fixture, &log).await?;
    log.append(&fixture, b"late-key-value").await?;
    let denied = next_data(&mut body).await?;
    assert!(denied.contains("event: error"));
    assert!(!denied.contains("late-key-value"));
    assert!(!denied.contains("event: done"));
    assert_eq!(revisions(&fixture).await?, before);
    drop(body);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn live_execution_tail_rechecks_current_admission_at_actual_next_frame_revocation_barrier(
) -> Result<()> {
    let fixture = fixture_for_live(&["read", "decrypt"]).await?;
    let mut log = PendingLog::create(&fixture).await?;
    log.append(&fixture, b"first-live-frame").await?;
    let response = log.response(&fixture, true).await?;
    let mut body = response.into_body().into_data_stream();
    assert!(next_data(&mut body).await?.contains("first-live-frame"));
    let mut revoke = fixture.ctx.pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *revoke)
        .await?;
    RuntimeAuthorityRepository::revoke_admission(&mut revoke, fixture.credential.admission_id)
        .await?;
    log.append(&fixture, b"must-not-escape-after-revocation")
        .await?;
    let next = tokio::spawn(async move {
        loop {
            let Some(bytes) = body.try_next().await.unwrap() else {
                panic!("missing revocation error");
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.contains("event: error") || text.contains("must-not-escape") {
                return text;
            }
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&fixture.ctx.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await?;
    revoke.commit().await?;
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), next).await??;
    assert!(event.contains("event: error"));
    assert!(!event.contains("must-not-escape-after-revocation"));
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_metadata_stream_rebuilds_typed_projection_and_revalidates_each_candidate(
) -> Result<()> {
    let fixture = fixture_with_additional_grants(
        false,
        false,
        &["read"],
        &[],
        &[json!({"resource":"executions","actions":["read"]})],
    )
    .await?;
    ExecutionRepository::update(
        &fixture.ctx.pool,
        fixture.credential.execution_id,
        UpdateExecutionInput {
            result: Some(json!({"private":"stored-result-must-not-escape"})),
            ..Default::default()
        },
    )
    .await?;
    let request = axum::http::Request::builder()
        .uri(format!(
            "/api/v1/executions/stream?execution_id={}",
            fixture.credential.execution_id
        ))
        .header("authorization", format!("Bearer {}", fixture.token))
        .body(axum::body::Body::empty())?;
    let response = fixture.ctx.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    fixture.ctx.state.broadcast_tx.send(json!({"entity_type":"execution","entity_id":fixture.credential.execution_id,
        "payload":{"config":"broadcast-config-must-not-escape","result":"broadcast-result-must-not-escape"}}).to_string())?;
    let event = next_data(&mut body).await?;
    assert!(event.contains("recorded_emitter.emit"));
    assert!(!event.contains("must-not-escape"));
    // Use a committed revocation, not a reusable initial-connect boolean.
    let mut revoke = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut revoke, fixture.credential.admission_id)
        .await?;
    revoke.commit().await?;
    fixture.ctx.state.broadcast_tx.send(json!({"entity_type":"execution","entity_id":fixture.credential.execution_id,"payload":{"result":"queued-private-result"}}).to_string())?;
    let event = next_data(&mut body).await?;
    assert!(event.contains("event: error"));
    assert!(!event.contains("queued-private-result"));
    drop(body);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn live_tail_consumer_closure_prevents_next_frame_and_done() -> Result<()> {
    let fixture = fixture_for_live(&["read", "decrypt"]).await?;
    let mut log = PendingLog::create(&fixture).await?;
    log.append(&fixture, b"before-close").await?;
    let response = log.response(&fixture, true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(next_data(&mut body).await?.contains("before-close"));
    let mut tx = fixture.ctx.pool.begin().await?;
    Reads::close_execution(&mut tx, fixture.credential.execution_id).await?;
    tx.commit().await?;
    log.append(&fixture, b"after-close-must-not-escape").await?;
    log.seal(&fixture).await?;
    let event = next_data(&mut body).await?;
    assert!(event.contains("event: error"));
    assert!(!event.contains("after-close-must-not-escape"));
    assert!(!event.contains("event: done"));
    drop(body);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn live_tail_truncated_final_frame_never_emits_done() -> Result<()> {
    let fixture = fixture_for_live(&["read", "decrypt"]).await?;
    let mut log = PendingLog::create(&fixture).await?;
    log.append(&fixture, b"verified-prefix").await?;
    let response = log.response(&fixture, false).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(next_data(&mut body).await?.contains("verified-prefix"));
    log.append(&fixture, b"truncated-final-frame").await?;
    // Keep F's proof for the complete frame, but truncate the backing file.
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&log.path)
        .await?;
    file.set_len(log.ciphertext - 1).await?;
    log.seal(&fixture).await?;
    let event = next_data(&mut body).await?;
    assert!(event.contains("event: error"));
    assert!(!event.contains("event: done"));
    drop(body);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn self_live_tail_deduplicates_carriers_but_retains_later_key_receipts() -> Result<()> {
    let fixture = fixture_for_live(&["read", "decrypt"]).await?;
    let mut log = PendingLog::create(&fixture).await?;
    log.append(&fixture, b"self-first").await?;
    let request = axum::http::Request::builder()
        .uri(format!("/api/v1/artifacts/{}/stream", log.artifact))
        .header("authorization", format!("Bearer {}", log.token))
        .body(axum::body::Body::empty())?;
    let response = fixture.ctx.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(next_data(&mut body).await?.contains("self-first"));
    log.append(&fixture, b"self-second").await?;
    assert!(next_data(&mut body).await?.contains("self-second"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM execution_runtime_read_origin o JOIN runtime_authority_admission a ON a.id=o.admission WHERE a.owner_kind='execution' AND a.owner_id=$1 AND o.file_sink_version IS NULL")
        .bind(log.producer).fetch_one(&fixture.ctx.pool).await?;
    assert_eq!(
        count, 1,
        "unchanged self-tail is not a recursive acquisition"
    );
    let key = late_key(&fixture, &log).await?;
    log.append(&fixture, b"late-key-value").await?;
    assert!(next_data(&mut body).await?.contains("late-key-value"));
    let origins: Vec<Value> = sqlx::query_scalar("SELECT o.origin FROM execution_runtime_read_origin o JOIN runtime_authority_admission a ON a.id=o.admission WHERE a.owner_kind='execution' AND a.owner_id=$1 AND o.file_sink_version IS NULL ORDER BY o.id")
        .bind(log.producer).fetch_all(&fixture.ctx.pool).await?;
    assert_eq!(origins.len(), 2);
    assert!(origins[1]
        .to_string()
        .contains(&format!("\"key_id\":{key}")));
    log.seal(&fixture).await?;
    let event = next_data(&mut body).await?;
    assert!(
        event.contains("event: error"),
        "closing one's own acquisition registry ends process-token delivery"
    );
    assert!(!event.contains("event: done"));
    drop(body);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn live_tail_rechecks_retained_key_existence_after_first_authorized_frame() -> Result<()> {
    use attune_common::repositories::Delete;
    let fixture = fixture_for_live(&["read", "decrypt"]).await?;
    let mut log = PendingLog::create(&fixture).await?;
    let key = late_key(&fixture, &log).await?;
    log.append(&fixture, b"authorized-key-frame").await?;
    let response = log.response(&fixture, true).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(next_data(&mut body).await?.contains("authorized-key-frame"));
    assert!(KeyRepository::delete(&fixture.ctx.pool, key).await?);
    log.append(&fixture, b"deleted-key-frame-must-not-escape")
        .await?;
    let event = next_data(&mut body).await?;
    assert!(event.contains("event: error"));
    assert!(!event.contains("deleted-key-frame-must-not-escape"));
    assert!(!event.contains("event: done"));
    drop(body);
    fixture.ctx.cleanup().await?;
    Ok(())
}
