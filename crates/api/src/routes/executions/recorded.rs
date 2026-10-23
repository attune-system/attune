//! Execution metadata/control never derives authority from the sponsor's IAM.
use super::*;
use attune_common::repositories::runtime_target::{RuntimeTarget, RuntimeTargetRepository};

pub(super) fn active(user: &AuthenticatedUser) -> ApiResult<bool> {
    crate::recorded_consumers::is_recorded(user)
}

pub(super) fn metadata(execution: attune_common::models::Execution) -> ExecutionResponse {
    let mut response = ExecutionResponse::from(execution);
    response.config = None;
    response.result = None;
    response.workflow_task = None;
    response.worker_selector = None;
    response.worker_tolerations = None;
    response.worker_affinity = None;
    response.parent = None;
    response.enforcement = None;
    response.original_execution = None;
    response
}

pub(super) async fn detail(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    id: i64,
    decrypt: bool,
) -> ApiResult<axum::response::Response> {
    let mut tx = state.db.begin().await?;
    let content = if decrypt {
        Some(
            crate::recorded_consumers::authorize_execution_content_prepared(
                &mut tx,
                user,
                id,
                state.config.security.encryption_key.as_deref(),
            )
            .await?,
        )
    } else {
        None
    };
    let metadata_authority = if content.is_none() {
        Some(
            crate::recorded_consumers::authorize_target(
                &mut tx,
                user,
                RuntimeTarget::Execution(id),
                Action::Read,
            )
            .await?,
        )
    } else {
        None
    };
    let execution = ExecutionRepository::find_by_id(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Execution unavailable".into()))?;
    let mut response = metadata(execution.clone());
    if let Some(content) = content.as_ref() {
        for (entity, value) in [
            (
                attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
                execution.config,
            ),
            (
                attune_common::secret_values::ENTITY_EXECUTION_RESULT,
                execution.result,
            ),
        ] {
            let value = if let Some(value) = value {
                Some(content.restore_entity(&mut tx, entity, value).await?)
            } else {
                None
            };
            if entity == attune_common::secret_values::ENTITY_EXECUTION_CONFIG {
                response.config = value;
            } else {
                response.result = value;
            }
        }
    }
    // Serialize while the source transaction is still held, before final checks.
    let encoded = serde_json::to_vec(&ApiResponse::new(response))
        .map_err(|_| ApiError::Forbidden("Execution content unavailable".into()))?;
    if let Some(content) = content.as_ref() {
        content.ensure_fresh_on(&mut tx).await?;
    }
    if let Some(authority) = metadata_authority.as_ref() {
        authority.ensure_fresh_on(&mut tx).await?;
    }
    // Content authorization captures a durable read origin. Returning plaintext
    // from an implicitly rolled-back transaction would erase the sink fence.
    tx.commit().await?;
    // No application await follows the final source and token deadline checks.
    if let Some(content) = content.as_ref() {
        content.ensure_fresh()?;
    }
    if let Some(authority) = metadata_authority.as_ref() {
        authority.ensure_fresh()?;
    }
    if chrono::Utc::now().timestamp() >= user.claims.exp {
        return Err(ApiError::Forbidden("Execution content unavailable".into()));
    }
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        encoded,
    )
        .into_response())
}

pub(super) async fn search(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    filters: &ExecutionSearchFilters,
    action: Action,
) -> ApiResult<ExecutionSearchResult> {
    if filters.result_contains.is_some() {
        return Err(ApiError::Forbidden(
            "Recorded metadata search cannot inspect execution plaintext".into(),
        ));
    }
    let mut tx = state.db.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let filter = authority.execution_collection(action);
    Ok(
        attune_common::repositories::runtime_authority::recorded_collections::executions(
            &mut tx, &filter, filters,
        )
        .await?,
    )
}

pub(super) async fn status_counts(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
) -> ApiResult<Vec<ExecutionStatusCountRow>> {
    let mut tx = state.db.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let filter = authority.execution_collection(Action::Read);
    let rows = attune_common::repositories::runtime_authority::recorded_collections::execution_status_counts(&mut tx, &filter).await?;
    Ok(rows
        .into_iter()
        .map(|(status, count)| ExecutionStatusCountRow { status, count })
        .collect())
}

pub(super) async fn cancel(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    id: i64,
) -> ApiResult<ExecutionResponse> {
    let mut tx = state.db.begin().await?;
    // Source first, then the target mutation lock, then typed target resolution.
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let changes = ExecutionRepository::cancel_recorded_tree(&mut tx, id, &authority)
        .await
        .map_err(|error| match error {
            attune_common::Error::InvalidState(message) => ApiError::Conflict(message),
            other => other.into(),
        })?;
    let updated = ExecutionRepository::find_by_id(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Execution unavailable".into()))?;
    authority.ensure_fresh_on(&mut tx).await?;
    tx.commit().await?;
    let publisher = state.get_publisher().await;
    for (execution, status) in changes {
        publish_status_change_to_executor(publisher.as_deref(), &execution, status, "api-service")
            .await;
        if status == ExecutionStatus::Canceling {
            if let Some(worker) = execution.worker {
                send_cancel_to_worker(publisher.as_deref(), execution.id, worker).await;
            }
        }
    }
    Ok(metadata(updated))
}

pub(super) async fn reschedule(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    id: i64,
) -> ApiResult<ExecutionRescheduleResponse> {
    use attune_common::repositories::requested_execution_publication::RequestedExecutionPublicationRepository as Requested;
    let mut tx = state.db.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let execution = ExecutionRepository::find_by_id(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Execution unavailable".into()))?;
    let target = RuntimeTargetRepository::resolve(
        &mut tx,
        RuntimeTarget::Execution(id),
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Execution target unavailable".into()))?;
    if !authority.allows_target(&target, Action::Cancel) {
        return Err(ApiError::Forbidden("Recorded reschedule denied".into()));
    }
    let publisher = state.get_publisher().await.ok_or_else(|| {
        ApiError::InternalServerError("Message queue publisher unavailable".into())
    })?;
    let key = state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::InternalServerError("Requested input encryption unavailable".into())
        })?;
    let prepared = Requested::prepare(&mut tx, &execution, key)
        .await?
        .ok_or_else(|| ApiError::Conflict("Execution is not eligible for reschedule".into()))?;
    prepared.lock_target(&mut tx).await?;
    let attempt = MaintenanceRepository::mark_execution_reschedule_attempt(
        &mut *tx,
        id,
        "api-service",
        "manual execution reschedule requested",
        state.config.maintenance.execution_reschedule_max_attempts,
        state.config.maintenance.execution_reschedule_grace_seconds,
        true,
    )
    .await?
    .ok_or_else(|| ApiError::Conflict("Execution is not eligible for reschedule".into()))?;
    let payload = ExecutionRequestedPayload {
        execution_id: id,
        action_id: attempt.action_id,
        action_ref: attempt.action_ref.clone(),
        parent_id: attempt.parent_id,
        enforcement_id: attempt.enforcement_id,
        config: attempt.config.clone(),
        release_id: execution.pack_release,
        release_digest: execution.pack_release_digest.clone(),
    };
    let message = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
        .with_source("api-service")
        .with_correlation_id(uuid::Uuid::new_v4());
    // Caller management authority and original TARGET input authority remain
    // independent, with both deadlines checked through confirmed publication.
    let published: ApiResult<()> = async {
        authority.ensure_fresh_on(&mut tx).await?;
        prepared
            .publish_bounded(&mut tx, publisher.publish_envelope(&message))
            .await?;
        authority.ensure_fresh_on(&mut tx).await?;
        Ok(())
    }
    .await;
    if let Err(error) = published {
        tx.rollback().await?;
        return Err(error);
    }
    tx.commit().await?;
    emit_execution_reschedule_audit(state, user, &execution, &attempt);
    Ok(ExecutionRescheduleResponse {
        message: "Execution request republished; pending scheduling".into(),
        attempt_count: attempt.attempt_count,
        last_attempt_at: attempt.last_attempt_at,
        execution: metadata(execution),
    })
}
