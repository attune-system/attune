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

use attune_common::rbac::{Action as RbacAction, AuthorizationContext, Grant, Resource};
use attune_common::repositories::{
    event::{EnforcementRepository, EventRepository},
    execution::{ExecutionRepository, ExecutionWithRefs},
    identity::IdentityRepository,
    work_queue::{WorkQueueDispatchRepository, WorkQueueItemRepository, WorkQueueRepository},
    FindById,
};
use attune_common::trace_tag::normalize_trace_tag;

use crate::auth::{
    jwt::TokenType,
    middleware::{AuthenticatedUser, RequireAuth},
};
use crate::authz::AuthorizationService;
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
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Err(ApiError::Forbidden(
            "Trace reports are only available to access or execution identities".to_string(),
        ));
    }

    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = AuthorizationService::new(state.db.clone());
    let grants = authz.effective_grants(&user).await?;

    let can_read_executions = has_resource_read_grant(&grants, Resource::Executions);
    let can_read_enforcements = has_resource_read_grant(&grants, Resource::Enforcements);
    let can_read_events = has_resource_read_grant(&grants, Resource::Events);

    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("Identity not found".to_string()))?;
    let identity_attributes = match identity.attributes {
        serde_json::Value::Object(map) => map.into_iter().collect(),
        _ => HashMap::new(),
    };

    let normalized = normalize_trace_tag(&trace_tag)
        .map_err(|e| ApiError::BadRequest(format!("Invalid trace_tag: {e}")))?;

    let executions = ExecutionRepository::list_by_trace_tag(&state.db, &normalized).await?;
    let execution_ids: Vec<i64> = executions.iter().map(|execution| execution.id).collect();

    let mut ancestor_cache: HashMap<Option<i64>, Vec<i64>> = HashMap::new();
    let mut visible_execution_ids = HashSet::new();
    let mut visible_execution_rows = Vec::new();

    if can_read_executions {
        let global_execution_read = has_unconstrained_read_grant(&grants, Resource::Executions);
        for execution in &executions {
            let allowed = if global_execution_read {
                true
            } else {
                execution_visible_for_trace(
                    &state,
                    &grants,
                    identity_id,
                    &identity_attributes,
                    execution,
                    &mut ancestor_cache,
                )
                .await?
            };

            if allowed {
                visible_execution_ids.insert(execution.id);
                visible_execution_rows.push(execution.clone());
            }
        }
    }

    let mut seen_enforcement_ids = HashSet::new();
    let mut fetched_enforcements = Vec::new();
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
            fetched_enforcements.push(enforcement);
        }
    }

    let mut visible_enforcement_ids = HashSet::new();
    let mut visible_enforcements = Vec::new();
    if can_read_enforcements {
        let global_enforcement_read = has_unconstrained_read_grant(&grants, Resource::Enforcements);
        for enforcement in &fetched_enforcements {
            let allowed = if global_enforcement_read {
                true
            } else {
                enforcement_visible_for_trace(
                    &grants,
                    identity_id,
                    &identity_attributes,
                    enforcement,
                )
            };
            if allowed {
                visible_enforcement_ids.insert(enforcement.id);
                visible_enforcements.push(enforcement.clone());
            }
        }
    }

    let mut seen_event_ids = HashSet::new();
    let mut fetched_events = Vec::new();
    for event_id in fetched_enforcements
        .iter()
        .filter_map(|enforcement| enforcement.event)
    {
        if !seen_event_ids.insert(event_id) {
            continue;
        }
        if let Some(event) = EventRepository::find_by_id(&state.db, event_id).await? {
            fetched_events.push(event);
        }
    }

    let mut visible_event_ids = HashSet::new();
    let mut visible_events = Vec::new();
    if can_read_events {
        let global_event_read = has_unconstrained_read_grant(&grants, Resource::Events);
        for event in fetched_events {
            let allowed = if global_event_read {
                true
            } else {
                event_visible_for_trace(&grants, identity_id, &identity_attributes, &event)
            };
            if allowed {
                visible_event_ids.insert(event.id);
                visible_events.push(event);
            }
        }
    }

    // Include origin event even when no enforcement/execution was created yet.
    if can_read_events {
        let global_event_read = has_unconstrained_read_grant(&grants, Resource::Events);
        if let Some((prefix, id)) = parse_default_trace_id(&normalized) {
            if let Some(event) = EventRepository::find_by_id(&state.db, id).await? {
                let allowed = global_event_read
                    || event_visible_for_trace(&grants, identity_id, &identity_attributes, &event);
                if event.trigger_ref == prefix && allowed && visible_event_ids.insert(event.id) {
                    visible_events.push(event);
                }
            }
        }
    }

    let mut queue_visibility_cache: HashMap<i64, bool> = HashMap::new();

    let mut queue_dispatches = Vec::new();
    for dispatch in
        WorkQueueDispatchRepository::list_by_executions(&state.db, &execution_ids).await?
    {
        if !visible_execution_ids.contains(&dispatch.execution) {
            continue;
        }
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
            let mut response = WorkQueueItemResponse::from(item);
            if response
                .requested_by_execution
                .is_some_and(|id| !visible_execution_ids.contains(&id))
            {
                response.requested_by_execution = None;
            }
            if response
                .leased_execution
                .is_some_and(|id| !visible_execution_ids.contains(&id))
            {
                response.leased_execution = None;
            }
            if response
                .requested_by_enforcement
                .is_some_and(|id| !visible_enforcement_ids.contains(&id))
            {
                response.requested_by_enforcement = None;
            }
            queue_items.push(response);
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
                let mut response = WorkQueueItemResponse::from(item);
                if response
                    .requested_by_execution
                    .is_some_and(|id| !visible_execution_ids.contains(&id))
                {
                    response.requested_by_execution = None;
                }
                if response
                    .leased_execution
                    .is_some_and(|id| !visible_execution_ids.contains(&id))
                {
                    response.leased_execution = None;
                }
                if response
                    .requested_by_enforcement
                    .is_some_and(|id| !visible_enforcement_ids.contains(&id))
                {
                    response.requested_by_enforcement = None;
                }
                queue_items.push(response);
            }
        }
    }

    let mut origins = Vec::new();
    if !visible_events.is_empty() {
        origins.push("event".to_string());
    }
    if !queue_items.is_empty() || !queue_dispatches.is_empty() {
        origins.push("work_queue_item".to_string());
    }
    if !visible_execution_rows.is_empty()
        && visible_enforcements.is_empty()
        && queue_dispatches.is_empty()
        && queue_items.is_empty()
    {
        origins.push("manual_execution".to_string());
    }

    let mut execution_summaries = Vec::with_capacity(visible_execution_rows.len());
    for execution in visible_execution_rows {
        let mut summary = ExecutionSummary::from(execution);
        if summary
            .parent
            .is_some_and(|parent_id| !visible_execution_ids.contains(&parent_id))
        {
            summary.parent = None;
        }
        if summary
            .original_execution
            .is_some_and(|original_id| !visible_execution_ids.contains(&original_id))
        {
            summary.original_execution = None;
        }
        if summary
            .enforcement
            .is_some_and(|enforcement_id| !visible_enforcement_ids.contains(&enforcement_id))
        {
            summary.enforcement = None;
            summary.rule_ref = None;
            summary.trigger_ref = None;
        }
        execution_summaries.push(summary);
    }

    let mut enforcement_summaries = Vec::with_capacity(visible_enforcements.len());
    for enforcement in visible_enforcements {
        let mut summary = TraceEnforcementSummary::from(enforcement);
        if summary
            .event
            .is_some_and(|event_id| !visible_event_ids.contains(&event_id))
        {
            summary.event = None;
        }
        enforcement_summaries.push(summary);
    }

    let response = TraceReportResponse {
        trace_tag: normalized,
        origins,
        executions: execution_summaries,
        enforcements: enforcement_summaries,
        events: visible_events.into_iter().map(EventSummary::from).collect(),
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

fn has_resource_read_grant(grants: &[Grant], resource: Resource) -> bool {
    grants
        .iter()
        .any(|grant| grant.resource == resource && grant.actions.contains(&RbacAction::Read))
}

fn has_unconstrained_read_grant(grants: &[Grant], resource: Resource) -> bool {
    grants.iter().any(|grant| {
        grant.resource == resource
            && grant.actions.contains(&RbacAction::Read)
            && grant.constraints.is_none()
    })
}

async fn execution_visible_for_trace(
    state: &Arc<AppState>,
    grants: &[Grant],
    identity_id: i64,
    identity_attributes: &HashMap<String, serde_json::Value>,
    execution: &ExecutionWithRefs,
    ancestor_cache: &mut HashMap<Option<i64>, Vec<i64>>,
) -> ApiResult<bool> {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.identity_attributes = identity_attributes.clone();
    ctx.target_id = Some(execution.id);
    ctx.target_ref = Some(execution.action_ref.clone());
    ctx.pack_ref = execution
        .action_ref
        .split_once('.')
        .map(|(pack, _)| pack.to_string());
    ctx.owner_identity_id = execution.executor;
    ctx.execution_owner_identity_id = execution.executor;
    ctx.execution_ancestor_identity_ids =
        execution_ancestor_identity_ids_for_trace(state, execution.parent, ancestor_cache).await?;

    Ok(AuthorizationService::is_allowed(
        grants,
        Resource::Executions,
        RbacAction::Read,
        &ctx,
    ))
}

async fn execution_ancestor_identity_ids_for_trace(
    state: &Arc<AppState>,
    parent_id: Option<i64>,
    cache: &mut HashMap<Option<i64>, Vec<i64>>,
) -> ApiResult<Vec<i64>> {
    if let Some(cached) = cache.get(&parent_id) {
        return Ok(cached.clone());
    }

    let mut identities = Vec::new();
    let mut current_parent = parent_id;
    let mut guard = 0u32;
    while let Some(current_id) = current_parent {
        guard += 1;
        if guard > 64 {
            break;
        }
        let Some(parent) = ExecutionRepository::find_by_id(&state.db, current_id).await? else {
            break;
        };
        if let Some(executor) = parent.executor {
            identities.push(executor);
        }
        current_parent = parent.parent;
    }
    identities.sort_unstable();
    identities.dedup();
    cache.insert(parent_id, identities.clone());
    Ok(identities)
}

fn enforcement_visible_for_trace(
    grants: &[Grant],
    identity_id: i64,
    identity_attributes: &HashMap<String, serde_json::Value>,
    enforcement: &attune_common::models::event::Enforcement,
) -> bool {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.identity_attributes = identity_attributes.clone();
    ctx.target_id = Some(enforcement.id);
    ctx.target_ref = Some(enforcement.rule_ref.clone());
    ctx.pack_ref = enforcement
        .rule_ref
        .split_once('.')
        .map(|(pack, _)| pack.to_string());

    AuthorizationService::is_allowed(grants, Resource::Enforcements, RbacAction::Read, &ctx)
}

fn event_visible_for_trace(
    grants: &[Grant],
    identity_id: i64,
    identity_attributes: &HashMap<String, serde_json::Value>,
    event: &attune_common::models::event::Event,
) -> bool {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.identity_attributes = identity_attributes.clone();
    ctx.target_id = Some(event.id);
    ctx.target_ref = Some(event.trigger_ref.clone());
    ctx.pack_ref = event
        .trigger_ref
        .split_once('.')
        .map(|(pack, _)| pack.to_string());

    AuthorizationService::is_allowed(grants, Resource::Events, RbacAction::Read, &ctx)
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
