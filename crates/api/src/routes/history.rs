//! Entity history API routes
//!
//! Provides read-only access to the TimescaleDB entity history hypertables.
//! History records are written by PostgreSQL triggers — these endpoints only query them.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::sync::Arc;

use attune_common::{
    models::entity_history::HistoryEntityType,
    rbac::{Action as RbacAction, AuthorizationContext, Grant, Resource},
    repositories::{
        entity_history::EntityHistoryRepository, execution::ExecutionRepository,
        identity::IdentityRepository, runtime::WorkerRepository, FindById,
    },
};

use crate::{
    auth::{
        jwt::TokenType,
        middleware::{AuthenticatedUser, RequireAuth},
    },
    authz::AuthorizationService,
    dto::{
        common::{PaginatedResponse, PaginationMeta, PaginationParams},
        history::{HistoryQueryParams, HistoryRecordResponse},
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

/// List history records for a given entity type.
///
/// Supported entity types: `execution`, `worker`.
/// Returns a paginated list of change records ordered by time descending.
#[utoipa::path(
    get,
    path = "/api/v1/history/{entity_type}",
    tag = "history",
    params(
        ("entity_type" = String, Path, description = "Entity type: execution or worker"),
        HistoryQueryParams,
    ),
    responses(
        (status = 200, description = "Paginated list of history records", body = PaginatedResponse<HistoryRecordResponse>),
        (status = 400, description = "Invalid entity type"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_entity_history(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(entity_type_str): Path<String>,
    Query(query): Query<HistoryQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let entity_type = parse_entity_type(&entity_type_str)?;
    authorize_history_list_access(&state, &user, entity_type, query.entity_id).await?;
    if let Some(entity_id) = query.entity_id {
        authorize_history_parent_entity(&state, &user, entity_type, entity_id).await?;
    }

    let repo_params = query.to_repo_params();

    let (records, total) = tokio::try_join!(
        EntityHistoryRepository::query(&state.db, entity_type, &repo_params),
        EntityHistoryRepository::count(&state.db, entity_type, &repo_params),
    )?;

    let data: Vec<HistoryRecordResponse> = records.into_iter().map(Into::into).collect();

    let pagination_params = PaginationParams {
        page: query.page,
        page_size: query.page_size,
    };

    let response = PaginatedResponse {
        items: data,
        pagination: PaginationMeta::new(
            pagination_params.page,
            pagination_params.page_size,
            total as u64,
        ),
    };

    Ok((StatusCode::OK, Json(response)))
}

/// Get history for a specific execution by ID.
///
/// Returns all change records for the given execution, ordered by time descending.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{id}/history",
    tag = "history",
    params(
        ("id" = i64, Path, description = "Execution ID"),
        HistoryQueryParams,
    ),
    responses(
        (status = 200, description = "History records for the execution", body = PaginatedResponse<HistoryRecordResponse>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_execution_history(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
    Query(query): Query<HistoryQueryParams>,
) -> ApiResult<impl IntoResponse> {
    get_entity_history_by_id(&state, &user, HistoryEntityType::Execution, id, query).await
}

/// Get history for a specific worker by ID.
///
/// Returns all change records for the given worker, ordered by time descending.
#[utoipa::path(
    get,
    path = "/api/v1/workers/{id}/history",
    tag = "history",
    params(
        ("id" = i64, Path, description = "Worker ID"),
        HistoryQueryParams,
    ),
    responses(
        (status = 200, description = "History records for the worker", body = PaginatedResponse<HistoryRecordResponse>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_worker_history(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
    Query(query): Query<HistoryQueryParams>,
) -> ApiResult<impl IntoResponse> {
    get_entity_history_by_id(&state, &user, HistoryEntityType::Worker, id, query).await
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Parse and validate the entity type path parameter.
fn parse_entity_type(s: &str) -> Result<HistoryEntityType, ApiError> {
    s.parse::<HistoryEntityType>().map_err(ApiError::BadRequest)
}

fn parent_resource_for_history(entity_type: HistoryEntityType) -> Resource {
    match entity_type {
        HistoryEntityType::Execution => Resource::Executions,
        HistoryEntityType::Worker => Resource::Workers,
    }
}

fn has_unconstrained_read(grants: &[Grant], resource: Resource) -> bool {
    grants.iter().any(|grant| {
        grant.resource == resource
            && grant.actions.contains(&RbacAction::Read)
            && grant.constraints.is_none()
    })
}

async fn authorize_history_list_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    entity_type: HistoryEntityType,
    entity_id: Option<i64>,
) -> ApiResult<()> {
    if entity_id.is_some() {
        return Ok(());
    }

    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Err(ApiError::Forbidden(
            "History list requires access or execution identity".to_string(),
        ));
    }

    let grants = AuthorizationService::new(state.db.clone())
        .effective_grants(user)
        .await?;
    let parent_resource = parent_resource_for_history(entity_type);
    if has_unconstrained_read(&grants, parent_resource) {
        return Ok(());
    }

    Err(ApiError::Forbidden(format!(
        "Scoped history access requires an entity_id filter for {} history",
        entity_type
    )))
}

async fn authorize_history_parent_entity(
    state: &AppState,
    user: &AuthenticatedUser,
    entity_type: HistoryEntityType,
    entity_id: i64,
) -> ApiResult<()> {
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Err(ApiError::Forbidden(
            "History requires access or execution identity".to_string(),
        ));
    }

    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("Identity not found".to_string()))?;
    let identity_attributes: std::collections::HashMap<String, serde_json::Value> =
        match identity.attributes {
            serde_json::Value::Object(map) => map.into_iter().collect(),
            _ => Default::default(),
        };
    let grants = AuthorizationService::new(state.db.clone())
        .effective_grants(user)
        .await?;

    match entity_type {
        HistoryEntityType::Execution => {
            let Some(execution) = ExecutionRepository::find_by_id(&state.db, entity_id).await?
            else {
                return Ok(());
            };
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
                execution_ancestor_identity_ids(&state.db, execution.parent).await?;

            if AuthorizationService::is_allowed(
                &grants,
                Resource::Executions,
                RbacAction::Read,
                &ctx,
            ) {
                Ok(())
            } else {
                Err(ApiError::Forbidden(
                    "Insufficient permissions: executions:read".to_string(),
                ))
            }
        }
        HistoryEntityType::Worker => {
            let Some(worker) = WorkerRepository::find_by_id(&state.db, entity_id).await? else {
                return Ok(());
            };
            let mut ctx = AuthorizationContext::new(identity_id);
            ctx.identity_attributes = identity_attributes.clone();
            ctx.target_id = Some(worker.id);
            ctx.target_ref = Some(worker.name);

            if AuthorizationService::is_allowed(&grants, Resource::Workers, RbacAction::Read, &ctx)
            {
                Ok(())
            } else {
                Err(ApiError::Forbidden(
                    "Insufficient permissions: workers:read".to_string(),
                ))
            }
        }
    }
}

async fn execution_ancestor_identity_ids(
    db: &sqlx::PgPool,
    mut parent_id: Option<i64>,
) -> Result<Vec<i64>, ApiError> {
    let mut identities = Vec::new();
    let mut guard = 0;
    while let Some(id) = parent_id {
        guard += 1;
        if guard > 64 {
            break;
        }
        let Some(parent) = ExecutionRepository::find_by_id(db, id).await? else {
            break;
        };
        if let Some(executor) = parent.executor {
            identities.push(executor);
        }
        parent_id = parent.parent;
    }
    identities.sort_unstable();
    identities.dedup();
    Ok(identities)
}

/// Shared implementation for `GET /<entities>/:id/history` endpoints.
async fn get_entity_history_by_id(
    state: &AppState,
    user: &AuthenticatedUser,
    entity_type: HistoryEntityType,
    entity_id: i64,
    query: HistoryQueryParams,
) -> ApiResult<impl IntoResponse> {
    authorize_history_parent_entity(state, user, entity_type, entity_id).await?;

    // Override entity_id from the path — ignore any entity_id in query params
    let mut repo_params = query.to_repo_params();
    repo_params.entity_id = Some(entity_id);

    let (records, total) = tokio::try_join!(
        EntityHistoryRepository::query(&state.db, entity_type, &repo_params),
        EntityHistoryRepository::count(&state.db, entity_type, &repo_params),
    )?;

    let data: Vec<HistoryRecordResponse> = records.into_iter().map(Into::into).collect();

    let pagination_params = PaginationParams {
        page: query.page,
        page_size: query.page_size,
    };

    let response = PaginatedResponse {
        items: data,
        pagination: PaginationMeta::new(
            pagination_params.page,
            pagination_params.page_size,
            total as u64,
        ),
    };

    Ok((StatusCode::OK, Json(response)))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the history routes.
///
/// Mounts:
/// - `GET /history/:entity_type`          — generic history query
/// - `GET /executions/:id/history`        — execution-specific history
/// - `GET /workers/:id/history`           — worker-specific history (note: currently no /workers base route exists)
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Generic history endpoint
        .route("/history/{entity_type}", get(list_entity_history))
        // Entity-specific convenience endpoints
        .route("/executions/{id}/history", get(get_execution_history))
        .route("/workers/{id}/history", get(get_worker_history))
}
