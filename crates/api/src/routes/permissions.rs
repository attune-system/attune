use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use std::sync::Arc;
use validator::Validate;

use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome},
    auth::generate_integration_token,
    models::identity::{Identity, IdentityRoleAssignment},
    mq::{
        IdentityAuthorizationChangedPayload, MessageEnvelope, MessageType,
        PermissionSetChangedPayload,
    },
    rbac::{Action, AuthorizationContext, Resource},
    repositories::{
        identity::{
            CreateIdentityInput, CreateIdentityRoleAssignmentInput,
            CreatePermissionAssignmentInput, CreatePermissionSetRoleAssignmentInput,
            IdentityRepository, IdentityRoleAssignmentRepository, PermissionAssignmentRepository,
            PermissionSetRepository, PermissionSetRoleAssignmentRepository, UpdateIdentityInput,
            UpdatePermissionSetInput,
        },
        integration_token::{CreateIntegrationTokenInput, IntegrationTokenRepository},
        Create, Delete, FindById, FindByRef, List, Update,
    },
};

use crate::{
    auth::hash_password,
    auth::middleware::RequireAuth,
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        ApiResponse, CreateIdentityRequest, CreateIdentityRoleAssignmentRequest,
        CreateIntegrationTokenRequest, CreateIntegrationTokenResponse,
        CreatePermissionAssignmentRequest, CreatePermissionSetRoleAssignmentRequest,
        IdentityResponse, IdentityRoleAssignmentResponse, IdentitySummary,
        IntegrationTokenResponse, PermissionAssignmentResponse, PermissionSetQueryParams,
        PermissionSetRoleAssignmentResponse, PermissionSetSummary, RevokeIntegrationTokenRequest,
        SuccessResponse, UpdateIdentityRequest, UpdatePermissionSetRequest,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

async fn publish_permission_set_metadata_change(
    state: &Arc<AppState>,
    permission_set_id: i64,
    permission_set_ref: &str,
    operation: &str,
) {
    AuthorizationService::invalidate_permission_set_caches().await;

    let Some(publisher) = state.get_publisher().await else {
        return;
    };

    let payload = PermissionSetChangedPayload {
        permission_set_id,
        permission_set_ref: permission_set_ref.to_string(),
        operation: operation.to_string(),
        updated_at: chrono::Utc::now(),
    };
    let envelope =
        MessageEnvelope::new(MessageType::PermissionSetChanged, payload).with_source("api-service");
    if let Err(error) = publisher.publish_envelope(&envelope).await {
        tracing::warn!(
            "Failed to publish PermissionSetChanged metadata invalidation for permission set '{}': {}",
            permission_set_ref,
            error
        );
    }
}

async fn publish_identity_authorization_metadata_change(
    state: &Arc<AppState>,
    identity_id: i64,
    operation: &str,
) {
    AuthorizationService::invalidate_identity_authz_cache(identity_id).await;

    let Some(publisher) = state.get_publisher().await else {
        return;
    };

    let payload = IdentityAuthorizationChangedPayload {
        identity_id,
        operation: operation.to_string(),
        updated_at: chrono::Utc::now(),
    };
    let envelope = MessageEnvelope::new(MessageType::IdentityAuthorizationChanged, payload)
        .with_source("api-service");
    if let Err(error) = publisher.publish_envelope(&envelope).await {
        tracing::warn!(
            "Failed to publish IdentityAuthorizationChanged metadata invalidation for identity {}: {}",
            identity_id,
            error
        );
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/identities",
    tag = "permissions",
    params(PaginationParams),
    responses(
        (status = 200, description = "List identities", body = PaginatedResponse<IdentitySummary>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_identities(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Read).await?;

    let identities = IdentityRepository::list(&state.db).await?;
    let total = identities.len() as u64;
    let start = query.offset() as usize;
    let end = (start + query.limit() as usize).min(identities.len());
    let page_items = if start >= identities.len() {
        Vec::new()
    } else {
        identities[start..end].to_vec()
    };

    let mut summaries = Vec::with_capacity(page_items.len());
    for identity in page_items {
        let role_assignments =
            IdentityRoleAssignmentRepository::find_by_identity(&state.db, identity.id).await?;
        let roles = role_assignments.into_iter().map(|ra| ra.role).collect();
        let mut summary = IdentitySummary::from(identity);
        summary.roles = roles;
        summaries.push(summary);
    }

    Ok((
        StatusCode::OK,
        Json(PaginatedResponse::new(summaries, &query, total)),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/identities/{id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    responses(
        (status = 200, description = "Identity details", body = inline(ApiResponse<IdentityResponse>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_identity(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Read).await?;

    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id)))?;
    let roles = IdentityRoleAssignmentRepository::find_by_identity(&state.db, identity_id).await?;
    let assignments =
        PermissionAssignmentRepository::find_by_identity(&state.db, identity_id).await?;
    let permission_sets = PermissionSetRepository::find_by_identity(&state.db, identity_id).await?;
    let permission_set_refs = permission_sets
        .into_iter()
        .map(|ps| (ps.id, ps.r#ref))
        .collect::<std::collections::HashMap<_, _>>();

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(IdentityResponse {
            id: identity.id,
            login: identity.login,
            display_name: identity.display_name,
            frozen: identity.frozen,
            attributes: identity.attributes,
            roles: roles
                .into_iter()
                .map(IdentityRoleAssignmentResponse::from)
                .collect(),
            direct_permissions: assignments
                .into_iter()
                .filter_map(|assignment| {
                    permission_set_refs.get(&assignment.permset).cloned().map(
                        |permission_set_ref| PermissionAssignmentResponse {
                            id: assignment.id,
                            identity_id: assignment.identity,
                            permission_set_id: assignment.permset,
                            permission_set_ref,
                            created: assignment.created,
                        },
                    )
                })
                .collect(),
        })),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/identities",
    tag = "permissions",
    request_body = CreateIdentityRequest,
    responses(
        (status = 201, description = "Identity created", body = inline(ApiResponse<IdentityResponse>)),
        (status = 409, description = "Identity already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_identity(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreateIdentityRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Create).await?;
    request.validate()?;

    let password_hash = match request.password {
        Some(password) => Some(hash_password(&password)?),
        None => None,
    };

    let identity = IdentityRepository::create(
        &state.db,
        CreateIdentityInput {
            login: request.login,
            display_name: request.display_name,
            password_hash,
            attributes: request.attributes,
        },
    )
    .await?;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::IDENTITY_CREATED,
        "identity",
        Some(identity.id),
        Some(identity.login.as_str()),
        serde_json::json!({
            "login": identity.login.as_str(),
            "display_name": identity.display_name.as_deref(),
            "frozen": identity.frozen,
            "password_set": identity.password_hash.is_some(),
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::new(IdentityResponse::from(identity))),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/identities/{id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    request_body = UpdateIdentityRequest,
    responses(
        (status = 200, description = "Identity updated", body = inline(ApiResponse<IdentityResponse>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_identity(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
    Json(request): Json<UpdateIdentityRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Update).await?;

    let existing = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id)))?;

    let password_hash = match request.password {
        Some(password) => Some(hash_password(&password)?),
        None => None,
    };

    let identity = IdentityRepository::update(
        &state.db,
        identity_id,
        UpdateIdentityInput {
            display_name: request.display_name,
            password_hash,
            attributes: request.attributes,
            frozen: request.frozen,
        },
    )
    .await?;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::IDENTITY_UPDATED,
        "identity",
        Some(identity.id),
        Some(identity.login.as_str()),
        serde_json::json!({
            "login": identity.login.as_str(),
            "changed_fields": {
                "display_name": existing.display_name != identity.display_name,
                "password": existing.password_hash != identity.password_hash,
                "attributes": existing.attributes != identity.attributes,
                "frozen": existing.frozen != identity.frozen,
            },
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(IdentityResponse::from(identity))),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/identities/{id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    responses(
        (status = 200, description = "Identity deleted", body = inline(ApiResponse<SuccessResponse>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_identity(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Delete).await?;

    let caller_identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    if caller_identity_id == identity_id {
        return Err(ApiError::BadRequest(
            "Refusing to delete the currently authenticated identity".to_string(),
        ));
    }

    let deleted = IdentityRepository::delete(&state.db, identity_id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Identity '{}' not found",
            identity_id
        )));
    }

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::IDENTITY_DELETED,
        "identity",
        Some(identity_id),
        None,
        serde_json::json!({ "identity_id": identity_id }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(SuccessResponse::new(
            "Identity deleted successfully",
        ))),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/permissions/sets",
    tag = "permissions",
    params(PermissionSetQueryParams),
    responses(
        (status = 200, description = "List permission sets", body = Vec<PermissionSetSummary>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_permission_sets(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<PermissionSetQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Read).await?;

    let mut permission_sets = PermissionSetRepository::list(&state.db).await?;
    if let Some(pack_ref) = &query.pack_ref {
        permission_sets.retain(|ps| ps.pack_ref.as_deref() == Some(pack_ref.as_str()));
    }

    let mut response = Vec::with_capacity(permission_sets.len());
    for permission_set in permission_sets {
        let permission_set_ref = permission_set.r#ref.clone();
        let roles = PermissionSetRoleAssignmentRepository::find_by_permission_set(
            &state.db,
            permission_set.id,
        )
        .await?;
        response.push(PermissionSetSummary {
            id: permission_set.id,
            r#ref: permission_set.r#ref,
            pack_ref: permission_set.pack_ref,
            label: permission_set.label,
            description: permission_set.description,
            grants: permission_set.grants,
            roles: roles
                .into_iter()
                .map(|assignment| PermissionSetRoleAssignmentResponse {
                    id: assignment.id,
                    permission_set_id: assignment.permset,
                    permission_set_ref: Some(permission_set_ref.clone()),
                    role: assignment.role,
                    created: assignment.created,
                })
                .collect(),
        });
    }

    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    put,
    path = "/api/v1/permissions/sets/{id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Permission set ID")
    ),
    request_body = UpdatePermissionSetRequest,
    responses(
        (status = 200, description = "Permission set updated", body = inline(ApiResponse<PermissionSetSummary>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Permission set not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_permission_set(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(permission_set_id): Path<i64>,
    Json(request): Json<UpdatePermissionSetRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Manage).await?;
    request.validate()?;
    validate_permission_grants(&request.grants)?;

    let existing = PermissionSetRepository::find_by_id(&state.db, permission_set_id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Permission set '{}' not found", permission_set_id))
        })?;

    let updated = PermissionSetRepository::update(
        &state.db,
        existing.id,
        UpdatePermissionSetInput {
            label: request.label,
            description: request.description,
            grants: Some(request.grants),
        },
    )
    .await?;
    let roles =
        PermissionSetRoleAssignmentRepository::find_by_permission_set(&state.db, updated.id)
            .await?;
    publish_permission_set_metadata_change(&state, updated.id, &updated.r#ref, "updated").await;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::PERMISSION_SET_CHANGED,
        "permission_set",
        Some(updated.id),
        Some(updated.r#ref.as_str()),
        serde_json::json!({
            "operation": "updated",
            "permission_set_ref": updated.r#ref.as_str(),
            "updated_fields": ["label", "description", "grants"],
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(PermissionSetSummary {
            id: updated.id,
            r#ref: updated.r#ref.clone(),
            pack_ref: updated.pack_ref,
            label: updated.label,
            description: updated.description,
            grants: updated.grants,
            roles: roles
                .into_iter()
                .map(|assignment| PermissionSetRoleAssignmentResponse {
                    id: assignment.id,
                    permission_set_id: assignment.permset,
                    permission_set_ref: Some(updated.r#ref.clone()),
                    role: assignment.role,
                    created: assignment.created,
                })
                .collect(),
        })),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/identities/{id}/permissions",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    responses(
        (status = 200, description = "List permission assignments for an identity", body = Vec<PermissionAssignmentResponse>),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_identity_permissions(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Read).await?;

    IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id)))?;

    let assignments =
        PermissionAssignmentRepository::find_by_identity(&state.db, identity_id).await?;
    let permission_sets = PermissionSetRepository::find_by_identity(&state.db, identity_id).await?;

    let permission_set_refs = permission_sets
        .into_iter()
        .map(|ps| (ps.id, ps.r#ref))
        .collect::<std::collections::HashMap<_, _>>();

    let response: Vec<PermissionAssignmentResponse> = assignments
        .into_iter()
        .filter_map(|assignment| {
            permission_set_refs
                .get(&assignment.permset)
                .cloned()
                .map(|permission_set_ref| PermissionAssignmentResponse {
                    id: assignment.id,
                    identity_id: assignment.identity,
                    permission_set_id: assignment.permset,
                    permission_set_ref,
                    created: assignment.created,
                })
        })
        .collect();

    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    post,
    path = "/api/v1/permissions/assignments",
    tag = "permissions",
    request_body = CreatePermissionAssignmentRequest,
    responses(
        (status = 201, description = "Permission assignment created", body = inline(ApiResponse<PermissionAssignmentResponse>)),
        (status = 404, description = "Identity or permission set not found"),
        (status = 409, description = "Assignment already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_permission_assignment(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreatePermissionAssignmentRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Manage).await?;

    let identity = resolve_identity(&state, &request).await?;
    let permission_set =
        PermissionSetRepository::find_by_ref(&state.db, &request.permission_set_ref)
            .await?
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "Permission set '{}' not found",
                    request.permission_set_ref
                ))
            })?;

    let assignment = PermissionAssignmentRepository::create(
        &state.db,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await?;
    publish_identity_authorization_metadata_change(&state, identity.id, "permission_assigned")
        .await;

    let response = PermissionAssignmentResponse {
        id: assignment.id,
        identity_id: assignment.identity,
        permission_set_id: assignment.permset,
        permission_set_ref: permission_set.r#ref.clone(),
        created: assignment.created,
    };

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::PERMISSION_ASSIGNMENT_CHANGED,
        "permission_assignment",
        Some(assignment.id),
        Some(permission_set.r#ref.as_str()),
        serde_json::json!({
            "operation": "created",
            "identity_id": identity.id,
            "identity_login": identity.login.as_str(),
            "permission_set_id": permission_set.id,
            "permission_set_ref": permission_set.r#ref.as_str(),
        }),
    );

    Ok((StatusCode::CREATED, Json(ApiResponse::new(response))))
}

#[utoipa::path(
    delete,
    path = "/api/v1/permissions/assignments/{id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Permission assignment ID")
    ),
    responses(
        (status = 200, description = "Permission assignment deleted", body = inline(ApiResponse<SuccessResponse>)),
        (status = 404, description = "Assignment not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_permission_assignment(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(assignment_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Manage).await?;

    let existing = PermissionAssignmentRepository::find_by_id(&state.db, assignment_id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Permission assignment '{}' not found",
                assignment_id
            ))
        })?;

    let deleted = PermissionAssignmentRepository::delete(&state.db, existing.id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Permission assignment '{}' not found",
            assignment_id
        )));
    }
    publish_identity_authorization_metadata_change(
        &state,
        existing.identity,
        "permission_unassigned",
    )
    .await;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::PERMISSION_ASSIGNMENT_CHANGED,
        "permission_assignment",
        Some(existing.id),
        None,
        serde_json::json!({
            "operation": "deleted",
            "identity_id": existing.identity,
            "permission_set_id": existing.permset,
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(SuccessResponse::new(
            "Permission assignment deleted successfully",
        ))),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/identities/{id}/roles",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    request_body = CreateIdentityRoleAssignmentRequest,
    responses(
        (status = 201, description = "Identity role assignment created", body = inline(ApiResponse<IdentityRoleAssignmentResponse>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_identity_role_assignment(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
    Json(request): Json<CreateIdentityRoleAssignmentRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Manage).await?;
    request.validate()?;

    IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id)))?;

    let assignment = IdentityRoleAssignmentRepository::create(
        &state.db,
        CreateIdentityRoleAssignmentInput {
            identity: identity_id,
            role: request.role,
            source: "manual".to_string(),
            managed: false,
        },
    )
    .await?;
    publish_identity_authorization_metadata_change(&state, assignment.identity, "role_assigned")
        .await;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::ROLE_ASSIGNMENT_CHANGED,
        "identity_role_assignment",
        Some(assignment.id),
        Some(assignment.role.as_str()),
        serde_json::json!({
            "operation": "created",
            "identity_id": assignment.identity,
            "role": assignment.role.as_str(),
            "source": assignment.source.as_str(),
            "managed": assignment.managed,
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::new(IdentityRoleAssignmentResponse::from(
            assignment,
        ))),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/identities/roles/{id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity role assignment ID")
    ),
    responses(
        (status = 200, description = "Identity role assignment deleted", body = inline(ApiResponse<SuccessResponse>)),
        (status = 404, description = "Identity role assignment not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_identity_role_assignment(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(assignment_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Manage).await?;

    let assignment = IdentityRoleAssignmentRepository::find_by_id(&state.db, assignment_id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Identity role assignment '{}' not found",
                assignment_id
            ))
        })?;

    if assignment.managed {
        return Err(ApiError::BadRequest(
            "Managed role assignments must be updated through the identity provider sync"
                .to_string(),
        ));
    }

    IdentityRoleAssignmentRepository::delete(&state.db, assignment_id).await?;
    publish_identity_authorization_metadata_change(&state, assignment.identity, "role_unassigned")
        .await;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::ROLE_ASSIGNMENT_CHANGED,
        "identity_role_assignment",
        Some(assignment.id),
        Some(assignment.role.as_str()),
        serde_json::json!({
            "operation": "deleted",
            "identity_id": assignment.identity,
            "role": assignment.role.as_str(),
            "source": assignment.source.as_str(),
            "managed": assignment.managed,
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(SuccessResponse::new(
            "Identity role assignment deleted successfully",
        ))),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/permissions/sets/{id}/roles",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Permission set ID")
    ),
    request_body = CreatePermissionSetRoleAssignmentRequest,
    responses(
        (status = 201, description = "Permission set role assignment created", body = inline(ApiResponse<PermissionSetRoleAssignmentResponse>)),
        (status = 404, description = "Permission set not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_permission_set_role_assignment(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(permission_set_id): Path<i64>,
    Json(request): Json<CreatePermissionSetRoleAssignmentRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Manage).await?;
    request.validate()?;

    let permission_set = PermissionSetRepository::find_by_id(&state.db, permission_set_id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Permission set '{}' not found", permission_set_id))
        })?;

    let assignment = PermissionSetRoleAssignmentRepository::create(
        &state.db,
        CreatePermissionSetRoleAssignmentInput {
            permset: permission_set_id,
            role: request.role,
        },
    )
    .await?;
    publish_permission_set_metadata_change(
        &state,
        permission_set.id,
        &permission_set.r#ref,
        "role_assigned",
    )
    .await;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::PERMISSION_SET_CHANGED,
        "permission_set_role_assignment",
        Some(assignment.id),
        Some(permission_set.r#ref.as_str()),
        serde_json::json!({
            "operation": "created",
            "permission_set_id": assignment.permset,
            "permission_set_ref": permission_set.r#ref.as_str(),
            "role": assignment.role.as_str(),
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::new(PermissionSetRoleAssignmentResponse {
            id: assignment.id,
            permission_set_id: assignment.permset,
            permission_set_ref: Some(permission_set.r#ref),
            role: assignment.role,
            created: assignment.created,
        })),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/permissions/sets/roles/{id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Permission set role assignment ID")
    ),
    responses(
        (status = 200, description = "Permission set role assignment deleted", body = inline(ApiResponse<SuccessResponse>)),
        (status = 404, description = "Permission set role assignment not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_permission_set_role_assignment(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(assignment_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Permissions, Action::Manage).await?;

    let assignment = PermissionSetRoleAssignmentRepository::find_by_id(&state.db, assignment_id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Permission set role assignment '{}' not found",
                assignment_id
            ))
        })?;

    PermissionSetRoleAssignmentRepository::delete(&state.db, assignment_id).await?;
    if let Some(permission_set) =
        PermissionSetRepository::find_by_id(&state.db, assignment.permset).await?
    {
        publish_permission_set_metadata_change(
            &state,
            permission_set.id,
            &permission_set.r#ref,
            "role_unassigned",
        )
        .await;
    } else {
        AuthorizationService::invalidate_permission_set_caches().await;
    }

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::PERMISSION_SET_CHANGED,
        "permission_set_role_assignment",
        Some(assignment.id),
        Some(assignment.role.as_str()),
        serde_json::json!({
            "operation": "deleted",
            "permission_set_id": assignment.permset,
            "role": assignment.role.as_str(),
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(SuccessResponse::new(
            "Permission set role assignment deleted successfully",
        ))),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/identities/{id}/freeze",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    responses(
        (status = 200, description = "Identity frozen", body = inline(ApiResponse<SuccessResponse>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn freeze_identity(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    set_identity_frozen(&state, &user, identity_id, true).await
}

#[utoipa::path(
    post,
    path = "/api/v1/identities/{id}/unfreeze",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    responses(
        (status = 200, description = "Identity unfrozen", body = inline(ApiResponse<SuccessResponse>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn unfreeze_identity(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    set_identity_frozen(&state, &user, identity_id, false).await
}

#[utoipa::path(
    get,
    path = "/api/v1/identities/{id}/integration-tokens",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    responses(
        (status = 200, description = "List integration tokens", body = inline(ApiResponse<Vec<IntegrationTokenResponse>>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_integration_tokens(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Read).await?;
    ensure_identity_exists(&state, identity_id).await?;

    let tokens = IntegrationTokenRepository::list_by_identity(&state.db, identity_id).await?;
    let response = tokens
        .into_iter()
        .map(IntegrationTokenResponse::from)
        .collect::<Vec<_>>();

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

#[utoipa::path(
    post,
    path = "/api/v1/identities/{id}/integration-tokens",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID")
    ),
    request_body = CreateIntegrationTokenRequest,
    responses(
        (status = 201, description = "Integration token created", body = inline(ApiResponse<CreateIntegrationTokenResponse>)),
        (status = 404, description = "Identity not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_integration_token(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(identity_id): Path<i64>,
    Json(request): Json<CreateIntegrationTokenRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Update).await?;
    request.validate()?;
    ensure_identity_exists(&state, identity_id).await?;

    if request
        .expires_at
        .map(|expires_at| expires_at <= chrono::Utc::now())
        .unwrap_or(false)
    {
        return Err(ApiError::BadRequest(
            "Integration token expiration must be in the future".to_string(),
        ));
    }

    let generated = generate_integration_token()?;
    let label = request.label.trim().to_string();
    let created_by = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;

    let token = IntegrationTokenRepository::create(
        &state.db,
        CreateIntegrationTokenInput {
            identity: identity_id,
            label,
            description: request.description,
            token_hash: generated.hash,
            token_prefix: generated.prefix,
            token_suffix: generated.suffix,
            created_by: Some(created_by),
            expires_at: request.expires_at,
        },
    )
    .await?;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::IDENTITY_UPDATED,
        "integration_token",
        Some(token.id),
        Some(token.label.as_str()),
        serde_json::json!({
            "identity_id": identity_id,
            "action": "created",
            "expires_at": token.expires_at,
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::new(CreateIntegrationTokenResponse {
            token: generated.secret,
            integration_token: IntegrationTokenResponse::from(token),
        })),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/identities/{id}/integration-tokens/{token_id}/revoke",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID"),
        ("token_id" = i64, Path, description = "Integration token ID")
    ),
    request_body = RevokeIntegrationTokenRequest,
    responses(
        (status = 200, description = "Integration token revoked", body = inline(ApiResponse<IntegrationTokenResponse>)),
        (status = 404, description = "Integration token not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn revoke_integration_token(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((identity_id, token_id)): Path<(i64, i64)>,
    Json(request): Json<RevokeIntegrationTokenRequest>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Update).await?;
    request.validate()?;
    ensure_token_belongs_to_identity(&state, identity_id, token_id).await?;

    let revoked_by = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let token = IntegrationTokenRepository::revoke(
        &state.db,
        token_id,
        Some(revoked_by),
        request.reason.as_deref(),
    )
    .await?;

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::IDENTITY_UPDATED,
        "integration_token",
        Some(token.id),
        Some(token.label.as_str()),
        serde_json::json!({
            "identity_id": identity_id,
            "action": "revoked",
            "reason": token.revocation_reason,
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(IntegrationTokenResponse::from(token))),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/identities/{id}/integration-tokens/{token_id}",
    tag = "permissions",
    params(
        ("id" = i64, Path, description = "Identity ID"),
        ("token_id" = i64, Path, description = "Integration token ID")
    ),
    responses(
        (status = 200, description = "Integration token deleted", body = inline(ApiResponse<SuccessResponse>)),
        (status = 404, description = "Integration token not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_integration_token(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((identity_id, token_id)): Path<(i64, i64)>,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(&state, &user, Resource::Identities, Action::Update).await?;
    let token = ensure_token_belongs_to_identity(&state, identity_id, token_id).await?;

    let deleted = IntegrationTokenRepository::delete(&state.db, token_id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Integration token '{}' not found",
            token_id
        )));
    }

    emit_admin_audit(
        &state,
        &user,
        event_type::admin::IDENTITY_UPDATED,
        "integration_token",
        Some(token_id),
        Some(token.label.as_str()),
        serde_json::json!({
            "identity_id": identity_id,
            "action": "deleted",
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(SuccessResponse::new(
            "Integration token deleted successfully",
        ))),
    ))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/identities", get(list_identities).post(create_identity))
        .route(
            "/identities/{id}",
            get(get_identity)
                .put(update_identity)
                .delete(delete_identity),
        )
        .route(
            "/identities/{id}/roles",
            post(create_identity_role_assignment),
        )
        .route(
            "/identities/{id}/permissions",
            get(list_identity_permissions),
        )
        .route("/identities/{id}/freeze", post(freeze_identity))
        .route("/identities/{id}/unfreeze", post(unfreeze_identity))
        .route(
            "/identities/{id}/integration-tokens",
            get(list_integration_tokens).post(create_integration_token),
        )
        .route(
            "/identities/{id}/integration-tokens/{token_id}",
            delete(delete_integration_token),
        )
        .route(
            "/identities/{id}/integration-tokens/{token_id}/revoke",
            post(revoke_integration_token),
        )
        .route(
            "/identities/roles/{id}",
            delete(delete_identity_role_assignment),
        )
        .route("/permissions/sets", get(list_permission_sets))
        .route("/permissions/sets/{id}", put(update_permission_set))
        .route(
            "/permissions/sets/{id}/roles",
            post(create_permission_set_role_assignment),
        )
        .route(
            "/permissions/sets/roles/{id}",
            delete(delete_permission_set_role_assignment),
        )
        .route(
            "/permissions/assignments",
            post(create_permission_assignment),
        )
        .route(
            "/permissions/assignments/{id}",
            delete(delete_permission_assignment),
        )
}

async fn authorize_permissions(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    resource: Resource,
    action: Action,
) -> ApiResult<()> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = AuthorizationService::new(state.db.clone());
    authz
        .authorize(
            user,
            AuthorizationCheck {
                resource,
                action,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await
}

async fn ensure_identity_exists(state: &Arc<AppState>, identity_id: i64) -> ApiResult<Identity> {
    IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id)))
}

async fn ensure_token_belongs_to_identity(
    state: &Arc<AppState>,
    identity_id: i64,
    token_id: i64,
) -> ApiResult<attune_common::models::IntegrationToken> {
    ensure_identity_exists(state, identity_id).await?;
    let token = IntegrationTokenRepository::find_by_id(&state.db, token_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Integration token '{}' not found", token_id)))?;

    if token.identity != identity_id {
        return Err(ApiError::NotFound(format!(
            "Integration token '{}' not found",
            token_id
        )));
    }

    Ok(token)
}

fn validate_permission_grants(grants: &serde_json::Value) -> ApiResult<()> {
    if !grants.is_array() {
        return Err(ApiError::BadRequest(
            "Permission set grants must be an array".to_string(),
        ));
    }

    let parsed = serde_json::from_value::<Vec<attune_common::rbac::Grant>>(grants.clone())
        .map_err(|e| ApiError::BadRequest(format!("Invalid permission grant schema: {}", e)))?;

    for grant in parsed {
        validate_grant_actions(&grant)?;
        if let Some(constraints) = &grant.constraints {
            if constraints.ids.is_some() {
                return Err(ApiError::BadRequest(
                    "Permission set grants cannot be scoped by database IDs; use metadata refs instead"
                        .to_string(),
                ));
            }
            if constraints.owner_refs.is_some() {
                return Err(ApiError::BadRequest(
                    "Permission set grants cannot use owner_refs; use a pack scope, component ref scope, or owner type/self constraints".to_string(),
                ));
            }
            if constraints.pack_refs.is_some() && constraints.refs.is_some() {
                return Err(ApiError::BadRequest(
                    "Permission set grants can be pack scoped or component scoped, not both"
                        .to_string(),
                ));
            }
            validate_grant_constraints(&grant.resource, constraints)?;
        }
    }

    Ok(())
}

fn validate_grant_actions(grant: &attune_common::rbac::Grant) -> ApiResult<()> {
    let allowed = match grant.resource {
        Resource::Packs => &[
            Action::Read,
            Action::Create,
            Action::Install,
            Action::Configure,
            Action::Delete,
        ][..],
        Resource::Actions => &[
            Action::Read,
            Action::Create,
            Action::Update,
            Action::Delete,
            Action::Execute,
        ][..],
        Resource::Policies => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::Queues => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::QueueItems => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::Rules => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::Triggers => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::Executions => &[
            Action::Read,
            Action::Update,
            Action::Cancel,
            Action::Decrypt,
        ][..],
        Resource::Events => &[Action::Read, Action::Decrypt][..],
        Resource::Enforcements => &[Action::Read, Action::Decrypt][..],
        Resource::Inquiries => &[
            Action::Read,
            Action::Create,
            Action::Update,
            Action::Delete,
            Action::Respond,
        ][..],
        Resource::Keys => &[
            Action::Read,
            Action::Create,
            Action::Update,
            Action::Delete,
            Action::Decrypt,
        ][..],
        Resource::Artifacts => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::Runtimes => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::Workers => &[Action::Read, Action::Manage][..],
        Resource::Retention => &[Action::Read, Action::Update][..],
        Resource::Identities => &[Action::Read, Action::Create, Action::Update, Action::Delete][..],
        Resource::Permissions => &[Action::Read, Action::Manage][..],
        Resource::AuditLog => &[Action::Read][..],
    };

    if grant.actions.is_empty() || grant.actions.iter().any(|action| !allowed.contains(action)) {
        return Err(ApiError::BadRequest(format!(
            "Permission grant for {:?} includes unsupported actions",
            grant.resource
        )));
    }

    Ok(())
}

fn validate_grant_constraints(
    resource: &Resource,
    constraints: &attune_common::rbac::GrantConstraints,
) -> ApiResult<()> {
    if constraints.pack_refs.is_some()
        && !matches!(
            resource,
            Resource::Packs
                | Resource::Actions
                | Resource::Queues
                | Resource::Rules
                | Resource::Triggers
                | Resource::Executions
                | Resource::Enforcements
                | Resource::Artifacts
        )
    {
        return Err(ApiError::BadRequest(format!(
            "{:?} grants do not support pack scope constraints",
            resource
        )));
    }

    if constraints.refs.is_some()
        && !matches!(
            resource,
            Resource::Packs
                | Resource::Actions
                | Resource::Queues
                | Resource::Rules
                | Resource::Triggers
                | Resource::Executions
                | Resource::Enforcements
                | Resource::Keys
                | Resource::Artifacts
        )
    {
        return Err(ApiError::BadRequest(format!(
            "{:?} grants do not support component ref constraints",
            resource
        )));
    }

    if constraints.owner.is_some()
        && !matches!(
            resource,
            Resource::Packs | Resource::Keys | Resource::Artifacts
        )
    {
        return Err(ApiError::BadRequest(format!(
            "{:?} grants do not support owner constraints",
            resource
        )));
    }

    if constraints.owner_types.is_some()
        && !matches!(resource, Resource::Keys | Resource::Artifacts)
    {
        return Err(ApiError::BadRequest(format!(
            "{:?} grants do not support owner type constraints",
            resource
        )));
    }

    if constraints.visibility.is_some() && !matches!(resource, Resource::Artifacts) {
        return Err(ApiError::BadRequest(
            "Visibility constraints only apply to artifacts grants".to_string(),
        ));
    }

    if constraints.execution_scope.is_some() && !matches!(resource, Resource::Executions) {
        return Err(ApiError::BadRequest(
            "Execution scope constraints only apply to executions grants".to_string(),
        ));
    }

    if constraints.encrypted.is_some() && !matches!(resource, Resource::Keys) {
        return Err(ApiError::BadRequest(
            "Encryption constraints only apply to keys grants".to_string(),
        ));
    }

    Ok(())
}

async fn resolve_identity(
    state: &Arc<AppState>,
    request: &CreatePermissionAssignmentRequest,
) -> ApiResult<Identity> {
    match (request.identity_id, request.identity_login.as_deref()) {
        (Some(identity_id), None) => IdentityRepository::find_by_id(&state.db, identity_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id))),
        (None, Some(identity_login)) => {
            IdentityRepository::find_by_login(&state.db, identity_login)
                .await?
                .ok_or_else(|| {
                    ApiError::NotFound(format!("Identity '{}' not found", identity_login))
                })
        }
        (Some(_), Some(_)) => Err(ApiError::BadRequest(
            "Provide either identity_id or identity_login, not both".to_string(),
        )),
        (None, None) => Err(ApiError::BadRequest(
            "Either identity_id or identity_login is required".to_string(),
        )),
    }
}

fn emit_admin_audit(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    event_type: &'static str,
    resource_type: &'static str,
    resource_id: Option<i64>,
    resource_ref: Option<&str>,
    details: serde_json::Value,
) {
    let mut builder =
        AuditEventBuilder::new(AuditCategory::Admin, event_type, AuditOutcome::Success)
            .resource(resource_type)
            .with_details(details);

    if let Some(resource_id) = resource_id {
        builder = builder.resource_id(resource_id);
    }
    if let Some(resource_ref) = resource_ref {
        builder = builder.resource_ref(resource_ref.to_string());
    }
    if let Ok(identity_id) = user.identity_id() {
        builder = builder.actor_identity(identity_id);
    }
    builder = builder
        .actor_login(user.login().to_string())
        .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase());

    state.audit_emitter.emit(builder.build());
}

impl From<Identity> for IdentitySummary {
    fn from(value: Identity) -> Self {
        Self {
            id: value.id,
            login: value.login,
            display_name: value.display_name,
            frozen: value.frozen,
            attributes: value.attributes,
            roles: Vec::new(),
        }
    }
}

impl From<IdentityRoleAssignment> for IdentityRoleAssignmentResponse {
    fn from(value: IdentityRoleAssignment) -> Self {
        Self {
            id: value.id,
            identity_id: value.identity,
            role: value.role,
            source: value.source,
            managed: value.managed,
            created: value.created,
            updated: value.updated,
        }
    }
}

impl From<Identity> for IdentityResponse {
    fn from(value: Identity) -> Self {
        Self {
            id: value.id,
            login: value.login,
            display_name: value.display_name,
            frozen: value.frozen,
            attributes: value.attributes,
            roles: Vec::new(),
            direct_permissions: Vec::new(),
        }
    }
}

async fn set_identity_frozen(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    identity_id: i64,
    frozen: bool,
) -> ApiResult<impl IntoResponse> {
    authorize_permissions(state, user, Resource::Identities, Action::Update).await?;

    let caller_identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    if caller_identity_id == identity_id && frozen {
        return Err(ApiError::BadRequest(
            "Refusing to freeze the currently authenticated identity".to_string(),
        ));
    }

    IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Identity '{}' not found", identity_id)))?;

    IdentityRepository::update(
        &state.db,
        identity_id,
        UpdateIdentityInput {
            display_name: None,
            password_hash: None,
            attributes: None,
            frozen: Some(frozen),
        },
    )
    .await?;

    emit_admin_audit(
        state,
        user,
        event_type::admin::IDENTITY_UPDATED,
        "identity",
        Some(identity_id),
        None,
        serde_json::json!({
            "changed_fields": { "frozen": true },
            "frozen": frozen,
        }),
    );

    let message = if frozen {
        "Identity frozen successfully"
    } else {
        "Identity unfrozen successfully"
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(SuccessResponse::new(message))),
    ))
}
