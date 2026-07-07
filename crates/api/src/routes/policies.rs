//! Policy management API routes.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::sync::Arc;
use validator::Validate;

use attune_common::models::Policy;
use attune_common::rbac::{
    Action, AuthorizationContext, ExecutionScopeConstraint, Grant, GrantConstraints,
    OwnerConstraint, Resource,
};
use attune_common::repositories::{
    action::{
        ActionRepository, CreatePolicyInput, PolicyRepository, PolicyScopeFilter,
        PolicySearchFilters, PolicyVisibilityFilter, PolicyVisibilityScope, UpdatePolicyInput,
    },
    pack::PackRepository,
    Create, Delete, FindByRef, Update,
};

use crate::{
    auth::{
        jwt::TokenType,
        middleware::{AuthenticatedUser, RequireAuth},
    },
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        policy::{
            quotas_to_json, CreatePolicyRequest, PolicyListParams, PolicyResponse, PolicyScopeType,
            PolicySummary, UpdatePolicyRequest,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

const SUPPORTED_QUOTA_TYPES: &[&str] = &["running_executions", "executions_total"];

#[utoipa::path(
    get,
    path = "/api/v1/policies",
    tag = "policies",
    params(PolicyListParams),
    responses((status = 200, description = "List of policies", body = PaginatedResponse<PolicySummary>)),
    security(("bearer_auth" = []))
)]
pub async fn list_policies(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<PolicyListParams>,
) -> ApiResult<impl IntoResponse> {
    let pagination = PaginationParams {
        page: query.page,
        page_size: query.page_size,
    };
    let filters = PolicySearchFilters {
        pack: None,
        pack_ref: query.pack_ref,
        action: None,
        action_ref: query.action_ref,
        scope: query.scope.map(scope_filter),
        enabled: query.enabled,
        tag: query.tag,
        ..Default::default()
    };
    let response = list_visible_policies(&state, &user, &pagination, filters).await?;
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/packs/{pack_ref}/policies",
    tag = "policies",
    params(("pack_ref" = String, Path, description = "Pack reference"), PaginationParams),
    responses((status = 200, description = "List of policies for a pack", body = PaginatedResponse<PolicySummary>)),
    security(("bearer_auth" = []))
)]
pub async fn list_policies_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;
    let filters = PolicySearchFilters {
        pack: Some(pack.id),
        ..Default::default()
    };
    let response = list_visible_policies(&state, &user, &pagination, filters).await?;
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/actions/{action_ref}/policies",
    tag = "policies",
    params(("action_ref" = String, Path, description = "Action reference"), PaginationParams),
    responses((status = 200, description = "List of policies for an action", body = PaginatedResponse<PolicySummary>)),
    security(("bearer_auth" = []))
)]
pub async fn list_policies_by_action(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(action_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    let action = ActionRepository::find_by_ref(&state.db, &action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", action_ref)))?;
    let filters = PolicySearchFilters {
        action: Some(action.id),
        ..Default::default()
    };
    let response = list_visible_policies(&state, &user, &pagination, filters).await?;
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/policies/{ref}",
    tag = "policies",
    params(("ref" = String, Path, description = "Policy reference")),
    responses((status = 200, description = "Policy details", body = ApiResponse<PolicyResponse>)),
    security(("bearer_auth" = []))
)]
pub async fn get_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(policy_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let policy = find_policy(&state, &policy_ref).await?;
    if let Err(err) = authorize_for_policy(&state, &user, Action::Read, &policy).await {
        return match err {
            // Unauthorized single-resource reads are shaped as 404s so a
            // policy's existence is not leaked to identities who cannot see it.
            ApiError::Forbidden(_) => Err(ApiError::NotFound(format!(
                "Policy '{}' not found",
                policy_ref
            ))),
            other => Err(other),
        };
    }
    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(PolicyResponse::from(policy))),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/policies",
    tag = "policies",
    request_body = CreatePolicyRequest,
    responses((status = 201, description = "Policy created", body = ApiResponse<PolicyResponse>)),
    security(("bearer_auth" = []))
)]
pub async fn create_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreatePolicyRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    validate_policy_features(
        request.concurrency.is_some(),
        request.rate_limit.is_some(),
        !request.quotas.is_empty(),
    )?;
    validate_quotas(&request.quotas)?;

    if PolicyRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Policy with ref '{}' already exists",
            request.r#ref
        )));
    }

    let resolved = resolve_scope(&state, &request.scope).await?;
    authorize_policy_action(
        &state,
        &user,
        Action::Create,
        resolved.pack_ref.as_deref(),
        resolved.action_ref.as_deref(),
        Some(&request.r#ref),
    )
    .await?;

    let input = CreatePolicyInput {
        r#ref: request.r#ref,
        pack: resolved.pack,
        pack_ref: resolved.pack_ref,
        action: resolved.action,
        action_ref: resolved.action_ref,
        enabled: request.enabled,
        priority: request.priority,
        parameters: request
            .concurrency
            .as_ref()
            .map(|concurrency| concurrency.parameters.clone())
            .unwrap_or_default(),
        method: request
            .concurrency
            .as_ref()
            .map(|concurrency| concurrency.method),
        threshold: request
            .concurrency
            .as_ref()
            .map(|concurrency| concurrency.limit),
        rate_limit_max_executions: request
            .rate_limit
            .as_ref()
            .map(|rate_limit| rate_limit.max_executions),
        rate_limit_window_seconds: request
            .rate_limit
            .as_ref()
            .map(|rate_limit| rate_limit.window_seconds),
        quotas: quotas_to_json(&request.quotas),
        name: request.name,
        description: request.description,
        tags: request.tags,
    };

    let policy = PolicyRepository::create(&state.db, input).await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            PolicyResponse::from(policy),
            "Policy created successfully",
        )),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/policies/{ref}",
    tag = "policies",
    params(("ref" = String, Path, description = "Policy reference")),
    request_body = UpdatePolicyRequest,
    responses((status = 200, description = "Policy updated", body = ApiResponse<PolicyResponse>)),
    security(("bearer_auth" = []))
)]
pub async fn update_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(policy_ref): Path<String>,
    Json(request): Json<UpdatePolicyRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    let existing = find_policy(&state, &policy_ref).await?;
    authorize_for_policy(&state, &user, Action::Update, &existing).await?;

    if let Some(quotas) = &request.quotas {
        validate_quotas(quotas)?;
    }
    validate_update_keeps_feature(&existing, &request)?;

    let update = UpdatePolicyInput {
        enabled: request.enabled,
        priority: request.priority,
        parameters: request.concurrency.as_ref().map(|concurrency| {
            concurrency
                .as_ref()
                .map(|value| value.parameters.clone())
                .unwrap_or_default()
        }),
        method: request
            .concurrency
            .as_ref()
            .map(|concurrency| concurrency.as_ref().map(|value| value.method)),
        threshold: request
            .concurrency
            .as_ref()
            .map(|concurrency| concurrency.as_ref().map(|value| value.limit)),
        rate_limit_max_executions: request
            .rate_limit
            .as_ref()
            .map(|rate_limit| rate_limit.as_ref().map(|value| value.max_executions)),
        rate_limit_window_seconds: request
            .rate_limit
            .as_ref()
            .map(|rate_limit| rate_limit.as_ref().map(|value| value.window_seconds)),
        quotas: request.quotas.as_ref().map(|quotas| quotas_to_json(quotas)),
        name: request.name,
        description: request.description,
        tags: request.tags,
    };

    let policy = PolicyRepository::update(&state.db, existing.id, update).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            PolicyResponse::from(policy),
            "Policy updated successfully",
        )),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/policies/{ref}",
    tag = "policies",
    params(("ref" = String, Path, description = "Policy reference")),
    responses((status = 200, description = "Policy deleted", body = ApiResponse<SuccessResponse>)),
    security(("bearer_auth" = []))
)]
pub async fn delete_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(policy_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let policy = find_policy(&state, &policy_ref).await?;
    authorize_for_policy(&state, &user, Action::Delete, &policy).await?;
    PolicyRepository::delete(&state.db, policy.id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            SuccessResponse::new("Policy deleted successfully"),
            "Policy deleted successfully",
        )),
    ))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/policies", get(list_policies).post(create_policy))
        .route(
            "/policies/{ref}",
            get(get_policy).put(update_policy).delete(delete_policy),
        )
        .route("/packs/{pack_ref}/policies", get(list_policies_by_pack))
        .route(
            "/actions/{action_ref}/policies",
            get(list_policies_by_action),
        )
}

struct ResolvedScope {
    pack: Option<i64>,
    pack_ref: Option<String>,
    action: Option<i64>,
    action_ref: Option<String>,
}

async fn resolve_scope(
    state: &Arc<AppState>,
    scope: &crate::dto::policy::PolicyScopeRequest,
) -> ApiResult<ResolvedScope> {
    match scope.r#type {
        PolicyScopeType::Global => Ok(ResolvedScope {
            pack: None,
            pack_ref: None,
            action: None,
            action_ref: None,
        }),
        PolicyScopeType::Pack => {
            let pack_ref = scope.pack_ref.as_deref().ok_or_else(|| {
                ApiError::BadRequest("pack_ref is required for pack-scoped policies".to_string())
            })?;
            let pack = PackRepository::find_by_ref(&state.db, pack_ref)
                .await?
                .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;
            Ok(ResolvedScope {
                pack: Some(pack.id),
                pack_ref: Some(pack.r#ref),
                action: None,
                action_ref: None,
            })
        }
        PolicyScopeType::Action => {
            let action_ref = scope.action_ref.as_deref().ok_or_else(|| {
                ApiError::BadRequest(
                    "action_ref is required for action-scoped policies".to_string(),
                )
            })?;
            let action = ActionRepository::find_by_ref(&state.db, action_ref)
                .await?
                .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", action_ref)))?;
            Ok(ResolvedScope {
                pack: Some(action.pack),
                pack_ref: Some(action.pack_ref.clone()),
                action: Some(action.id),
                action_ref: Some(action.r#ref.clone()),
            })
        }
    }
}

async fn find_policy(state: &Arc<AppState>, policy_ref: &str) -> ApiResult<Policy> {
    PolicyRepository::find_by_ref(&state.db, policy_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Policy '{}' not found", policy_ref)))
}

async fn authorize_for_policy(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: Action,
    policy: &Policy,
) -> ApiResult<()> {
    authorize_policy_action(
        state,
        user,
        action,
        policy.pack_ref.as_deref(),
        policy.action_ref.as_deref(),
        Some(&policy.r#ref),
    )
    .await
}

async fn authorize_policy_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: Action,
    pack_ref: Option<&str>,
    action_ref: Option<&str>,
    target_ref: Option<&str>,
) -> ApiResult<()> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.pack_ref = pack_ref.map(str::to_string);
    ctx.owner_ref = action_ref.map(str::to_string);
    ctx.target_ref = target_ref.map(str::to_string);
    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Policies,
                action,
                context: ctx,
            },
        )
        .await
}

/// Lists policies visible to `user`, applying RBAC visibility entirely in
/// SQL (see [`build_policy_visibility_filter`]) instead of a coarse
/// all-or-nothing gate, so pagination and totals are accurate and every
/// identity only ever sees the policies its grants actually cover.
async fn list_visible_policies(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    pagination: &PaginationParams,
    mut filters: PolicySearchFilters,
) -> ApiResult<PaginatedResponse<PolicySummary>> {
    if matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let grants = AuthorizationService::new(state.db.clone())
            .effective_grants(user)
            .await?;
        filters.visibility = Some(build_policy_visibility_filter(&grants));
    }

    filters.limit = pagination.limit() as i64;
    filters.offset = pagination.offset() as i64;
    let result = PolicyRepository::list_search(&state.db, &filters).await?;
    Ok(PaginatedResponse::new(
        result.rows.into_iter().map(PolicySummary::from).collect(),
        pagination,
        result.total,
    ))
}

/// Translates a token's effective RBAC grants into a SQL-evaluable
/// [`PolicyVisibilityFilter`]. Each qualifying grant becomes one OR-branch
/// (scope); an unconstrained grant short-circuits to "match everything".
/// Grants whose constraints can never be satisfied for a policy's
/// authorization context (e.g. `owner_types`, `visibility`, non-`Any`
/// `execution_scope`, `encrypted`, or non-empty `attributes` -- none of
/// which policies ever populate) are skipped, mirroring the row-level
/// semantics previously enforced by evaluating `Grant::allows` per row in
/// memory.
fn build_policy_visibility_filter(grants: &[Grant]) -> PolicyVisibilityFilter {
    let mut scopes = Vec::new();
    for grant in grants {
        if grant.resource != Resource::Policies || !grant.actions.contains(&Action::Read) {
            continue;
        }
        let Some(constraints) = &grant.constraints else {
            // Fully unconstrained read grant: every policy is visible.
            return PolicyVisibilityFilter {
                scopes: vec![PolicyVisibilityScope::default()],
            };
        };
        if !policy_grant_context_feasible(constraints) {
            continue;
        }
        scopes.push(PolicyVisibilityScope {
            pack_refs: constraints.pack_refs.clone(),
            action_refs: constraints.owner_refs.clone(),
            refs: constraints.refs.clone(),
            ids: constraints.ids.clone(),
        });
    }
    PolicyVisibilityFilter { scopes }
}

/// Returns `false` when `constraints` depend on authorization-context fields
/// that are never populated for policy visibility checks (policies have no
/// owner identity, artifact visibility, execution scope, or encryption
/// flag), meaning the grant could never match any policy row.
fn policy_grant_context_feasible(constraints: &GrantConstraints) -> bool {
    if let Some(owner) = constraints.owner {
        // `ctx.owner_identity_id` is always `None` for policies, so only
        // `SelfOnly` (which requires a match) is infeasible; `Any`/`None`
        // hold unconditionally and add no row-level restriction.
        if matches!(owner, OwnerConstraint::SelfOnly) {
            return false;
        }
    }
    if constraints.owner_types.is_some() {
        return false;
    }
    if constraints.visibility.is_some() {
        return false;
    }
    if let Some(execution_scope) = constraints.execution_scope {
        if !matches!(execution_scope, ExecutionScopeConstraint::Any) {
            return false;
        }
    }
    if constraints.encrypted.is_some() {
        return false;
    }
    if let Some(attributes) = &constraints.attributes {
        if !attributes.is_empty() {
            return false;
        }
    }
    true
}

fn scope_filter(scope: PolicyScopeType) -> PolicyScopeFilter {
    match scope {
        PolicyScopeType::Global => PolicyScopeFilter::Global,
        PolicyScopeType::Pack => PolicyScopeFilter::Pack,
        PolicyScopeType::Action => PolicyScopeFilter::Action,
    }
}

fn validate_quotas(quotas: &[crate::dto::policy::QuotaPolicyRequest]) -> ApiResult<()> {
    for quota in quotas {
        if !SUPPORTED_QUOTA_TYPES.contains(&quota.quota_type.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "Unsupported quota type '{}'. Supported quota types: {}",
                quota.quota_type,
                SUPPORTED_QUOTA_TYPES.join(", ")
            )));
        }
    }
    Ok(())
}

fn validate_policy_features(concurrency: bool, rate_limit: bool, quotas: bool) -> ApiResult<()> {
    if concurrency || rate_limit || quotas {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "At least one policy feature must be configured".to_string(),
        ))
    }
}

fn validate_update_keeps_feature(
    existing: &Policy,
    request: &UpdatePolicyRequest,
) -> ApiResult<()> {
    let has_concurrency = request
        .concurrency
        .as_ref()
        .map(|value| value.is_some())
        .unwrap_or(existing.threshold.is_some());
    let has_rate_limit = request
        .rate_limit
        .as_ref()
        .map(|value| value.is_some())
        .unwrap_or(existing.rate_limit_max_executions.is_some());
    let has_quotas = request
        .quotas
        .as_ref()
        .map(|value| !value.is_empty())
        .unwrap_or_else(|| {
            existing
                .quotas
                .as_array()
                .map(|value| !value.is_empty())
                .unwrap_or(false)
        });
    validate_policy_features(has_concurrency, has_rate_limit, has_quotas)
}
