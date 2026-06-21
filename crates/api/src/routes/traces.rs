//! Trace report API route.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};

use attune_common::rbac::{Action as RbacAction, AuthorizationContext, Resource};
use attune_common::repositories::{
    event::{EnforcementRepository, EventRepository},
    execution::ExecutionRepository,
    work_queue::{WorkQueueDispatchRepository, WorkQueueItemRepository, WorkQueueRepository},
    FindById,
};
use attune_common::trace_tag::normalize_trace_tag;

use crate::auth::middleware::{AuthenticatedUser, RequireAuth};
use crate::authz::{AuthorizationCheck, AuthorizationService};
use crate::dto::{
    event::EventSummary,
    execution::ExecutionSummary,
    trace::{TraceEnforcementSummary, TraceReportResponse, TraceWorkQueueDispatchSummary},
    work_queue::WorkQueueItemResponse,
    ApiResponse,
};
use crate::middleware::{ApiError, ApiResult};
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/traces/{trace_tag}",
    tag = "traces",
    params(
        ("trace_tag" = String, Path, description = "Exact trace tag to report")
    ),
    responses(
        (status = 200, description = "Trace activity report", body = ApiResponse<TraceReportResponse>),
        (status = 400, description = "Invalid trace tag"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_trace_report(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(trace_tag): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = AuthorizationService::new(state.db.clone());
    let auth_ctx = AuthorizationContext::new(identity_id);
    for resource in [
        Resource::Executions,
        Resource::Enforcements,
        Resource::Events,
        Resource::QueueItems,
    ] {
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource,
                    action: RbacAction::Read,
                    context: auth_ctx.clone(),
                },
            )
            .await?;
    }

    let normalized = normalize_trace_tag(&trace_tag)
        .map_err(|e| ApiError::BadRequest(format!("Invalid trace_tag: {e}")))?;

    let executions = ExecutionRepository::list_by_trace_tag(&state.db, &normalized).await?;
    let execution_ids: Vec<i64> = executions.iter().map(|execution| execution.id).collect();

    let mut seen_enforcement_ids = HashSet::new();
    let mut enforcements = Vec::new();
    for enforcement_id in executions
        .iter()
        .filter_map(|execution| execution.enforcement)
    {
        if !seen_enforcement_ids.insert(enforcement_id) {
            continue;
        }
        if let Some(enforcement) =
            EnforcementRepository::find_by_id(&state.db, enforcement_id).await?
        {
            enforcements.push(TraceEnforcementSummary::from(enforcement));
        }
    }

    let mut seen_event_ids = HashSet::new();
    let mut events = Vec::new();
    for event_id in enforcements
        .iter()
        .filter_map(|enforcement| enforcement.event)
    {
        if !seen_event_ids.insert(event_id) {
            continue;
        }
        if let Some(event) = EventRepository::find_by_id(&state.db, event_id).await? {
            events.push(EventSummary::from(event));
        }
    }

    // Include origin event even when no enforcement/execution was created yet.
    if let Some((prefix, id)) = parse_default_trace_id(&normalized) {
        if let Some(event) = EventRepository::find_by_id(&state.db, id).await? {
            if event.trigger_ref == prefix && seen_event_ids.insert(event.id) {
                events.push(EventSummary::from(event));
            }
        }
    }

    let mut queue_visibility_cache: HashMap<i64, bool> = HashMap::new();

    let mut queue_dispatches = Vec::new();
    for dispatch in
        WorkQueueDispatchRepository::list_by_executions(&state.db, &execution_ids).await?
    {
        if queue_visible_for_trace(&state, &user, dispatch.queue, &mut queue_visibility_cache)
            .await?
        {
            queue_dispatches.push(TraceWorkQueueDispatchSummary::from(dispatch));
        }
    }

    let mut queue_items = Vec::new();
    let mut seen_queue_item_ids = HashSet::new();

    for item in
        WorkQueueItemRepository::list_by_related_executions(&state.db, &execution_ids).await?
    {
        if !seen_queue_item_ids.insert(item.id) {
            continue;
        }
        if queue_visible_for_trace(&state, &user, item.queue, &mut queue_visibility_cache).await? {
            queue_items.push(WorkQueueItemResponse::from(item));
        }
    }

    // Include origin queue item even when no dispatch/execution was created yet.
    if let Some((prefix, id)) = parse_default_trace_id(&normalized) {
        if let Some(item) = WorkQueueItemRepository::find_by_id(&state.db, id).await? {
            if item.queue_ref == prefix
                && seen_queue_item_ids.insert(item.id)
                && queue_visible_for_trace(&state, &user, item.queue, &mut queue_visibility_cache)
                    .await?
            {
                queue_items.push(WorkQueueItemResponse::from(item));
            }
        }
    }

    let mut origins = Vec::new();
    if !events.is_empty() {
        origins.push("event".to_string());
    }
    if !queue_items.is_empty() || !queue_dispatches.is_empty() {
        origins.push("work_queue_item".to_string());
    }
    if !executions.is_empty()
        && enforcements.is_empty()
        && queue_dispatches.is_empty()
        && queue_items.is_empty()
    {
        origins.push("manual_execution".to_string());
    }

    let response = TraceReportResponse {
        trace_tag: normalized,
        origins,
        executions: executions.into_iter().map(ExecutionSummary::from).collect(),
        enforcements,
        events,
        queue_dispatches,
        queue_items,
    };

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

/// Apply the same per-queue visibility rules used by queue-item endpoints so
/// trace reports never leak items or dispatches from queues the caller cannot
/// see. The decision only depends on the queue + caller, so results are cached
/// per queue.
async fn queue_visible_for_trace(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue_id: i64,
    cache: &mut HashMap<i64, bool>,
) -> ApiResult<bool> {
    if let Some(visible) = cache.get(&queue_id) {
        return Ok(*visible);
    }

    let visible = match WorkQueueRepository::find_by_id(&state.db, queue_id).await? {
        Some(queue) => {
            crate::routes::work_queues::queue_item_visible(state, user, RbacAction::Read, &queue)
                .await?
        }
        None => false,
    };
    cache.insert(queue_id, visible);
    Ok(visible)
}

fn parse_default_trace_id(trace_tag: &str) -> Option<(&str, i64)> {
    let (prefix, id) = trace_tag.rsplit_once('.')?;
    let parsed_id = id.parse::<i64>().ok()?;
    if prefix.is_empty() {
        return None;
    }
    Some((prefix, parsed_id))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/traces/{trace_tag}", get(get_trace_report))
}
