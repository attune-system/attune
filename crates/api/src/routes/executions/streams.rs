//! Execution transports use B's metadata projection and C's typed content
//! frontier, never broadcast payloads or a contributor's identity as authority.
use super::*;
use attune_common::repositories::{
    identity::IdentityRepository,
    runtime_read::{RuntimeReadOrigin, RuntimeReadRepository as Reads, RuntimeReadTarget},
    runtime_target::{RuntimeTarget, RuntimeTargetRepository},
};
use futures::TryStreamExt;

async fn human_live(state: &Arc<AppState>, user: &AuthenticatedUser) -> ApiResult<()> {
    if chrono::Utc::now().timestamp() >= user.claims.exp {
        return Err(ApiError::Unauthorized("Stream credential expired".into()));
    }
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Stream identity unavailable".into()))?;
    if !IdentityRepository::find_by_id(&state.db, identity)
        .await?
        .is_some_and(|row| !row.frozen)
    {
        return Err(ApiError::Unauthorized(
            "Stream identity is no longer active".into(),
        ));
    }
    if chrono::Utc::now().timestamp() >= user.claims.exp {
        return Err(ApiError::Unauthorized("Stream credential expired".into()));
    }
    Ok(())
}

async fn metadata_candidate(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    id: Option<i64>,
) -> ApiResult<Option<String>> {
    if crate::recorded_consumers::is_recorded(user)? {
        if chrono::Utc::now().timestamp() >= user.claims.exp {
            return Err(ApiError::Unauthorized("Stream credential expired".into()));
        }
        let mut tx = state.db.begin().await?;
        let authority = user
            .recorded_runtime_authority(&mut tx)
            .await?
            .ok_or_else(|| ApiError::Forbidden("Recorded source unavailable".into()))?;
        let mut payload = None;
        if let Some(id) = id {
            let target = RuntimeTargetRepository::resolve(
                &mut tx,
                RuntimeTarget::Execution(id),
                authority.credential_binding().sponsor_identity_id,
            )
            .await?;
            authority.ensure_fresh_on(&mut tx).await?;
            if target
                .as_ref()
                .is_some_and(|target| authority.allows_target(target, Action::Read))
            {
                let execution = ExecutionRepository::find_by_id(&mut *tx, id)
                    .await?
                    .ok_or_else(|| ApiError::NotFound("Execution unavailable".into()))?;
                let metadata = super::recorded::metadata(execution);
                payload = Some(serde_json::to_string(&serde_json::json!({"entity_type":"execution","entity_id":id,"payload":metadata})).map_err(|_| ApiError::InternalServerError("Execution metadata encoding failed".into()))?);
                Reads::record(
                    &mut tx,
                    &authority,
                    RuntimeReadOrigin {
                        target: RuntimeReadTarget::ExecutionMetadata { execution: id },
                        provenance: vec![],
                        parents: vec![],
                    },
                )
                .await?;
            }
        }
        if chrono::Utc::now().timestamp() >= user.claims.exp {
            return Err(ApiError::Unauthorized("Stream credential expired".into()));
        }
        authority.ensure_fresh_on(&mut tx).await?;
        tx.commit().await?;
        authority.ensure_fresh()?;
        if chrono::Utc::now().timestamp() >= user.claims.exp {
            return Err(ApiError::Unauthorized("Stream credential expired".into()));
        }
        return Ok(payload);
    }
    human_live(state, user).await?;
    let Some(id) = id else {
        return Ok(None);
    };
    let execution = ExecutionRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Execution unavailable".into()))?;
    match authorize_execution_access(
        state,
        user,
        &execution,
        Action::Read,
        None,
        &mut ExecutionVisibilityCache::default(),
    )
    .await
    {
        Ok(()) => {}
        Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    }
    let metadata = super::recorded::metadata(execution);
    let payload = serde_json::to_string(
        &serde_json::json!({"entity_type":"execution","entity_id":id,"payload":metadata}),
    )
    .map_err(|_| ApiError::InternalServerError("Execution metadata encoding failed".into()))?;
    human_live(state, user).await?;
    Ok(Some(payload))
}

pub(super) async fn updates(
    state: Arc<AppState>,
    user: AuthenticatedUser,
    filter: Option<i64>,
) -> ApiResult<axum::response::Response> {
    let receiver = state.broadcast_tx.subscribe();
    let initial = metadata_candidate(&state, &user, filter).await?;
    if filter.is_some() && initial.is_none() {
        return Err(ApiError::Forbidden(
            "Execution metadata stream is unavailable".into(),
        ));
    }
    let stream = futures::stream::unfold(
        (state, user, receiver, filter, false),
        |(state, user, mut receiver, filter, finished)| async move {
            if finished {
                return None;
            }
            loop {
                let message = tokio::select! {
                    value = receiver.recv() => match value {
                        Ok(value) => Some(value),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    },
                    _ = tokio::time::sleep(Duration::from_millis(250)) => None,
                };
                let id = match message {
                    None => None,
                    Some(message) => {
                        if message.len() > 1024 * 1024 {
                            continue;
                        }
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&message) else {
                            continue;
                        };
                        if value.get("entity_type").and_then(serde_json::Value::as_str)
                            != Some("execution")
                        {
                            continue;
                        }
                        let Some(id) = value
                            .get("entity_id")
                            .and_then(serde_json::Value::as_i64)
                            .filter(|id| *id > 0)
                        else {
                            continue;
                        };
                        if filter.is_some_and(|filter| filter != id) {
                            continue;
                        }
                        Some(id)
                    }
                };
                let (event, finished) = match metadata_candidate(&state, &user, id).await {
                    Ok(Some(payload)) => (Event::default().data(payload), false),
                    Ok(None) if id.is_none() => {
                        (Event::default().comment("authorized-wait"), false)
                    }
                    Ok(None) => continue,
                    Err(_) => (
                        Event::default()
                            .event("error")
                            .data("Execution stream authority is no longer valid"),
                        true,
                    ),
                };
                return Some((
                    Ok::<_, std::convert::Infallible>(event),
                    (state, user, receiver, filter, finished),
                ));
            }
        },
    );
    Ok((
        [
            ("x-accel-buffering", "no"),
            ("cache-control", "private, no-store"),
        ],
        Sse::new(stream),
    )
        .into_response())
}

async fn log_execution(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    id: i64,
) -> ApiResult<attune_common::models::Execution> {
    if !crate::recorded_consumers::is_recorded(user)? {
        human_live(state, user).await?;
        let execution = ExecutionRepository::find_by_id(&state.db, id)
            .await?
            .ok_or_else(|| ApiError::NotFound("Execution log source unavailable".into()))?;
        authorize_execution_log_stream(state, user, &execution).await?;
        human_live(state, user).await?;
        return Ok(execution);
    }
    let mut tx = state.db.begin().await?;
    let authority = crate::recorded_consumers::authorize_target(
        &mut tx,
        user,
        RuntimeTarget::Execution(id),
        Action::Read,
    )
    .await?;
    crate::recorded_disclosure::require_target(
        &mut tx,
        &authority,
        RuntimeTarget::Execution(id),
        &[Action::Decrypt],
    )
    .await?;
    let execution = ExecutionRepository::find_by_id(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Execution log source unavailable".into()))?;
    authority.ensure_fresh_on(&mut tx).await?;
    tx.commit().await?;
    authority.ensure_fresh()?;
    if chrono::Utc::now().timestamp() >= user.claims.exp {
        return Err(ApiError::Unauthorized("Stream credential expired".into()));
    }
    Ok(execution)
}

pub(super) async fn protected_log(
    state: Arc<AppState>,
    user: AuthenticatedUser,
    execution_id: i64,
    name: String,
    start: u64,
    permit: crate::execution_log_streams::ExecutionLogStreamPermit,
) -> ApiResult<axum::response::Response> {
    let execution = log_execution(&state, &user, execution_id).await?;
    let parsed = ExecutionLogStream::parse(&name)?;
    let reference = if Reads::source_admission(&state.db, execution_id)
        .await?
        .is_some()
    {
        attune_common::worker_delivery::log_reference(&execution, &name)?
    } else {
        parsed.artifact_ref(&execution.action_ref)
    };
    // HEAD headers cannot rely on the live body's first poll. Prepare an exact
    // pin and run its complete source/target/frontier authorization now.
    let initial_body =
        if let Some(artifact) = ArtifactRepository::find_by_ref(&state.db, &reference).await? {
            if let Some(version) = ArtifactVersionRepository::find_by_artifact_and_execution(
                &state.db,
                artifact.id,
                execution_id,
            )
            .await?
            {
                let pin = crate::live_logs::LogPin::select(&state, &user, version).await?;
                Some(
                    pin.response(state.clone(), user.clone())
                        .await?
                        .into_body()
                        .into_data_stream(),
                )
            } else {
                log_execution(&state, &user, execution_id).await?;
                None
            }
        } else {
            log_execution(&state, &user, execution_id).await?;
            None
        };
    let lost = permit.lease_lost_token();
    let stream = futures::stream::unfold(
        (
            state,
            user,
            reference,
            execution.action_ref,
            initial_body,
            0u64,
            true,
            false,
            permit,
            lost,
        ),
        move |(
            state,
            user,
            reference,
            action_ref,
            mut body,
            mut cursor,
            mut first,
            finished,
            permit,
            lost,
        )| async move {
            if finished {
                return None;
            }
            let result = async {
                if body.is_none() {
                    let execution = log_execution(&state, &user, execution_id).await?;
                    if execution.action_ref != action_ref {
                        return Err(ApiError::Forbidden("Execution source changed".into()));
                    }
                    if let Some(artifact) =
                        ArtifactRepository::find_by_ref(&state.db, &reference).await?
                    {
                        if let Some(version) =
                            ArtifactVersionRepository::find_by_artifact_and_execution(
                                &state.db,
                                artifact.id,
                                execution_id,
                            )
                            .await?
                        {
                            let pin =
                                crate::live_logs::LogPin::select(&state, &user, version).await?;
                            body = Some(
                                pin.response(state.clone(), user.clone())
                                    .await?
                                    .into_body()
                                    .into_data_stream(),
                            );
                        }
                    }
                    if body.is_none() {
                        if matches!(
                            execution.status,
                            attune_common::models::ExecutionStatus::Completed
                                | attune_common::models::ExecutionStatus::Failed
                                | attune_common::models::ExecutionStatus::Cancelled
                                | attune_common::models::ExecutionStatus::Timeout
                                | attune_common::models::ExecutionStatus::Abandoned
                        ) {
                            return Err(ApiError::NotFound(
                                "Execution finished without a retained log stream".into(),
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        // The discovery query/sleep is not a reusable permit.
                        log_execution(&state, &user, execution_id).await?;
                        return Ok((
                            Event::default()
                                .event("waiting")
                                .data("Log allocation pending"),
                            false,
                        ));
                    }
                }
                loop {
                    let bytes = body.as_mut().unwrap().try_next().await.map_err(|_| {
                        ApiError::Forbidden(
                            "Execution log is incomplete or authority expired".into(),
                        )
                    })?;
                    if lost.is_cancelled()
                        || state.execution_log_streams.shutdown_token().is_cancelled()
                    {
                        return Err(ApiError::Forbidden(
                            "Execution log stream lease ended".into(),
                        ));
                    }
                    let Some(bytes) = bytes else {
                        if cursor < start {
                            return Err(ApiError::BadRequest(
                                "Log cursor is beyond the verified plaintext length".into(),
                            ));
                        }
                        return Ok((
                            Event::default()
                                .id(cursor.to_string())
                                .event("done")
                                .data("Log stream verified"),
                            true,
                        ));
                    };
                    if bytes.is_empty() {
                        return Ok((Event::default().event("waiting").data(""), false));
                    }
                    let before = cursor;
                    cursor = cursor
                        .checked_add(bytes.len() as u64)
                        .ok_or_else(|| ApiError::BadRequest("Log cursor overflow".into()))?;
                    if cursor <= start {
                        continue;
                    }
                    let skip = start.saturating_sub(before) as usize;
                    let bytes = &bytes[skip..];
                    if first && start > 0 && bytes.first().is_some_and(|byte| byte & 0xc0 == 0x80) {
                        return Err(ApiError::BadRequest(
                            "Log cursor is inside a UTF-8 code point".into(),
                        ));
                    }
                    let event = Event::default()
                        .id(cursor.to_string())
                        .event(if first { "content" } else { "append" })
                        .data(String::from_utf8_lossy(bytes));
                    first = false;
                    return Ok((event, false));
                }
            }
            .await;
            let (event, finished) = match result {
                Ok(value) => value,
                Err(_) => (
                    Event::default()
                        .event("error")
                        .data("Execution log delivery is unavailable or incomplete"),
                    true,
                ),
            };
            Some((
                Ok::<_, std::convert::Infallible>(event),
                (
                    state, user, reference, action_ref, body, cursor, first, finished, permit, lost,
                ),
            ))
        },
    );
    Ok((
        [
            ("x-accel-buffering", "no"),
            ("cache-control", "private, no-store"),
        ],
        Sse::new(stream),
    )
        .into_response())
}

pub(super) fn guard_legacy_log<S>(
    inner: S,
    state: Arc<AppState>,
    user: AuthenticatedUser,
    id: i64,
    permit: crate::execution_log_streams::ExecutionLogStreamPermit,
) -> impl futures::Stream<Item = Result<Event, std::convert::Infallible>>
where
    S: futures::Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
{
    use futures::StreamExt;
    futures::stream::unfold(
        (Box::pin(inner), state, user, permit, false),
        move |(mut inner, state, user, permit, finished)| async move {
            if finished {
                return None;
            }
            let event = inner.next().await?;
            let checked = log_execution(&state, &user, id).await;
            let (event, finished) = if checked.is_err() || permit.lease_lost_token().is_cancelled()
            {
                (
                    Ok(Event::default()
                        .event("error")
                        .data("Execution log authority is no longer valid")),
                    true,
                )
            } else {
                (event, false)
            };
            Some((event, (inner, state, user, permit, finished)))
        },
    )
}
