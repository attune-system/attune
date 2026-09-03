//! Runtime management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use validator::Validate;

use attune_common::models::Runtime as RuntimeModel;
use attune_common::mq::{MessageEnvelope, MessageType, RuntimeChangedPayload};
use attune_common::rbac::{Action, AuthorizationContext, Resource};
use attune_common::repositories::{
    pack::PackRepository,
    runtime::{CreateRuntimeInput, RuntimeRepository, RuntimeSearchFilters, UpdateRuntimeInput},
    sensor_admission::{
        SensorAdmissionFailure, SensorAdmissionRepository, SensorAdmissionRequirement,
    },
    Create, Delete, FindByRef, Patch, Update,
};

use crate::{
    auth::middleware::RequireAuth,
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        runtime::{
            CreateRuntimeRequest, NullableJsonPatch, NullableStringPatch, RuntimeListParams,
            RuntimeResponse, RuntimeSummary, UpdateRuntimeRequest,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

fn reject_sensor_admission(failures: Vec<SensorAdmissionFailure>) -> ApiResult<()> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(ApiError::UnprocessableEntity(
        serde_json::to_string(&failures)
            .unwrap_or_else(|_| "Managed sensor runtime is unavailable".to_string()),
    ))
}

async fn authorize_runtime(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    action: Action,
) -> ApiResult<()> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    state
        .authorization_service()
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Runtimes,
                action,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await
}

async fn can_view_runtime_execution_config(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    runtime: &RuntimeModel,
) -> ApiResult<bool> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let grants = state.authorization_service().effective_grants(user).await?;

    let mut context = AuthorizationContext::new(identity_id);
    context.target_id = Some(runtime.id);
    context.target_ref = Some(runtime.r#ref.clone());
    context.pack_ref = runtime.pack_ref.clone();

    Ok(AuthorizationService::is_allowed(
        &grants,
        Resource::Runtimes,
        Action::Update,
        &context,
    ))
}

fn redact_runtime_execution_config(mut runtime: RuntimeModel) -> RuntimeModel {
    runtime.execution_config = JsonValue::Object(Default::default());
    runtime
}

async fn publish_runtime_metadata_change(
    state: &Arc<AppState>,
    runtime: &RuntimeModel,
    operation: &str,
    updated_at: chrono::DateTime<chrono::Utc>,
) {
    let Some(publisher) = state.get_publisher().await else {
        return;
    };

    let payload = RuntimeChangedPayload {
        runtime_id: runtime.id,
        runtime_ref: runtime.r#ref.clone(),
        pack_ref: runtime.pack_ref.clone(),
        operation: operation.to_string(),
        updated_at,
    };
    let envelope =
        MessageEnvelope::new(MessageType::RuntimeChanged, payload).with_source("api-service");
    if let Err(error) = publisher.publish_envelope(&envelope).await {
        tracing::warn!(
            "Failed to publish RuntimeChanged metadata invalidation for runtime '{}': {}",
            runtime.r#ref,
            error
        );
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/runtimes",
    tag = "runtimes",
    params(RuntimeListParams),
    responses(
        (status = 200, description = "List of runtimes", body = PaginatedResponse<RuntimeSummary>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_runtimes(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<RuntimeListParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_runtime(&state, &user, Action::Read).await?;

    let pagination = query.pagination();
    let result = RuntimeRepository::list_search(
        &state.db,
        &RuntimeSearchFilters {
            query: query.q,
            limit: pagination.limit(),
            offset: pagination.offset(),
            ..Default::default()
        },
    )
    .await?;

    let response = PaginatedResponse::new(
        result.rows.into_iter().map(RuntimeSummary::from).collect(),
        &pagination,
        result.total,
    );

    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/packs/{pack_ref}/runtimes",
    tag = "runtimes",
    params(
        ("pack_ref" = String, Path, description = "Pack reference identifier"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of runtimes for a pack", body = PaginatedResponse<RuntimeSummary>),
        (status = 404, description = "Pack not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_runtimes_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_runtime(&state, &user, Action::Read).await?;

    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let all_runtimes = RuntimeRepository::find_by_pack(&state.db, pack.id).await?;
    let total = all_runtimes.len() as u64;
    let rows: Vec<_> = all_runtimes
        .into_iter()
        .skip(pagination.offset() as usize)
        .take(pagination.limit() as usize)
        .collect();

    let response = PaginatedResponse::new(
        rows.into_iter().map(RuntimeSummary::from).collect(),
        &pagination,
        total,
    );

    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/runtimes/{ref}",
    tag = "runtimes",
    params(("ref" = String, Path, description = "Runtime reference identifier")),
    responses(
        (status = 200, description = "Runtime details", body = ApiResponse<RuntimeResponse>),
        (status = 404, description = "Runtime not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_runtime(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(runtime_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    authorize_runtime(&state, &user, Action::Read).await?;

    let runtime = RuntimeRepository::find_by_ref(&state.db, &runtime_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Runtime '{}' not found", runtime_ref)))?;
    let can_view_sensitive = can_view_runtime_execution_config(&state, &user, &runtime).await?;
    let runtime = if can_view_sensitive {
        runtime
    } else {
        redact_runtime_execution_config(runtime)
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(RuntimeResponse::from(runtime))),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/runtimes",
    tag = "runtimes",
    request_body = CreateRuntimeRequest,
    responses(
        (status = 201, description = "Runtime created successfully", body = ApiResponse<RuntimeResponse>),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Pack not found"),
        (status = 409, description = "Runtime with same ref already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_runtime(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreateRuntimeRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_runtime(&state, &user, Action::Create).await?;

    request.validate()?;
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;

    if RuntimeRepository::find_by_ref(&mut *tx, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Runtime with ref '{}' already exists",
            request.r#ref
        )));
    }

    let (pack_id, pack_ref) = if let Some(ref pack_ref_str) = request.pack_ref {
        let pack = PackRepository::find_by_ref(&mut *tx, pack_ref_str)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref_str)))?;
        (Some(pack.id), Some(pack.r#ref))
    } else {
        (None, None)
    };

    let runtime = RuntimeRepository::create(
        &mut *tx,
        CreateRuntimeInput {
            r#ref: request.r#ref,
            pack: pack_id,
            pack_ref,
            description: request.description,
            name: request.name,
            aliases: vec![],
            distributions: request.distributions,
            installation: request.installation,
            execution_config: request.execution_config,
            auto_detected: false,
            detection_config: serde_json::json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    publish_runtime_metadata_change(&state, &runtime, "created", runtime.updated).await;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            RuntimeResponse::from(runtime),
            "Runtime created successfully",
        )),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/runtimes/{ref}",
    tag = "runtimes",
    params(("ref" = String, Path, description = "Runtime reference identifier")),
    request_body = UpdateRuntimeRequest,
    responses(
        (status = 200, description = "Runtime updated successfully", body = ApiResponse<RuntimeResponse>),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Runtime not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_runtime(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(runtime_ref): Path<String>,
    Json(request): Json<UpdateRuntimeRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_runtime(&state, &user, Action::Update).await?;

    request.validate()?;
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;

    let existing_runtime = RuntimeRepository::find_by_ref(&mut *tx, &runtime_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Runtime '{}' not found", runtime_ref)))?;

    let runtime = RuntimeRepository::update(
        &mut *tx,
        existing_runtime.id,
        UpdateRuntimeInput {
            description: request.description.map(|patch| match patch {
                NullableStringPatch::Set(value) => Patch::Set(value),
                NullableStringPatch::Clear => Patch::Clear,
            }),
            name: request.name,
            distributions: request.distributions,
            installation: request.installation.map(|patch| match patch {
                NullableJsonPatch::Set(value) => Patch::Set(value),
                NullableJsonPatch::Clear => Patch::Clear,
            }),
            execution_config: request.execution_config,
            ..Default::default()
        },
    )
    .await?;
    reject_sensor_admission(
        SensorAdmissionRepository::assess_runtime(
            &mut tx,
            runtime.id,
            SensorAdmissionRequirement::Live,
        )
        .await?,
    )?;
    tx.commit().await?;
    publish_runtime_metadata_change(&state, &runtime, "updated", runtime.updated).await;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            RuntimeResponse::from(runtime),
            "Runtime updated successfully",
        )),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/runtimes/{ref}",
    tag = "runtimes",
    params(("ref" = String, Path, description = "Runtime reference identifier")),
    responses(
        (status = 200, description = "Runtime deleted successfully", body = SuccessResponse),
        (status = 404, description = "Runtime not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_runtime(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(runtime_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    authorize_runtime(&state, &user, Action::Delete).await?;
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;

    let runtime = RuntimeRepository::find_by_ref(&mut *tx, &runtime_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Runtime '{}' not found", runtime_ref)))?;

    let deleted = RuntimeRepository::delete(&mut *tx, runtime.id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Runtime '{}' not found",
            runtime_ref
        )));
    }
    tx.commit().await?;
    publish_runtime_metadata_change(&state, &runtime, "deleted", chrono::Utc::now()).await;

    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new(format!(
            "Runtime '{}' deleted successfully",
            runtime_ref
        ))),
    ))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/runtimes", get(list_runtimes).post(create_runtime))
        .route(
            "/runtimes/{ref}",
            get(get_runtime).put(update_runtime).delete(delete_runtime),
        )
        .route("/packs/{pack_ref}/runtimes", get(list_runtimes_by_pack))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_routes_structure() {
        let _router = routes();
    }
}
