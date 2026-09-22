//! Inquiry management API routes

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::collections::HashMap;
use std::sync::Arc;
use validator::Validate;

use attune_common::{
    inquiry_options::validate_response_options,
    inquiry_response_handle::issue_inquiry_response_handle,
    models::{execution::Execution, inquiry::Inquiry},
    rbac::{Action as RbacAction, AuthorizationContext, Grant, Resource},
    repositories::{
        execution::ExecutionRepository,
        identity::IdentityRepository,
        inquiry::{
            CreateWorkflowInquiryInput, InquiryContext, InquiryRepository, InquirySearchFilters,
            InquiryVisibilityContext,
        },
        FindById,
    },
};

use crate::auth::{
    jwt::TokenType,
    middleware::{AuthenticatedUser, RequireAuth},
};
use crate::{
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        inquiry::{
            CreateInquiryRequest, CreateInquiryResponse, InquiryQueryParams, InquiryRespondRequest,
            InquiryResponse, InquiryResponseOptionHandle, InquirySummary,
        },
        ApiResponse,
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
    let limit = query.limit.unwrap_or(50).clamp(1, 500) as u32;
    let offset = query.offset.unwrap_or(0) as u32;

    let base_filters = InquirySearchFilters {
        status: query.status,
        created_by_execution: query.created_by_execution,
        assigned_to: query.assigned_to,
        workflow_action_ref: query.workflow_action_ref,
        workflow_pack_ref: query.workflow_pack_ref,
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

    let vis_ctx = visibility.as_visibility_context();
    let mut enrichment =
        load_inquiry_enrichment(&state, &vis_ctx, std::slice::from_ref(&inquiry)).await?;
    let mut response = InquiryResponse::from(inquiry);
    let response_id = response.id;
    apply_response_enrichment(&mut response, enrichment.remove(&response_id));
    if !decision.execution_visible {
        response.created_by_execution = REDACTED_CREATED_BY_EXECUTION_ID;
        response.created_by_action_ref = None;
        response.created_by_pack_ref = None;
    }
    let response = ApiResponse::new(response);

    Ok((StatusCode::OK, Json(response)))
}

/// List inquiries by status
#[utoipa::path(
    get,
    path = "/api/v1/inquiries/status/{status}",
    tag = "inquiries",
    params(
        ("status" = String, Path, description = "Inquiry status (pending, responded, timeout, cancelled)"),
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
        "cancelled" => attune_common::models::enums::InquiryStatus::Cancelled,
        _ => {
            return Err(ApiError::BadRequest(format!(
            "Invalid inquiry status: '{}'. Valid values are: pending, responded, timeout, cancelled",
            status_str
        )))
        }
    };

    let base_filters = InquirySearchFilters {
        status: Some(status),
        created_by_execution: None,
        assigned_to: None,
        workflow_action_ref: None,
        workflow_pack_ref: None,
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

/// List inquiries created by a specific execution
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
        (status = 200, description = "List of inquiries created by execution", body = PaginatedResponse<InquirySummary>),
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
        created_by_execution: Some(execution_id),
        assigned_to: None,
        workflow_action_ref: None,
        workflow_pack_ref: None,
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
        (status = 201, description = "Inquiry and one-shot response handle created", body = ApiResponse<CreateInquiryResponse>),
        (status = 400, description = "Malformed request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Execution token or inquiries:create permission required"),
        (status = 404, description = "Execution not found"),
        (status = 409, description = "Idempotent creation fields differ"),
        (status = 422, description = "Inquiry request, schema, or options are invalid"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn create_inquiry(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateInquiryRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;
    validate_response_options(request.response_schema.as_ref(), &request.response_options)?;

    if user.claims.token_type != TokenType::Execution {
        return Err(ApiError::Forbidden(
            "Workflow inquiries must be created with an execution token".to_string(),
        ));
    }
    let execution = user.execution_id().ok_or_else(|| {
        ApiError::Unauthorized("Execution token is missing its execution scope".to_string())
    })?;
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    state
        .authorization_service()
        .authorize(
            &user,
            AuthorizationCheck {
                resource: Resource::Inquiries,
                action: RbacAction::Create,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await?;

    let encryption_key = state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::InternalServerError(
                "Cannot issue inquiry response handles without security.encryption_key".to_string(),
            )
        })?;

    let inquiry_input = CreateWorkflowInquiryInput {
        created_by_execution: execution,
        purpose: request.purpose,
        prompt: request.prompt,
        response_schema: request.response_schema,
        response_options: request.response_options,
        assigned_to: request.assigned_to,
        timeout_seconds: request.timeout_seconds,
    };

    let mut conn = state.db.acquire().await?;
    let inquiry =
        InquiryRepository::create_workflow_inquiry_idempotent(&mut conn, inquiry_input).await?;

    let response_options = inquiry
        .response_options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let option_index = u16::try_from(index).map_err(|_| {
                ApiError::InternalServerError("Too many inquiry response options".to_string())
            })?;
            Ok(InquiryResponseOptionHandle {
                r#ref: option.r#ref.clone(),
                label: option.label.clone(),
                style: option.style,
                response_handle: issue_inquiry_response_handle(
                    inquiry.id,
                    option_index,
                    encryption_key,
                )
                .map_err(ApiError::from)?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let response = ApiResponse::with_message(
        CreateInquiryResponse {
            inquiry: InquiryResponse::from(inquiry),
            response_options,
        },
        "Inquiry created successfully",
    );

    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(response),
    ))
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
        (status = 400, description = "Malformed request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not authorized to respond to this inquiry"),
        (status = 404, description = "Inquiry not found"),
        (status = 409, description = "Inquiry is no longer pending"),
        (status = 422, description = "Response does not conform to the inquiry schema"),
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

    let responded_by = user.0.identity_id().map_err(|_| {
        ApiError::Forbidden("Cannot record response: caller has no resolvable identity".to_string())
    })?;
    let updated_inquiry = crate::inquiry_response::submit_inquiry_response(
        &state,
        crate::inquiry_response::InquiryResponseSubmission::Human {
            inquiry_id: id,
            response: request.response,
            identity_id: responded_by,
            execution_id: user.0.execution_id(),
        },
    )
    .await?;

    let response = ApiResponse::with_message(
        redact_inquiry_response(
            InquiryResponse::from(updated_inquiry),
            visibility_decision.execution_visible,
        ),
        "Response submitted successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

const REDACTED_CREATED_BY_EXECUTION_ID: i64 = 0;

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

        let grants = state.authorization_service().effective_grants(user).await?;

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

    fn can_filter_workflow(&self, filters: &InquirySearchFilters) -> bool {
        if filters.workflow_action_ref.is_none() && filters.workflow_pack_ref.is_none() {
            return true;
        }

        let mut ctx = AuthorizationContext::new(self.identity_id);
        ctx.identity_attributes = self.identity_attributes.clone();
        ctx.target_ref = filters.workflow_action_ref.clone();
        ctx.pack_ref = filters.workflow_pack_ref.clone().or_else(|| {
            filters
                .workflow_action_ref
                .as_deref()
                .and_then(|action_ref| action_ref.split_once('.'))
                .map(|(pack_ref, _)| pack_ref.to_string())
        });

        AuthorizationService::is_allowed(&self.grants, Resource::Executions, RbacAction::Read, &ctx)
    }

    async fn evaluate(
        &mut self,
        inquiry: &attune_common::models::inquiry::Inquiry,
    ) -> ApiResult<InquiryAccessDecision> {
        let execution = self.linked_execution(inquiry.created_by_execution).await?;

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

fn redact_inquiry_response(
    mut response: InquiryResponse,
    execution_visible: bool,
) -> InquiryResponse {
    if !execution_visible {
        response.created_by_execution = REDACTED_CREATED_BY_EXECUTION_ID;
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
    if !evaluator.can_filter_workflow(&base_filters) {
        return Err(ApiError::Forbidden(
            "Workflow inquiry filters require permission to read the matching executions"
                .to_string(),
        ));
    }
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

    let enrichment = load_inquiry_enrichment(state, &vis_ctx, &rows).await?;
    let items = rows
        .into_iter()
        .map(|inquiry| {
            let mut summary = InquirySummary::from(inquiry);
            let summary_id = summary.id;
            apply_summary_enrichment(&mut summary, enrichment.get(&summary_id));
            summary
        })
        .collect();

    Ok((items, has_next))
}

#[derive(Debug, Default)]
struct InquiryEnrichment {
    context: Option<InquiryContext>,
    creator_execution: Option<Execution>,
    creator_execution_visible: bool,
    workflow_execution: Option<Execution>,
    workflow_execution_visible: bool,
}

async fn load_inquiry_enrichment(
    state: &Arc<AppState>,
    ctx: &InquiryVisibilityContext,
    inquiries: &[Inquiry],
) -> ApiResult<HashMap<i64, InquiryEnrichment>> {
    if inquiries.is_empty() {
        return Ok(HashMap::new());
    }

    let inquiry_ids: Vec<i64> = inquiries.iter().map(|inquiry| inquiry.id).collect();
    let contexts = InquiryRepository::find_contexts_by_ids(&state.db, &inquiry_ids).await?;
    let contexts_by_id: HashMap<i64, InquiryContext> = contexts
        .into_iter()
        .map(|context| (context.inquiry_id, context))
        .collect();

    let mut execution_ids: Vec<i64> = inquiries
        .iter()
        .map(|inquiry| inquiry.created_by_execution)
        .collect();
    execution_ids.extend(
        contexts_by_id
            .values()
            .filter_map(|context| context.workflow_root_execution_id),
    );
    execution_ids.sort_unstable();
    execution_ids.dedup();

    let executions = ExecutionRepository::find_by_ids(&state.db, &execution_ids).await?;
    let executions_by_id: HashMap<i64, attune_common::models::execution::Execution> = executions
        .into_iter()
        .map(|execution| (execution.id, execution))
        .collect();

    let ancestor_ids_by_execution =
        ExecutionRepository::ancestor_executor_ids_by_ids(&state.db, &execution_ids).await?;

    let enrichment = inquiries
        .iter()
        .map(|inquiry| {
            let context = contexts_by_id.get(&inquiry.id).cloned();
            let creator_execution = executions_by_id.get(&inquiry.created_by_execution).cloned();
            let creator_execution_visible = creator_execution.as_ref().is_some_and(|execution| {
                execution_is_visible(ctx, execution, &ancestor_ids_by_execution)
            });
            let workflow_execution = context
                .as_ref()
                .and_then(|context| context.workflow_root_execution_id)
                .and_then(|execution_id| executions_by_id.get(&execution_id))
                .cloned();
            let workflow_execution_visible = workflow_execution.as_ref().is_some_and(|execution| {
                execution_is_visible(ctx, execution, &ancestor_ids_by_execution)
            });

            (
                inquiry.id,
                InquiryEnrichment {
                    context,
                    creator_execution,
                    creator_execution_visible,
                    workflow_execution,
                    workflow_execution_visible,
                },
            )
        })
        .collect();

    Ok(enrichment)
}

fn execution_is_visible(
    ctx: &InquiryVisibilityContext,
    execution: &Execution,
    ancestor_ids_by_execution: &HashMap<i64, Vec<i64>>,
) -> bool {
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
}

fn action_pack_ref(action_ref: &str) -> Option<String> {
    action_ref
        .split_once('.')
        .map(|(pack_ref, _)| pack_ref.to_string())
}

fn apply_summary_enrichment(summary: &mut InquirySummary, enrichment: Option<&InquiryEnrichment>) {
    let Some(enrichment) = enrichment else {
        summary.created_by_execution = REDACTED_CREATED_BY_EXECUTION_ID;
        summary.workflow_execution = None;
        summary.workflow_task_name = None;
        return;
    };

    if enrichment.creator_execution_visible {
        if let Some(execution) = &enrichment.creator_execution {
            summary.created_by_action_ref = Some(execution.action_ref.clone());
            summary.created_by_pack_ref = action_pack_ref(&execution.action_ref);
        }
    } else {
        summary.created_by_execution = REDACTED_CREATED_BY_EXECUTION_ID;
    }

    if enrichment.workflow_execution_visible {
        if let Some(execution) = &enrichment.workflow_execution {
            summary.workflow_root_execution = Some(execution.id);
            summary.workflow_action_ref = Some(execution.action_ref.clone());
            summary.workflow_pack_ref = action_pack_ref(&execution.action_ref);
        }
    } else {
        summary.workflow_execution = None;
        summary.workflow_task_name = None;
    }

    if let Some(context) = &enrichment.context {
        summary.assigned_to_login = context.assigned_to_login.clone();
        summary.assigned_to_display_name = context.assigned_to_display_name.clone();
    }
}

fn apply_response_enrichment(
    response: &mut InquiryResponse,
    enrichment: Option<InquiryEnrichment>,
) {
    let Some(enrichment) = enrichment else {
        response.created_by_execution = REDACTED_CREATED_BY_EXECUTION_ID;
        response.workflow_execution = None;
        response.workflow_task_name = None;
        return;
    };

    if enrichment.creator_execution_visible {
        if let Some(execution) = &enrichment.creator_execution {
            response.created_by_action_ref = Some(execution.action_ref.clone());
            response.created_by_pack_ref = action_pack_ref(&execution.action_ref);
        }
    } else {
        response.created_by_execution = REDACTED_CREATED_BY_EXECUTION_ID;
    }

    if enrichment.workflow_execution_visible {
        if let Some(execution) = &enrichment.workflow_execution {
            response.workflow_root_execution = Some(execution.id);
            response.workflow_action_ref = Some(execution.action_ref.clone());
            response.workflow_pack_ref = action_pack_ref(&execution.action_ref);
        }
    } else {
        response.workflow_execution = None;
        response.workflow_task_name = None;
    }

    if let Some(context) = enrichment.context {
        response.assigned_to_login = context.assigned_to_login;
        response.assigned_to_display_name = context.assigned_to_display_name;
        response.responded_by_login = context.responded_by_login;
        response.responded_by_display_name = context.responded_by_display_name;
    }
}

/// Cancel an inquiry from its creator execution.
#[utoipa::path(
    post,
    path = "/api/v1/inquiries/{id}/cancel",
    tag = "inquiries",
    params(
        ("id" = i64, Path, description = "Inquiry ID")
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Inquiry cancelled", body = ApiResponse<InquiryResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Only the creator execution can cancel the inquiry"),
        (status = 409, description = "Inquiry is no longer pending"),
        (status = 404, description = "Inquiry not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn cancel_inquiry(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    if user.claims.token_type != TokenType::Execution {
        return Err(ApiError::Forbidden(
            "Only an execution token can cancel an action-owned inquiry".to_string(),
        ));
    }
    let creator_execution = user.execution_id().ok_or_else(|| {
        ApiError::Unauthorized("Execution token is missing its execution scope".to_string())
    })?;
    let inquiry = InquiryRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Inquiry with ID {} not found", id)))?;
    if inquiry.created_by_execution != creator_execution {
        return Err(ApiError::Forbidden(
            "Only the creator execution can cancel this inquiry".to_string(),
        ));
    }
    let cancelled = InquiryRepository::cancel_pending_by_creator(&state.db, id, creator_execution)
        .await?
        .ok_or_else(|| ApiError::Conflict("Inquiry is no longer pending".to_string()))?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            InquiryResponse::from(cancelled),
            "Inquiry cancelled",
        )),
    ))
}

/// Register inquiry routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/inquiries", get(list_inquiries).post(create_inquiry))
        .route("/inquiries/{id}", get(get_inquiry))
        .route("/inquiries/status/{status}", get(list_inquiries_by_status))
        .route(
            "/executions/{execution_id}/inquiries",
            get(list_inquiries_by_execution),
        )
        .route("/inquiries/{id}/respond", post(respond_to_inquiry))
        .route("/inquiries/{id}/cancel", post(cancel_inquiry))
}
