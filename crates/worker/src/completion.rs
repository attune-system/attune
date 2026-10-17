//! Worker-only delivery transports. These credentials never enter action envs.
use attune_common::{
    artifact_transport::{ApiTransport, ArtifactFileTransport, BoxAsyncReader},
    auth::jwt::JwtConfig,
    models::{
        ArtifactType, ArtifactVisibility, Execution, ExecutionExecutableSnapshot, LogStreamBackend,
        OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        execution::ExecutionRepository,
        execution_admission::ExecutionAdmissionRepository,
        log_stream::LogStreamRepository,
        runtime_authority::RuntimeOwner,
        worker_delivery::{
            DeliveryContext, DeliveryOperation, DeliveryPermit, WorkerDeliveryRepository,
        },
        Create, FindById, FindByRef,
    },
    Error, Result,
};
use sqlx::PgPool;
use std::{path::Path, sync::Arc};
use uuid::Uuid;

pub async fn register(
    pool: &PgPool,
    execution: &Execution,
    worker: i64,
    instance: Uuid,
) -> Result<Option<DeliveryContext>> {
    let mut tx = pool.begin().await?;
    if ExecutionAdmissionRepository::find(&mut tx, RuntimeOwner::Execution(execution.id))
        .await?
        .is_none()
    {
        return Ok(None);
    }
    ExecutionAdmissionRepository::require(&mut tx, RuntimeOwner::Execution(execution.id)).await?;
    let execution = ExecutionRepository::find_by_id(&mut *tx, execution.id)
        .await?
        .ok_or_else(|| Error::PermissionDenied("Recorded execution unavailable".into()))?;
    let snapshot = pinned_snapshot(&execution)?;
    let timeout =
        crate::executor::resolve_execution_timeout_seconds(&execution, &snapshot.executable.action);
    let seconds = i64::try_from(timeout)
        .ok()
        .and_then(|seconds| seconds.checked_add(60))
        .ok_or_else(|| Error::validation("Worker delivery timeout overflow"))?;
    let deadline = chrono::Duration::try_seconds(seconds)
        .and_then(|duration| chrono::Utc::now().checked_add_signed(duration))
        .ok_or_else(|| Error::validation("Worker delivery deadline overflow"))?;
    let context =
        WorkerDeliveryRepository::register(&mut tx, execution.id, worker, instance, deadline)
            .await?;
    tx.commit().await?;
    Ok(Some(context))
}

pub fn log_reference(execution: &Execution, stream: &str) -> Result<String> {
    attune_common::worker_delivery::log_reference(execution, stream)
}

fn pinned_snapshot(execution: &Execution) -> Result<ExecutionExecutableSnapshot> {
    execution
        .executable_snapshot
        .clone()
        .ok_or_else(|| Error::PermissionDenied("Pinned log owner unavailable".into()))
}

#[allow(clippy::too_many_arguments)]
pub async fn allocate_log(
    pool: &PgPool,
    execution: &Execution,
    context: &DeliveryContext,
    stream: &str,
    backend: LogStreamBackend,
    policy: RetentionPolicyType,
    limit: i32,
    segment_bytes: u64,
    flush_ms: u64,
    max_bytes: usize,
) -> Result<(String, i64)> {
    let mut tx = pool.begin().await?;
    if context.execution != execution.id {
        return Err(Error::PermissionDenied(
            "Worker launch context execution mismatch".into(),
        ));
    }
    let (admission, effect) =
        WorkerDeliveryRepository::require_launch_context(&mut tx, context).await?;
    let execution = ExecutionRepository::find_by_id(&mut *tx, execution.id)
        .await?
        .ok_or_else(|| Error::PermissionDenied("Recorded execution unavailable".into()))?;
    let snapshot = pinned_snapshot(&execution)?;
    let reference = log_reference(&execution, stream)?;
    let policy = snapshot
        .executable
        .action
        .log_retention_policy
        .unwrap_or(policy);
    let limit = snapshot
        .executable
        .action
        .log_retention_limit
        .unwrap_or(limit);
    use attune_common::repositories::runtime_target::{
        ComponentTarget, RuntimeTarget, RuntimeTargetRepository,
    };
    // This resolves only the live structural target under lock. Code, timeout,
    // owner identity and selection come exclusively from the admitted pin.
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Component(ComponentTarget::Action, snapshot.executable.action.id),
        admission.authority.owner.sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| Error::PermissionDenied("Log owner unavailable".into()))?;
    if target.pack_id() != Some(snapshot.executable.action.pack)
        || target.context().target_ref.as_deref() != Some(snapshot.executable.action.r#ref.as_str())
    {
        return Err(Error::PermissionDenied("Log owner identity changed".into()));
    }
    let artifact = match ArtifactRepository::find_by_ref(&mut *tx, &reference).await? {
        Some(artifact) => artifact,
        None => {
            ArtifactRepository::create(
                &mut *tx,
                CreateArtifactInput {
                    r#ref: reference.clone(),
                    scope: OwnerType::Action,
                    owner: execution.action_ref.clone(),
                    r#type: ArtifactType::FileText,
                    visibility: ArtifactVisibility::Private,
                    classification: attune_common::models::ArtifactClassification::RuntimeLog,
                    retention_policy: policy,
                    retention_limit: limit,
                    name: None,
                    description: None,
                    content_type: Some("text/plain".into()),
                    data: None,
                },
            )
            .await?
        }
    };
    ArtifactRepository::find_by_id_for_update(&mut tx, artifact.id)
        .await?
        .ok_or_else(|| Error::PermissionDenied("Worker log artifact unavailable".into()))?;
    WorkerDeliveryRepository::require_log_owner(
        &mut tx,
        artifact.id,
        snapshot.executable.action.id,
        snapshot.executable.action.pack,
    )
    .await?;
    let version = ArtifactVersionRepository::create_log_pending_with_conn(
        &mut tx,
        artifact.id,
        &reference,
        backend,
        execution.id,
        serde_json::json!({"stream":stream,"execution_id":execution.id}),
    )
    .await?;
    ArtifactVersionRepository::mark_encrypted_log(&mut *tx, version.id).await?;
    let log = LogStreamRepository::create_with_backend_with_conn(
        &mut tx,
        version.id,
        backend,
        segment_bytes,
        flush_ms,
    )
    .await?;
    let plaintext_cap =
        u64::try_from(max_bytes).map_err(|_| Error::validation("Log byte cap overflow"))?;
    let wire_cap = attune_common::runtime_read_log::maximum_encoded_bytes(plaintext_cap)?;
    WorkerDeliveryRepository::bind_encrypted_log(
        &mut tx,
        context,
        version.id,
        log.id,
        i64::try_from(wire_cap).map_err(|_| Error::validation("Log wire cap overflow"))?,
        i64::try_from(wire_cap).map_err(|_| Error::validation("Log sequence cap overflow"))?,
        i64::try_from(
            plaintext_cap
                .checked_add(128)
                .ok_or_else(|| Error::validation("Log plaintext cap overflow"))?,
        )
        .map_err(|_| Error::validation("Log plaintext cap overflow"))?,
        &effect,
    )
    .await?;
    effect.ensure_fresh_on(&mut tx).await?;
    tx.commit().await?;
    Ok((
        version
            .file_path
            .ok_or_else(|| Error::invalid_state("Missing log path"))?,
        version.id,
    ))
}

pub struct CompletionTransport {
    pool: PgPool,
    context: DeliveryContext,
    permit: DeliveryPermit,
    underlying: Arc<dyn ArtifactFileTransport>,
    api: ApiTransport,
}

impl std::fmt::Debug for CompletionTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompletionTransport")
    }
}

impl CompletionTransport {
    pub async fn for_version(
        pool: &PgPool,
        version: i64,
        worker: i64,
        instance: Uuid,
        underlying: Arc<dyn ArtifactFileTransport>,
        api_url: &str,
        config: &JwtConfig,
    ) -> Result<Arc<dyn ArtifactFileTransport>> {
        let mut tx = pool.begin().await?;
        let permit = WorkerDeliveryRepository::for_version(&mut tx, version)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Worker delivery permit unavailable".into()))?;
        let (context, permit, effect) =
            WorkerDeliveryRepository::require(&mut tx, permit.id, worker, instance, version)
                .await?;
        let token = attune_common::worker_delivery::mint(&context, &permit, config)?;
        let api = ApiTransport::new(api_url, &token, underlying.base_dir())?;
        effect.ensure_fresh_on(&mut tx).await?;
        tx.commit().await?;
        Ok(Arc::new(Self {
            pool: pool.clone(),
            context,
            permit,
            underlying,
            api,
        }))
    }

    async fn append_shared(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let (_, permit, effect) = WorkerDeliveryRepository::require(
            &mut tx,
            self.permit.id,
            self.context.worker,
            self.context.worker_instance,
            self.permit.artifact_version,
        )
        .await?;
        let version = ArtifactVersionRepository::find_by_id(&mut *tx, self.permit.artifact_version)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Worker log version unavailable".into()))?;
        if version.file_path.as_deref() != Some(path) {
            return Err(Error::PermissionDenied("Worker log path mismatch".into()));
        }
        ArtifactRepository::find_by_id_for_update(&mut tx, version.artifact)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Worker log artifact unavailable".into()))?;
        WorkerDeliveryRepository::require_owner_binding(&mut tx, self.permit.id, version.id)
            .await?;
        ArtifactVersionRepository::find_by_id_for_update(&mut tx, version.id)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Worker log version unavailable".into()))?;
        let stream = LogStreamRepository::lock(
            &mut tx,
            self.permit
                .log_stream
                .ok_or_else(|| Error::PermissionDenied("Not a log delivery permit".into()))?,
        )
        .await?;
        let file =
            attune_common::artifact_transport::VolumeTransport::new(self.underlying.base_dir())
                .lock_log_append(path)
                .await?;
        let current_size = file.size_bytes();
        let end = current_size
            .checked_add(bytes.len() as u64)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or_else(|| Error::validation("Worker log size overflow"))?;
        let operation = if bytes.is_empty() {
            if current_size != 0 {
                return Err(Error::PermissionDenied(
                    "Cannot truncate an existing worker log".into(),
                ));
            }
            DeliveryOperation::Seal {
                sequences: 0,
                bytes: 0,
            }
        } else {
            DeliveryOperation::Append {
                sequence: stream.next_sequence,
                end,
            }
        };
        if stream.sealed || !permit.allows(Some(stream.id), operation) {
            return Err(Error::PermissionDenied(
                "Worker log delivery exceeds bounds".into(),
            ));
        }
        let content = bytes.to_vec();
        let final_fence = effect.clone();
        let runtime = tokio::runtime::Handle::current();
        tx = tokio::task::spawn_blocking(move || {
            // The database clock runs AFTER this task's queue wait as well as
            // the actual flock. No source row or approval lock is reacquired.
            runtime.block_on(final_fence.ensure_fresh_on(&mut tx))?;
            file.append(&content)?;
            Ok::<_, Error>(tx)
        })
        .await
        .map_err(|error| Error::Io(format!("Worker log append task failed: {error}")))??;
        if !bytes.is_empty() {
            WorkerDeliveryRepository::record_shared_append(&mut tx, stream.id, end).await?;
        }
        effect.ensure_fresh_on(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ArtifactFileTransport for CompletionTransport {
    async fn write_file(&self, path: &str, bytes: &[u8], content_type: Option<&str>) -> Result<()> {
        if self.underlying.transport_mode() == "volume" && bytes.is_empty() {
            self.append_shared(path, bytes).await
        } else {
            self.api.write_file(path, bytes, content_type).await
        }
    }
    async fn write_file_from_path(
        &self,
        path: &str,
        source: &Path,
        content_type: Option<&str>,
    ) -> Result<u64> {
        self.api
            .write_file_from_path(path, source, content_type)
            .await
    }
    async fn append_log_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.append_shared(path, bytes).await
    }
    async fn commit_log_segment(&self, version: i64, sequence: i64, bytes: &[u8]) -> Result<()> {
        self.api.commit_log_segment(version, sequence, bytes).await
    }
    async fn seal_log_stream(&self, version: i64, truncated: bool) -> Result<()> {
        self.api.seal_log_stream(version, truncated).await
    }
    async fn seal_log_stream_with_manifest(
        &self,
        version: i64,
        truncated: bool,
        manifest: &str,
    ) -> Result<()> {
        self.api
            .seal_log_stream_with_manifest(version, truncated, manifest)
            .await
    }
    async fn complete_file(&self, path: &str) -> Result<Option<u64>> {
        self.api.complete_file(path).await
    }
    async fn file_exists(&self, path: &str) -> Result<bool> {
        self.underlying.file_exists(path).await
    }
    async fn file_size(&self, path: &str) -> Result<Option<u64>> {
        self.underlying.file_size(path).await
    }
    async fn delete_file(&self, _path: &str) -> Result<()> {
        Err(Error::PermissionDenied(
            "Delivery permits cannot delete files".into(),
        ))
    }
    async fn open_reader(&self, _path: &str, _offset: u64) -> Result<BoxAsyncReader> {
        Err(Error::PermissionDenied(
            "Delivery permits cannot read files".into(),
        ))
    }
    fn transport_mode(&self) -> &'static str {
        self.underlying.transport_mode()
    }
    fn base_dir(&self) -> &str {
        self.underlying.base_dir()
    }
    async fn ensure_parent_dirs(&self, path: &str) -> Result<()> {
        self.underlying.ensure_parent_dirs(path).await
    }
}
