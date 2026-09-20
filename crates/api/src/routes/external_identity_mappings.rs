use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use validator::Validate;

use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome},
    rbac::{Action, AuthorizationContext, Resource},
    repositories::{
        external_identity_mapping::{
            CreateExternalIdentityMappingInput, ExternalIdentityMappingRepository,
            UpdateExternalIdentityMappingInput,
        },
        identity::IdentityRepository,
        FindById,
    },
};

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::AuthorizationCheck,
    dto::{
        ApiResponse, CreateExternalIdentityMappingRequest, ExternalIdentityMappingResponse,
        PaginatedResponse, PaginationParams, SuccessResponse, UpdateExternalIdentityMappingRequest,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

#[utoipa::path(
    post,
    path = "/api/v1/identities/{integration_identity}/external-identity-mappings",
    tag = "external identity mappings",
    params(("integration_identity" = i64, Path, description = "Integration identity ID")),
    request_body = CreateExternalIdentityMappingRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Mapping created", body = ApiResponse<ExternalIdentityMappingResponse>),
        (status = 422, description = "Invalid mapping"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient identity administration permission"),
        (status = 404, description = "Integration identity not found"),
        (status = 409, description = "Mapping already exists")
    )
)]
pub async fn create_external_identity_mapping(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(integration_identity): Path<i64>,
    Json(request): Json<CreateExternalIdentityMappingRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_mapping_admin(&state, &user, integration_identity, Action::Update).await?;
    authorize_mapping_admin(&state, &user, request.mapped_identity, Action::Update).await?;
    request.validate()?;
    ensure_identity_exists(&state, integration_identity).await?;

    let created_by = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let mapping = ExternalIdentityMappingRepository::create(
        &state.db,
        integration_identity,
        CreateExternalIdentityMappingInput {
            mapped_identity: request.mapped_identity,
            provider: request.provider,
            tenant: request.tenant,
            external_subject: request.external_subject,
            created_by: Some(created_by),
        },
    )
    .await?;

    emit_mapping_audit(
        &state,
        &user,
        event_type::admin::EXTERNAL_IDENTITY_MAPPING_CREATED,
        &mapping,
        serde_json::json!({
            "integration_identity": mapping.integration_identity,
            "mapped_identity": mapping.mapped_identity,
            "provider": mapping.provider,
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::new(ExternalIdentityMappingResponse::from(
            mapping,
        ))),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/identities/{integration_identity}/external-identity-mappings",
    tag = "external identity mappings",
    params(("integration_identity" = i64, Path, description = "Integration identity ID"), PaginationParams),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Mappings", body = PaginatedResponse<ExternalIdentityMappingResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient identity read permission"),
        (status = 404, description = "Integration identity not found")
    )
)]
pub async fn list_external_identity_mappings(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(integration_identity): Path<i64>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_mapping_admin(&state, &user, integration_identity, Action::Read).await?;
    ensure_identity_exists(&state, integration_identity).await?;

    let limit = pagination.limit();
    let mut mappings = ExternalIdentityMappingRepository::list_page(
        &state.db,
        integration_identity,
        limit + 1,
        pagination.offset(),
    )
    .await?;
    let has_next = mappings.len() > limit as usize;
    mappings.truncate(limit as usize);
    let mappings = mappings
        .into_iter()
        .map(ExternalIdentityMappingResponse::from)
        .collect::<Vec<_>>();
    Ok((
        StatusCode::OK,
        Json(PaginatedResponse::without_totals(
            mappings,
            &pagination,
            has_next,
        )),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/identities/{integration_identity}/external-identity-mappings/{mapping_id}",
    tag = "external identity mappings",
    params(
        ("integration_identity" = i64, Path, description = "Integration identity ID"),
        ("mapping_id" = i64, Path, description = "Mapping ID")
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Mapping", body = ApiResponse<ExternalIdentityMappingResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient identity read permission"),
        (status = 404, description = "Identity or mapping not found")
    )
)]
pub async fn get_external_identity_mapping(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((integration_identity, mapping_id)): Path<(i64, i64)>,
) -> ApiResult<impl IntoResponse> {
    authorize_mapping_admin(&state, &user, integration_identity, Action::Read).await?;
    ensure_identity_exists(&state, integration_identity).await?;

    let mapping = find_mapping(&state, integration_identity, mapping_id).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(ExternalIdentityMappingResponse::from(
            mapping,
        ))),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/identities/{integration_identity}/external-identity-mappings/{mapping_id}",
    tag = "external identity mappings",
    params(
        ("integration_identity" = i64, Path, description = "Integration identity ID"),
        ("mapping_id" = i64, Path, description = "Mapping ID")
    ),
    request_body = UpdateExternalIdentityMappingRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Mapping updated", body = ApiResponse<ExternalIdentityMappingResponse>),
        (status = 422, description = "Invalid mapping"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient identity administration permission"),
        (status = 404, description = "Identity or mapping not found"),
        (status = 409, description = "Mapping already exists")
    )
)]
pub async fn update_external_identity_mapping(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((integration_identity, mapping_id)): Path<(i64, i64)>,
    Json(request): Json<UpdateExternalIdentityMappingRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_mapping_admin(&state, &user, integration_identity, Action::Update).await?;
    request.validate()?;
    ensure_identity_exists(&state, integration_identity).await?;
    let mut transaction = state.db.begin().await?;
    let existing = ExternalIdentityMappingRepository::find_by_id_for_update(
        &mut transaction,
        integration_identity,
        mapping_id,
    )
    .await?
    .ok_or_else(|| mapping_not_found(mapping_id))?;
    authorize_mapping_admin(&state, &user, existing.mapped_identity, Action::Update).await?;
    if request.mapped_identity != existing.mapped_identity {
        authorize_mapping_admin(&state, &user, request.mapped_identity, Action::Update).await?;
    }

    let mapping = ExternalIdentityMappingRepository::update(
        &mut *transaction,
        integration_identity,
        mapping_id,
        UpdateExternalIdentityMappingInput {
            mapped_identity: request.mapped_identity,
            provider: request.provider,
            tenant: request.tenant,
            external_subject: request.external_subject,
        },
    )
    .await?;
    transaction.commit().await?;

    emit_mapping_audit(
        &state,
        &user,
        event_type::admin::EXTERNAL_IDENTITY_MAPPING_UPDATED,
        &mapping,
        serde_json::json!({
            "integration_identity": mapping.integration_identity,
            "mapped_identity": mapping.mapped_identity,
            "provider": mapping.provider,
            "changed_fields": {
                "mapped_identity": existing.mapped_identity != mapping.mapped_identity,
                "provider": existing.provider != mapping.provider,
                "tenant": existing.tenant != mapping.tenant,
                "external_subject": existing.external_subject != mapping.external_subject,
            },
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(ExternalIdentityMappingResponse::from(
            mapping,
        ))),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/identities/{integration_identity}/external-identity-mappings/{mapping_id}",
    tag = "external identity mappings",
    params(
        ("integration_identity" = i64, Path, description = "Integration identity ID"),
        ("mapping_id" = i64, Path, description = "Mapping ID")
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Mapping deleted", body = ApiResponse<SuccessResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient identity administration permission"),
        (status = 404, description = "Identity or mapping not found")
    )
)]
pub async fn delete_external_identity_mapping(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((integration_identity, mapping_id)): Path<(i64, i64)>,
) -> ApiResult<impl IntoResponse> {
    authorize_mapping_admin(&state, &user, integration_identity, Action::Update).await?;
    ensure_identity_exists(&state, integration_identity).await?;
    let mut transaction = state.db.begin().await?;
    let mapping = ExternalIdentityMappingRepository::find_by_id_for_update(
        &mut transaction,
        integration_identity,
        mapping_id,
    )
    .await?
    .ok_or_else(|| mapping_not_found(mapping_id))?;
    authorize_mapping_admin(&state, &user, mapping.mapped_identity, Action::Update).await?;

    if !ExternalIdentityMappingRepository::delete(
        &mut *transaction,
        integration_identity,
        mapping_id,
    )
    .await?
    {
        return Err(mapping_not_found(mapping_id));
    }
    transaction.commit().await?;

    emit_mapping_audit(
        &state,
        &user,
        event_type::admin::EXTERNAL_IDENTITY_MAPPING_DELETED,
        &mapping,
        serde_json::json!({
            "integration_identity": mapping.integration_identity,
            "mapped_identity": mapping.mapped_identity,
            "provider": mapping.provider,
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(SuccessResponse::new(
            "External identity mapping deleted successfully",
        ))),
    ))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/identities/{integration_identity}/external-identity-mappings",
            get(list_external_identity_mappings).post(create_external_identity_mapping),
        )
        .route(
            "/identities/{integration_identity}/external-identity-mappings/{mapping_id}",
            get(get_external_identity_mapping)
                .put(update_external_identity_mapping)
                .delete(delete_external_identity_mapping),
        )
}

async fn authorize_mapping_admin(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    integration_identity: i64,
    action: Action,
) -> ApiResult<()> {
    let caller_identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let mut context = AuthorizationContext::new(caller_identity);
    context.target_id = Some(integration_identity);
    state
        .authorization_service()
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Identities,
                action,
                context,
            },
        )
        .await
}

async fn ensure_identity_exists(state: &Arc<AppState>, identity_id: i64) -> ApiResult<()> {
    IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id)))?;
    Ok(())
}

async fn find_mapping(
    state: &Arc<AppState>,
    integration_identity: i64,
    mapping_id: i64,
) -> ApiResult<attune_common::models::identity::ExternalIdentityMapping> {
    ExternalIdentityMappingRepository::find_by_id(&state.db, integration_identity, mapping_id)
        .await?
        .ok_or_else(|| mapping_not_found(mapping_id))
}

fn mapping_not_found(mapping_id: i64) -> ApiError {
    ApiError::NotFound(format!(
        "External identity mapping '{}' not found",
        mapping_id
    ))
}

fn emit_mapping_audit(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    event_type: &'static str,
    mapping: &attune_common::models::identity::ExternalIdentityMapping,
    details: serde_json::Value,
) {
    let mut builder =
        AuditEventBuilder::new(AuditCategory::Admin, event_type, AuditOutcome::Success)
            .resource("external_identity_mapping")
            .resource_id(mapping.id)
            .with_details(details)
            .actor_login(user.login().to_string())
            .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase());

    if let Ok(identity_id) = user.identity_id() {
        builder = builder.actor_identity(identity_id);
    }
    state.audit_emitter.emit(builder.build());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_builds() {
        let _router = routes();
    }
}
