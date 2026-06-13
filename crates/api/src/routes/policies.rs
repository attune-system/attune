//! Policy management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::sync::Arc;
use validator::Validate;

use attune_common::metadata_cache::repositories::CachedMetadataRepository;
use attune_common::rbac::{Action, AuthorizationContext, Resource};
use attune_common::repositories::{
    action::{ActionRepository, CreatePolicyInput, PolicyRepository, UpdatePolicyInput},
    pack::PackRepository,
    Create, Delete, FindByRef, List, Update,
};

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        policy::{
            CreatePolicyRequest, PolicyListParams, PolicyResponse, PolicyScopeKind, PolicySummary,
            UpdatePolicyRequest,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

async fn authorize_policy(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: Action,
    context: AuthorizationContext,
) -> ApiResult<()> {
    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Policies,
                action,
                context,
            },
        )
        .await
}

fn policy_auth_context(
    identity_id: i64,
    policy_ref: Option<String>,
    policy_id: Option<i64>,
    pack_ref: Option<String>,
) -> AuthorizationContext {
    let mut context = AuthorizationContext::new(identity_id);
    context.target_ref = policy_ref;
    context.target_id = policy_id;
    context.pack_ref = pack_ref;
    context
}

fn matches_scope(policy: &attune_common::models::Policy, scope: Option<PolicyScopeKind>) -> bool {
    match scope {
        Some(PolicyScopeKind::Action) => policy.action.is_some(),
        Some(PolicyScopeKind::Pack) => policy.action.is_none() && policy.pack.is_some(),
        Some(PolicyScopeKind::Global) => policy.action.is_none() && policy.pack.is_none(),
        None => true,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/policies",
    tag = "policies",
    params(PolicyListParams),
    responses(
        (status = 200, description = "List of execution policies", body = PaginatedResponse<PolicySummary>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_policies(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<PolicyListParams>,
) -> ApiResult<impl IntoResponse> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = AuthorizationService::new(state.db.clone());
    let grants = authz.effective_grants(&user).await?;

    let mut rows = PolicyRepository::list(&state.db).await?;
    rows.retain(|policy| {
        let context = policy_auth_context(
            identity_id,
            Some(policy.r#ref.clone()),
            Some(policy.id),
            policy.pack_ref.clone(),
        );

        AuthorizationService::is_allowed(&grants, Resource::Policies, Action::Read, &context)
            && matches_scope(policy, query.scope)
            && query
                .pack_ref
                .as_ref()
                .is_none_or(|pack_ref| policy.pack_ref.as_ref() == Some(pack_ref))
            && query
                .action_ref
                .as_ref()
                .is_none_or(|action_ref| policy.action_ref.as_ref() == Some(action_ref))
    });

    rows.sort_by(|a, b| {
        b.created
            .cmp(&a.created)
            .then_with(|| a.r#ref.cmp(&b.r#ref))
    });
    let total = rows.len() as u64;
    let pagination = PaginationParams {
        page: query.page,
        page_size: query.limit(),
    };
    let page_rows = rows
        .into_iter()
        .skip(query.offset())
        .take(query.limit() as usize)
        .map(PolicySummary::from)
        .collect();

    Ok((
        StatusCode::OK,
        Json(PaginatedResponse::new(page_rows, &pagination, total)),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/policies/{ref}",
    tag = "policies",
    params(("ref" = String, Path, description = "Policy reference identifier")),
    responses(
        (status = 200, description = "Policy details", body = ApiResponse<PolicyResponse>),
        (status = 404, description = "Policy not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(policy_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let policy = CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .find_policy_by_ref(&policy_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Policy '{}' not found", policy_ref)))?;
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    authorize_policy(
        &state,
        &user,
        Action::Read,
        policy_auth_context(
            identity_id,
            Some(policy.r#ref.clone()),
            Some(policy.id),
            policy.pack_ref.clone(),
        ),
    )
    .await?;
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .put_policy_best_effort(&policy)
        .await;

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
    responses(
        (status = 201, description = "Policy created successfully", body = ApiResponse<PolicyResponse>),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Referenced pack or action not found"),
        (status = 409, description = "Policy with same ref already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreatePolicyRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    if request.pack_ref.is_some() && request.action_ref.is_some() {
        return Err(ApiError::BadRequest(
            "Policy scope must target either an action, a pack, or global scope".to_string(),
        ));
    }

    let (pack, pack_ref, action, action_ref) = if let Some(action_ref) = &request.action_ref {
        let action = ActionRepository::find_by_ref(&state.db, action_ref)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", action_ref)))?;
        (
            Some(action.pack),
            Some(action.pack_ref.clone()),
            Some(action.id),
            Some(action.r#ref),
        )
    } else if let Some(pack_ref) = &request.pack_ref {
        let pack = PackRepository::find_by_ref(&state.db, pack_ref)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;
        (Some(pack.id), Some(pack.r#ref), None, None)
    } else {
        (None, None, None, None)
    };
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    authorize_policy(
        &state,
        &user,
        Action::Create,
        policy_auth_context(
            identity_id,
            Some(request.r#ref.clone()),
            None,
            pack_ref.clone(),
        ),
    )
    .await?;

    let policy = PolicyRepository::create(
        &state.db,
        CreatePolicyInput {
            r#ref: request.r#ref,
            pack,
            pack_ref,
            action,
            action_ref,
            parameters: request.parameters,
            method: request.method,
            threshold: request.threshold,
            name: request.name,
            description: request.description,
            tags: request.tags,
        },
    )
    .await?;
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .put_policy_best_effort(&policy)
        .await;

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
    params(("ref" = String, Path, description = "Policy reference identifier")),
    request_body = UpdatePolicyRequest,
    responses(
        (status = 200, description = "Policy updated successfully", body = ApiResponse<PolicyResponse>),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Policy not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(policy_ref): Path<String>,
    Json(request): Json<UpdatePolicyRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let existing = PolicyRepository::find_by_ref(&state.db, &policy_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Policy '{}' not found", policy_ref)))?;
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    authorize_policy(
        &state,
        &user,
        Action::Update,
        policy_auth_context(
            identity_id,
            Some(existing.r#ref.clone()),
            Some(existing.id),
            existing.pack_ref.clone(),
        ),
    )
    .await?;

    let policy = PolicyRepository::update(
        &state.db,
        existing.id,
        UpdatePolicyInput {
            parameters: request.parameters,
            method: request.method,
            threshold: request.threshold,
            name: request.name,
            description: request.description,
            tags: request.tags,
        },
    )
    .await?;

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
    params(("ref" = String, Path, description = "Policy reference identifier")),
    responses(
        (status = 200, description = "Policy deleted successfully", body = SuccessResponse),
        (status = 404, description = "Policy not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_policy(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(policy_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let existing = PolicyRepository::find_by_ref(&state.db, &policy_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Policy '{}' not found", policy_ref)))?;
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    authorize_policy(
        &state,
        &user,
        Action::Delete,
        policy_auth_context(
            identity_id,
            Some(existing.r#ref.clone()),
            Some(existing.id),
            existing.pack_ref.clone(),
        ),
    )
    .await?;

    PolicyRepository::delete(&state.db, existing.id).await?;
    CachedMetadataRepository::new(&state.db, &state.metadata_cache)
        .evict_policy_best_effort(&existing)
        .await;

    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new(format!(
            "Policy '{}' deleted successfully",
            policy_ref
        ))),
    ))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/policies", get(list_policies).post(create_policy))
        .route(
            "/policies/{ref}",
            get(get_policy).put(update_policy).delete(delete_policy),
        )
}
