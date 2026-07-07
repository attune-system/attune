//! Inquiry management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::collections::HashMap;
use std::sync::Arc;
use validator::Validate;

use attune_common::{
    mq::{InquiryRespondedPayload, MessageEnvelope, MessageType},
    rbac::{Action as RbacAction, AuthorizationContext, Grant, Resource},
    repositories::{
        execution::ExecutionRepository,
        identity::IdentityRepository,
        inquiry::{
            CreateInquiryInput, InquiryRepository, InquirySearchFilters, InquiryVisibilityContext,
            UpdateInquiryInput,
        },
        Create, Delete, FindById, Update,
    },
};

use crate::auth::{
    jwt::TokenType,
    middleware::{AuthenticatedUser, RequireAuth},
};
use crate::{
    authz::AuthorizationService,
    dto::{
        common::{PaginatedResponse, PaginationParams},
        inquiry::{
            CreateInquiryRequest, InquiryQueryParams, InquiryRespondRequest, InquiryResponse,
            InquirySummary, UpdateInquiryRequest,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

/// List all inquiries with pagination and optional filters
#[utoipa::path(
    get,
    path = "/api/v1/inquiries",
    tag = "inquiries",
    params(InquiryQueryParams),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of inquiries", body = PaginatedResponse<InquirySummary>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_inquiries(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<InquiryQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let limit = query.limit.unwrap_or(50).min(500).max(1) as u32;
    let offset = query.offset.unwrap_or(0) as u32;

    let base_filters = InquirySearchFilters {
        status: query.status,
        execution: query.execution,
        assigned_to: query.assigned_to,
        limit: 0,
        offset: 0,
    };
    let pagination_params = PaginationParams {
        page: (offset / limit) + 1,
        page_size: limit,
    };

    let (items, has_next) = list_visible_inquiry_summaries(
        &state,
        &user,
        base_filters,
        offset as usize,
        limit as usize,
    )
    .await?;
    let response = PaginatedResponse::without_totals(items, &pagination_params, has_next);

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single inquiry by ID
#[utoipa::path(
    get,
    path = "/api/v1/inquiries/{id}",
    tag = "inquiries",
    params(
        ("id" = i64, Path, description = "Inquiry ID")
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Inquiry details", body = ApiResponse<InquiryResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Inquiry not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_inquiry(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let inquiry = InquiryRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Inquiry with ID {} not found", id)))?;

    let mut visibility = InquiryVisibilityEvaluator::new(&state, &user).await?;
    let decision = visibility.evaluate(&inquiry).await?;
    if !decision.content_visible {
        return Err(ApiError::NotFound(format!(
            "Inquiry with ID {} not found",
            id
        )));
    }

    let response = ApiResponse::new(redact_inquiry_response(
        InquiryResponse::from(inquiry),
        decision.execution_visible,
    ));

    Ok((StatusCode::OK, Json(response)))
}

/// List inquiries by status
#[utoipa::path(
    get,
    path = "/api/v1/inquiries/status/{status}",
    tag = "inquiries",
    params(
        ("status" = String, Path, description = "Inquiry status (pending, responded, timeout, canceled)"),
        PaginationParams
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of inquiries with specified status", body = PaginatedResponse<InquirySummary>),
        (status = 400, description = "Invalid status"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_inquiries_by_status(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(status_str): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Parse status from string
    let status = match status_str.to_lowercase().as_str() {
        "pending" => attune_common::models::enums::InquiryStatus::Pending,
        "responded" => attune_common::models::enums::InquiryStatus::Responded,
        "timeout" => attune_common::models::enums::InquiryStatus::Timeout,
        "canceled" => attune_common::models::enums::InquiryStatus::Cancelled,
        _ => {
            return Err(ApiError::BadRequest(format!(
            "Invalid inquiry status: '{}'. Valid values are: pending, responded, timeout, canceled",
            status_str
        )))
        }
    };

    let base_filters = InquirySearchFilters {
        status: Some(status),
        execution: None,
        assigned_to: None,
        limit: 0,
        offset: 0,
    };

    let (items, has_next) = list_visible_inquiry_summaries(
        &state,
        &user,
        base_filters,
        pagination.offset() as usize,
        pagination.limit() as usize,
    )
    .await?;
    let response = PaginatedResponse::without_totals(items, &pagination, has_next);

    Ok((StatusCode::OK, Json(response)))
}

/// List inquiries for a specific execution
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/inquiries",
    tag = "inquiries",
    params(
        ("execution_id" = i64, Path, description = "Execution ID"),
        PaginationParams
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of inquiries for execution", body = PaginatedResponse<InquirySummary>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Execution not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_inquiries_by_execution(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(execution_id): Path<i64>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Verify execution exists
    let _execution = ExecutionRepository::find_by_id(&state.db, execution_id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Execution with ID {} not found", execution_id))
        })?;

    let base_filters = InquirySearchFilters {
        status: None,
        execution: Some(execution_id),
        assigned_to: None,
        limit: 0,
        offset: 0,
    };

    let (items, has_next) = list_visible_inquiry_summaries(
        &state,
        &user,
        base_filters,
        pagination.offset() as usize,
        pagination.limit() as usize,
    )
    .await?;
    let response = PaginatedResponse::without_totals(items, &pagination, has_next);

    Ok((StatusCode::OK, Json(response)))
}

/// Create a new inquiry
#[utoipa::path(
    post,
    path = "/api/v1/inquiries",
    tag = "inquiries",
    request_body = CreateInquiryRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Inquiry created successfully", body = ApiResponse<InquiryResponse>),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Execution not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn create_inquiry(
    _user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateInquiryRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Verify execution exists
    let _execution = ExecutionRepository::find_by_id(&state.db, request.execution)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Execution with ID {} not found", request.execution))
        })?;

    // Create inquiry input
    let inquiry_input = CreateInquiryInput {
        execution: request.execution,
        prompt: request.prompt,
        response_schema: request.response_schema,
        assigned_to: request.assigned_to,
        status: attune_common::models::enums::InquiryStatus::Pending,
        response: None,
        timeout_at: request.timeout_at,
    };

    let inquiry = InquiryRepository::create(&state.db, inquiry_input).await?;

    let response = ApiResponse::with_message(
        InquiryResponse::from(inquiry),
        "Inquiry created successfully",
    );

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing inquiry
#[utoipa::path(
    put,
    path = "/api/v1/inquiries/{id}",
    tag = "inquiries",
    params(
        ("id" = i64, Path, description = "Inquiry ID")
    ),
    request_body = UpdateInquiryRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Inquiry updated successfully", body = ApiResponse<InquiryResponse>),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Inquiry not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn update_inquiry(
    _user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateInquiryRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Verify inquiry exists
    let _existing = InquiryRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Inquiry with ID {} not found", id)))?;

    // Create update input
    let update_input = UpdateInquiryInput {
        status: request.status,
        response: request.response,
        responded_at: None, // Let the database handle this if needed
        assigned_to: request.assigned_to,
    };

    let updated_inquiry = InquiryRepository::update(&state.db, id, update_input).await?;

    let response = ApiResponse::with_message(
        InquiryResponse::from(updated_inquiry),
        "Inquiry updated successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Respond to an inquiry (user-facing endpoint)
#[utoipa::path(
    post,
    path = "/api/v1/inquiries/{id}/respond",
    tag = "inquiries",
    params(
        ("id" = i64, Path, description = "Inquiry ID")
    ),
    request_body = InquiryRespondRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Response submitted successfully", body = ApiResponse<InquiryResponse>),
        (status = 400, description = "Invalid request or inquiry cannot be responded to"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not authorized to respond to this inquiry"),
        (status = 404, description = "Inquiry not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn respond_to_inquiry(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<InquiryRespondRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Verify inquiry exists and is in pending status
    let inquiry = InquiryRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Inquiry with ID {} not found", id)))?;

    // `InquiryVisibilityEvaluator` also validates that the caller's token
    // type is one this endpoint accepts (Access or Execution), and computes
    // `execution_visible` for redacting the linked execution id in the
    // response payload. Unlike `get_inquiry`/list endpoints, however, the
    // respond endpoint's *authorization* is governed by `assigned_to`
    // (below) and the privilege-loop guard, not by the general RBAC
    // "content visible" predicate. Gating on `content_visible` here was
    // incorrectly turning "not the assignee, no RBAC read grant" into a 404
    // (masking the intended 403), and turning unassigned inquiries (any
    // authenticated caller allowed) into a 404 for callers without a
    // matching grant.
    let mut visibility = InquiryVisibilityEvaluator::new(&state, &user.0).await?;
    let visibility_decision = visibility.evaluate(&inquiry).await?;

    // Check if inquiry is still pending
    if inquiry.status != attune_common::models::enums::InquiryStatus::Pending {
        return Err(ApiError::BadRequest(format!(
            "Cannot respond to inquiry with status '{:?}'. Only pending inquiries can be responded to.",
            inquiry.status
        )));
    }

    // Privilege-loop guard: an execution that created an inquiry (e.g., via
    // `core.ask`) must not be allowed to respond to it using its own
    // execution-scoped token. The triggering identity may still respond from
    // a separate session (their normal access token), but a callback bearing
    // the *same* execution scope as the one that created the inquiry would
    // create a self-approval loop.
    //
    // This guard also blocks any *descendant* execution of the creating
    // execution: a child action spawned by the inquiry-creating workflow
    // cannot respond to its ancestor's inquiry, since that would still
    // amount to a self-approval loop. We walk the `execution.parent` chain
    // from the token's execution upward, capped at depth 100 to bound work
    // and tolerate any corrupted chain (cycles).
    if let Some(token_exec_id) = user.0.execution_id() {
        let creating_exec_id = inquiry.execution;
        if token_exec_id == creating_exec_id {
            return Err(ApiError::Forbidden(
                "An execution cannot respond to an inquiry it created (privilege loop)".to_string(),
            ));
        }

        let mut current: Option<i64> = Some(token_exec_id);
        let mut depth = 0u32;
        let mut is_descendant = false;
        while let Some(cur) = current {
            if depth >= 100 {
                break;
            }
            if cur == creating_exec_id {
                is_descendant = true;
                break;
            }
            let parent: Option<Option<i64>> =
                sqlx::query_scalar("SELECT parent FROM execution WHERE id = $1")
                    .bind(cur)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e| ApiError::InternalServerError(format!("ancestry check: {e}")))?;
            current = parent.flatten();
            depth += 1;
        }
        if is_descendant {
            return Err(ApiError::Forbidden(
                "A descendant execution cannot respond to an ancestor's inquiry (privilege loop)"
                    .to_string(),
            ));
        }
    }

    // Resolve the responding identity strictly. Tokens without a parseable
    // identity in `sub` cannot produce a usable audit record, so reject up
    // front rather than silently writing `responded_by = NULL` later.
    let responded_by = user.0.identity_id().map_err(|_| {
        ApiError::Forbidden("Cannot record response: caller has no resolvable identity".to_string())
    })?;

    // Enforce assigned_to: only the assignee may respond.
    if let Some(assigned_to) = inquiry.assigned_to {
        if assigned_to != responded_by {
            return Err(ApiError::Forbidden(format!(
                "Inquiry {} is assigned to identity {} and can only be answered by them",
                id, assigned_to
            )));
        }
    }

    // Check if inquiry has timed out
    if let Some(timeout_at) = inquiry.timeout_at {
        if timeout_at < chrono::Utc::now() {
            // Update inquiry to timeout status
            let timeout_input = UpdateInquiryInput {
                status: Some(attune_common::models::enums::InquiryStatus::Timeout),
                response: None,
                responded_at: None,
                assigned_to: None,
            };
            let _ = InquiryRepository::update(&state.db, id, timeout_input).await?;

            return Err(ApiError::BadRequest(
                "Inquiry has timed out and can no longer be responded to".to_string(),
            ));
        }
    }

    // TODO: Validate response against response_schema if present
    // For now, just accept the response as-is

    // Create update input with response
    let update_input = UpdateInquiryInput {
        status: Some(attune_common::models::enums::InquiryStatus::Responded),
        response: Some(request.response.clone()),
        responded_at: Some(chrono::Utc::now()),
        assigned_to: None,
    };

    let updated_inquiry = InquiryRepository::update(&state.db, id, update_input).await?;

    // Publish InquiryResponded message if publisher is available
    if let Some(publisher) = state.get_publisher().await {
        let payload = InquiryRespondedPayload {
            inquiry_id: id,
            execution_id: inquiry.execution,
            response: request.response.clone(),
            responded_by: Some(responded_by),
            responded_at: chrono::Utc::now(),
        };

        let envelope =
            MessageEnvelope::new(MessageType::InquiryResponded, payload).with_source("api");

        if let Err(e) = publisher.publish_envelope(&envelope).await {
            tracing::error!("Failed to publish InquiryResponded message: {}", e);
            // Don't fail the request - inquiry is already saved
        } else {
            tracing::info!("Published InquiryResponded message for inquiry {}", id);
        }
    } else {
        tracing::warn!("No publisher available to publish InquiryResponded message");
    }

    let response = ApiResponse::with_message(
        redact_inquiry_response(
            InquiryResponse::from(updated_inquiry),
            visibility_decision.execution_visible,
        ),
        "Response submitted successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

const REDACTED_INQUIRY_EXECUTION_ID: i64 = 0;

#[derive(Debug, Clone, Copy)]
struct InquiryAccessDecision {
    content_visible: bool,
    execution_visible: bool,
}

/// Evaluates per-identity inquiry visibility for single-item endpoints
/// (`get_inquiry`, `respond_to_inquiry`).
///
/// List endpoints do *not* use this struct's `evaluate` in a scanning loop
/// any more — see [`list_visible_inquiry_summaries`], which pushes the same
/// participant/scope-reader predicate into SQL via
/// [`InquiryVisibilityContext`] instead. This struct (and the pure
/// [`execution_readable_from`] predicate it shares with the list path)
/// remains the single source of truth for the RBAC semantics.
struct InquiryVisibilityEvaluator {
    state: Arc<AppState>,
    identity_id: i64,
    identity_attributes: HashMap<String, serde_json::Value>,
    grants: Vec<Grant>,
    execution_cache: HashMap<i64, Option<attune_common::models::execution::Execution>>,
    ancestor_cache: HashMap<i64, Vec<i64>>,
}

impl InquiryVisibilityEvaluator {
    async fn new(state: &Arc<AppState>, user: &AuthenticatedUser) -> ApiResult<Self> {
        if !matches!(
            user.claims.token_type,
            TokenType::Access | TokenType::Execution
        ) {
            return Err(ApiError::Forbidden(
                "Inquiries are only available to access or execution identities".to_string(),
            ));
        }

        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let identity = IdentityRepository::find_by_id(&state.db, identity_id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("Identity not found".to_string()))?;

        let identity_attributes = match identity.attributes {
            serde_json::Value::Object(map) => map.into_iter().collect(),
            _ => HashMap::new(),
        };

        let grants = AuthorizationService::new(state.db.clone())
            .effective_grants(user)
            .await?;

        Ok(Self {
            state: state.clone(),
            identity_id,
            identity_attributes,
            grants,
            execution_cache: HashMap::new(),
            ancestor_cache: HashMap::new(),
        })
    }

    /// Bridges this evaluator's identity/attributes/grants into the
    /// repository-layer context used to build the SQL-side visibility
    /// predicate for list endpoints.
    fn as_visibility_context(&self) -> InquiryVisibilityContext {
        InquiryVisibilityContext {
            identity_id: self.identity_id,
            identity_attributes: self.identity_attributes.clone(),
            grants: self.grants.clone(),
        }
    }

    async fn evaluate(
        &mut self,
        inquiry: &attune_common::models::inquiry::Inquiry,
    ) -> ApiResult<InquiryAccessDecision> {
        let execution = self.linked_execution(inquiry.execution).await?;

        let participant = inquiry.assigned_to == Some(self.identity_id)
            || execution
                .as_ref()
                .and_then(|linked| linked.executor)
                .is_some_and(|executor| executor == self.identity_id);
        let scope_reader = self.inquiry_readable_with_scope(inquiry, execution.as_ref());
        let content_visible = participant || scope_reader;

        if !content_visible {
            return Ok(InquiryAccessDecision {
                content_visible: false,
                execution_visible: false,
            });
        }

        let execution_visible = if let Some(linked_execution) = execution.as_ref() {
            self.execution_readable(linked_execution).await?
        } else {
            false
        };

        Ok(InquiryAccessDecision {
            content_visible: true,
            execution_visible,
        })
    }

    async fn linked_execution(
        &mut self,
        execution_id: i64,
    ) -> ApiResult<Option<attune_common::models::execution::Execution>> {
        if let Some(cached) = self.execution_cache.get(&execution_id) {
            return Ok(cached.clone());
        }

        let execution = ExecutionRepository::find_by_id(&self.state.db, execution_id).await?;
        self.execution_cache.insert(execution_id, execution.clone());
        Ok(execution)
    }

    fn inquiry_readable_with_scope(
        &self,
        inquiry: &attune_common::models::inquiry::Inquiry,
        execution: Option<&attune_common::models::execution::Execution>,
    ) -> bool {
        let mut ctx = AuthorizationContext::new(self.identity_id);
        ctx.identity_attributes = self.identity_attributes.clone();
        ctx.target_id = Some(inquiry.id);
        ctx.target_ref = Some(format!("inquiry:{}", inquiry.id));
        if let Some(execution) = execution {
            ctx.pack_ref = execution
                .action_ref
                .split_once('.')
                .map(|(pack, _)| pack.to_string());
            ctx.owner_identity_id = execution.executor;
            ctx.execution_owner_identity_id = execution.executor;
        }

        AuthorizationService::is_allowed(&self.grants, Resource::Inquiries, RbacAction::Read, &ctx)
    }

    async fn execution_readable(
        &mut self,
        execution: &attune_common::models::execution::Execution,
    ) -> ApiResult<bool> {
        let ancestor_ids = self.execution_ancestor_identity_ids(execution.id).await?;
        Ok(execution_readable_from(
            &self.grants,
            self.identity_id,
            &self.identity_attributes,
            execution,
            &ancestor_ids,
        ))
    }

    /// Resolves ancestor executor identity IDs for a single execution with a
    /// single bulk-capable query (`ExecutionRepository::ancestor_executor_ids_by_ids`),
    /// instead of walking the `parent` chain one round trip per level.
    async fn execution_ancestor_identity_ids(&mut self, execution_id: i64) -> ApiResult<Vec<i64>> {
        if let Some(cached) = self.ancestor_cache.get(&execution_id) {
            return Ok(cached.clone());
        }

        let mut ancestor_map =
            ExecutionRepository::ancestor_executor_ids_by_ids(&self.state.db, &[execution_id])
                .await?;
        let ids = ancestor_map.remove(&execution_id).unwrap_or_default();
        self.ancestor_cache.insert(execution_id, ids.clone());
        Ok(ids)
    }
}

/// Pure execution-read predicate shared by the single-item evaluator and the
/// bulk list-page redaction path, so both compute `execution_visible`
/// identically regardless of how ancestor identities were fetched.
fn execution_readable_from(
    grants: &[Grant],
    identity_id: i64,
    identity_attributes: &HashMap<String, serde_json::Value>,
    execution: &attune_common::models::execution::Execution,
    ancestor_identity_ids: &[i64],
) -> bool {
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
    ctx.execution_ancestor_identity_ids = ancestor_identity_ids.to_vec();

    AuthorizationService::is_allowed(grants, Resource::Executions, RbacAction::Read, &ctx)
}

fn redact_inquiry_summary(mut summary: InquirySummary, execution_visible: bool) -> InquirySummary {
    if !execution_visible {
        summary.execution = REDACTED_INQUIRY_EXECUTION_ID;
    }
    summary
}

fn redact_inquiry_response(
    mut response: InquiryResponse,
    execution_visible: bool,
) -> InquiryResponse {
    if !execution_visible {
        response.execution = REDACTED_INQUIRY_EXECUTION_ID;
    }
    response
}

/// Lists the page of inquiries visible to `user`, applying the
/// participant/scope-reader predicate in SQL (via
/// [`InquiryRepository::search_visible`]) instead of scanning batches of
/// rows and evaluating RBAC per row in the application layer.
///
/// Query-count impact: regardless of how many inquiries exist or how many
/// are invisible to the caller, this issues a small, constant number of
/// queries — identity lookup, (cached) effective grants, one data query
/// (page size + 1 rows, to detect `has_next`), and two bulk queries to
/// resolve `execution_visible` (for redaction only) across the returned
/// page. Previously this scanned up to `INQUIRY_SCAN_ROW_LIMIT` rows in
/// batches and issued one execution lookup per scanned row.
async fn list_visible_inquiry_summaries(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    mut base_filters: InquirySearchFilters,
    visible_offset: usize,
    page_size: usize,
) -> ApiResult<(Vec<InquirySummary>, bool)> {
    let page_size = page_size.max(1);
    let evaluator = InquiryVisibilityEvaluator::new(state, user).await?;
    let vis_ctx = evaluator.as_visibility_context();

    // Fetch one extra row past the page to detect `has_next` without a
    // separate COUNT query; the visibility predicate is already applied by
    // `search_visible`, so `visible_offset`/`page_size` map directly onto
    // SQL LIMIT/OFFSET.
    base_filters.limit = (page_size + 1) as u32;
    base_filters.offset = visible_offset as u32;

    let mut rows = InquiryRepository::search_visible(&state.db, &base_filters, &vis_ctx).await?;

    let has_next = rows.len() > page_size;
    if has_next {
        rows.truncate(page_size);
    }

    let items = redact_page_execution_visibility(state, &vis_ctx, rows).await?;

    Ok((items, has_next))
}

/// Resolves `execution_visible` (used only to decide whether to redact the
/// `execution` field) for an already visibility-filtered page of inquiries,
/// using two bulk queries — one for the linked executions, one for their
/// ancestor executor identities — instead of one round trip per row.
async fn redact_page_execution_visibility(
    state: &Arc<AppState>,
    ctx: &InquiryVisibilityContext,
    inquiries: Vec<attune_common::models::inquiry::Inquiry>,
) -> ApiResult<Vec<InquirySummary>> {
    if inquiries.is_empty() {
        return Ok(Vec::new());
    }

    let mut execution_ids: Vec<i64> = inquiries.iter().map(|inquiry| inquiry.execution).collect();
    execution_ids.sort_unstable();
    execution_ids.dedup();

    let executions = ExecutionRepository::find_by_ids(&state.db, &execution_ids).await?;
    let executions_by_id: HashMap<i64, attune_common::models::execution::Execution> = executions
        .into_iter()
        .map(|execution| (execution.id, execution))
        .collect();

    let ancestor_ids_by_execution =
        ExecutionRepository::ancestor_executor_ids_by_ids(&state.db, &execution_ids).await?;

    let items = inquiries
        .into_iter()
        .map(|inquiry| {
            let execution_visible =
                executions_by_id
                    .get(&inquiry.execution)
                    .is_some_and(|execution| {
                        let ancestors = ancestor_ids_by_execution
                            .get(&execution.id)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        execution_readable_from(
                            &ctx.grants,
                            ctx.identity_id,
                            &ctx.identity_attributes,
                            execution,
                            ancestors,
                        )
                    });
            redact_inquiry_summary(InquirySummary::from(inquiry), execution_visible)
        })
        .collect();

    Ok(items)
}

/// Delete an inquiry
#[utoipa::path(
    delete,
    path = "/api/v1/inquiries/{id}",
    tag = "inquiries",
    params(
        ("id" = i64, Path, description = "Inquiry ID")
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Inquiry deleted successfully", body = SuccessResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Inquiry not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn delete_inquiry(
    _user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    // Verify inquiry exists
    let _inquiry = InquiryRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Inquiry with ID {} not found", id)))?;

    // Delete the inquiry
    let deleted = InquiryRepository::delete(&state.db, id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Inquiry with ID {} not found",
            id
        )));
    }

    let response = SuccessResponse::new("Inquiry deleted successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Register inquiry routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/inquiries", get(list_inquiries).post(create_inquiry))
        .route(
            "/inquiries/{id}",
            get(get_inquiry).put(update_inquiry).delete(delete_inquiry),
        )
        .route("/inquiries/status/{status}", get(list_inquiries_by_status))
        .route(
            "/executions/{execution_id}/inquiries",
            get(list_inquiries_by_execution),
        )
        .route("/inquiries/{id}/respond", post(respond_to_inquiry))
}
