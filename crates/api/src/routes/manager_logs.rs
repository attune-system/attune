//! Allocate only logs of the live action attempt and resume exact, sealed custody.
use attune_common::{
    manager_control::{ActionLog, LogChannel, LogResume, OpenLog, ResumeLog},
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, Id, LogStreamBackend, OwnerType,
        RetentionPolicyType, WorkerRole,
    },
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        delegated_execution::DelegatedExecutionRepository,
        execution_runtime_admission::{ExecutionAdmissionRepository, ExistingExecutionAuthority},
        file_evidence_retention::FileEvidenceRetentionRepository,
        log_delivery::{LogDeliveryRepository, PrepareLogDelivery},
        log_stream::LogStreamRepository,
        manager_assignment::ManagerAssignmentRepository,
        manager_log::ManagerLogRepository,
        manager_runtime::ManagerRuntimeRepository,
        schema_type::SchemaTypeRepository,
        worker_delivery::WorkerDeliveryRepository,
    },
};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};

use crate::{
    auth::middleware::RequireAuth, middleware::ApiError,
    routes::managers::ManagerSessionAuthorization, state::SharedState,
};

pub(crate) async fn open(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(assignment): Path<Id>,
    Json(request): Json<OpenLog>,
) -> Result<impl IntoResponse, ApiError> {
    let mut tx = state.db.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let scope = auth.work_scope(&state, &mut tx).await?;
    let (execution, deadline) = ManagerAssignmentRepository::execution_for_launch(
        &mut tx,
        scope,
        assignment,
        request.request,
    )
    .await?;
    let worker = execution.worker.ok_or_else(denied)?;
    let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
    let action = &snapshot.executable.action;
    let recorded = ExecutionAdmissionRepository::existing_authority(&mut tx, execution.id).await?
        == ExistingExecutionAuthority::Recorded;
    let context = if recorded {
        Some(
            WorkerDeliveryRepository::for_execution(&mut tx, execution.id)
                .await?
                .ok_or_else(denied)?,
        )
    } else {
        DelegatedExecutionRepository::validate(&mut tx, execution.id).await?;
        None
    };
    let attempt = if let Some(context) = &context {
        if context.worker != worker || context.worker_instance != manager.instance {
            return Err(denied());
        }
        context.attempt
    } else {
        DelegatedExecutionRepository::current_attempt(&mut tx, execution.id).await?
    };
    let channel = request.channel.as_str();
    let max_plaintext = max_plaintext(&state, request.channel);
    let segments = state.config.artifacts.log_segment_writer_config();
    let version = if let Some(version) =
        ManagerLogRepository::existing(&mut tx, assignment, channel).await?
    {
        version
    } else {
        let reference = if recorded {
            attune_common::worker_delivery::log_reference(&execution, channel)?
        } else {
            format!("{}.{channel}.log", execution.action_ref)
        };
        let artifact = ArtifactRepository::create_or_get(
            &mut tx,
            CreateArtifactInput {
                r#ref: reference.clone(),
                scope: OwnerType::Action,
                owner: execution.action_ref.clone(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: action
                    .log_retention_policy
                    .or_else(|| {
                        state
                            .config
                            .worker
                            .as_ref()
                            .map(|worker| worker.execution_log_retention_policy)
                    })
                    .unwrap_or(RetentionPolicyType::Days),
                retention_limit: action
                    .log_retention_limit
                    .or_else(|| {
                        state
                            .config
                            .worker
                            .as_ref()
                            .map(|worker| worker.execution_log_retention_limit)
                    })
                    .unwrap_or(7),
                name: None,
                description: None,
                content_type: Some("text/plain".into()),
                data: None,
            },
        )
        .await?;
        ArtifactRepository::find_by_id_for_update(&mut tx, artifact.id)
            .await?
            .ok_or_else(denied)?;
        WorkerDeliveryRepository::require_log_owner(&mut tx, artifact.id, action.id, action.pack)
            .await?;
        let backend = LogStreamBackend::ObjectSegments;
        let version = ArtifactVersionRepository::create_log_pending_with_conn(
            &mut tx,
            artifact.id,
            &reference,
            backend,
            execution.id,
            serde_json::json!({"stream": channel, "execution_id": execution.id}),
        )
        .await?;
        ArtifactVersionRepository::mark_encrypted_log(&mut *tx, version.id).await?;
        let stream = LogStreamRepository::create_with_backend_with_conn(
            &mut tx,
            version.id,
            backend,
            segments.max_segment_bytes as u64,
            segments.flush_interval_ms,
        )
        .await?;
        if let Some(context) = &context {
            let (_, effect) =
                WorkerDeliveryRepository::require_launch_context(&mut tx, context).await?;
            let cap = attune_common::runtime_read_log::maximum_encoded_bytes(max_plaintext as u64)?;
            let bound = i64::try_from(cap).map_err(|_| denied())?;
            let plaintext =
                i64::try_from(max_plaintext.saturating_add(128)).map_err(|_| denied())?;
            WorkerDeliveryRepository::bind_encrypted_log(
                &mut tx, context, version.id, stream.id, bound, bound, plaintext, &effect,
            )
            .await?;
            effect.ensure_fresh_on(&mut tx).await?;
        } else {
            ArtifactVersionRepository::protect_delegated_logs(&mut tx, execution.id, &[version.id])
                .await?;
        }
        ManagerLogRepository::bind(&mut tx, assignment, version.id, worker, &manager, channel)
            .await?;
        version.id
    };
    let stream = LogStreamRepository::find_by_artifact_version_with_conn(&mut tx, version)
        .await?
        .ok_or_else(denied)?;
    let key = ManagerLogRepository::key(
        &mut *tx,
        version,
        state.config.security.encryption_key.as_deref(),
    )
    .await?
    .ok_or_else(denied)?;
    let capture_token = if let Some(context) = &context {
        let permit = WorkerDeliveryRepository::for_version(&mut tx, version)
            .await?
            .ok_or_else(denied)?;
        let (_, effect) =
            WorkerDeliveryRepository::require_launch_context(&mut tx, context).await?;
        effect.ensure_fresh_on(&mut tx).await?;
        attune_common::worker_delivery::mint(context, &permit, &state.jwt_config)?
    } else {
        attune_common::log_delivery::mint_capture(
            attune_common::log_delivery::CaptureCredential {
                version,
                worker,
                session: manager.id,
                instance: manager.instance,
            },
            &state.jwt_config,
        )?
    };
    ManagerAssignmentRepository::check_deadline(&mut tx, deadline).await?;
    tx.commit().await?;
    Ok(no_store(ActionLog {
        version,
        stream: stream.id,
        worker,
        attempt,
        key,
        max_plaintext,
        segment_bytes: usize::try_from(stream.max_unflushed_bytes).map_err(|_| denied())?,
        capture_token,
        recoverable: !recorded,
    }))
}

pub(crate) async fn prepare(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(version): Path<Id>,
    Json(request): Json<PrepareLogDelivery>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let (manager, scope) = load_scope(&state, &auth, &mut tx, version).await?;
    if request.version != version
        || request.worker != scope.worker
        || request.instance != manager.instance
    {
        return Err(denied());
    }
    let auth_scope = auth.work_scope(&state, &mut tx).await?;
    let lease_deadline = ManagerAssignmentRepository::require_log_preparation(
        &mut tx,
        auth_scope,
        scope.assignment,
        request.execution,
    )
    .await?;
    let intent = LogDeliveryRepository::prepare(&mut tx, request).await?;
    ManagerRuntimeRepository::check_deadline(
        &mut tx,
        &manager,
        auth.bearer_expires_at()
            .min(intent.finalize_by)
            .min(lease_deadline),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn resume(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(version): Path<Id>,
    Json(request): Json<ResumeLog>,
) -> Result<impl IntoResponse, ApiError> {
    if request.custody.is_nil() {
        return Err(denied());
    }
    let mut tx = state.db.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
    SchemaTypeRepository::lock_selection(&mut tx).await?;
    let (manager, scope) = load_scope(&state, &auth, &mut tx, version).await?;
    let intent = LogDeliveryRepository::find(&mut *tx, version).await?;
    let response = if let Some(intent) = intent {
        if intent.custody != request.custody || intent.worker != scope.worker {
            return Err(denied());
        }
        let ready = ArtifactVersionRepository::log_is_sealed(&mut tx, version).await?;
        if ready && matches!(intent.state.as_str(), "prepared" | "active") {
            LogDeliveryRepository::mark_ready(&mut tx, version).await?;
            LogResume::Closed
        } else {
            match intent.state.as_str() {
                "prepared" if intent.finalize_by > chrono::Utc::now() => LogResume::Waiting,
                "active" if intent.finalize_by > chrono::Utc::now() => {
                    if intent.delivery_instance != manager.instance {
                        LogDeliveryRepository::claim(
                            &mut tx,
                            version,
                            scope.worker,
                            manager.instance,
                            request.custody,
                        )
                        .await?;
                    }
                    let fence = LogDeliveryRepository::require(
                        &mut tx,
                        version,
                        scope.worker,
                        manager.instance,
                    )
                    .await?;
                    LogResume::Upload {
                        token: attune_common::log_delivery::mint(&fence.intent, &state.jwt_config)?,
                    }
                }
                "prepared" | "active" => {
                    LogDeliveryRepository::mark_incomplete(
                        &mut tx,
                        version,
                        scope.worker,
                        request.custody,
                        "deadline",
                    )
                    .await?;
                    LogResume::Closed
                }
                "ready" | "incomplete" | "revoked" => LogResume::Closed,
                _ => return Err(denied()),
            }
        }
    } else {
        // Check the assignment before declaring absence. Preparation for the
        // same slot and version takes this lock before inserting its intent.
        if ManagerAssignmentRepository::log_preparation_open(&mut tx, scope.assignment, manager.id)
            .await?
        {
            LogResume::Waiting
        } else {
            // Recheck after the lock in case the first read raced a commit.
            match LogDeliveryRepository::find(&mut *tx, version).await? {
                Some(_) => LogResume::Waiting,
                None => LogResume::Closed,
            }
        }
    };
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    Ok(no_store(response))
}

async fn load_scope(
    state: &SharedState,
    auth: &ManagerSessionAuthorization,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    version: Id,
) -> Result<
    (
        attune_common::repositories::manager_session::ManagerSessionRecord,
        attune_common::repositories::manager_log::ManagerLogScope,
    ),
    ApiError,
> {
    let manager = auth.live_record(state, tx).await?;
    let scope = ManagerLogRepository::find(&mut **tx, version)
        .await?
        .ok_or_else(denied)?;
    if manager.role != WorkerRole::Action
        || manager.pool != scope.pool
        || manager.name != scope.slot
    {
        return Err(denied());
    }
    ManagerLogRepository::retain_slot(tx, version, scope.worker, manager.instance)
        .await?
        .ok_or_else(denied)?;
    Ok((manager, scope))
}

pub(super) fn max_plaintext(state: &SharedState, channel: LogChannel) -> usize {
    state
        .config
        .worker
        .as_ref()
        .map_or(10 * 1024 * 1024, |worker| match channel {
            LogChannel::Stdout => worker.max_stdout_bytes,
            LogChannel::Stderr => worker.max_stderr_bytes,
        })
        .min(attune_common::manager_control::MAX_MANAGER_LOG_PLAINTEXT)
}

fn no_store<T>(value: T) -> ([(header::HeaderName, &'static str); 1], Json<T>) {
    ([(header::CACHE_CONTROL, "no-store")], Json(value))
}

fn denied() -> ApiError {
    ApiError::Forbidden("Manager log delivery is unavailable".into())
}
