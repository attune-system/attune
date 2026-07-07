//! Key/Secret management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::sync::Arc;
use validator::Validate;

use attune_common::repositories::{
    action::ActionRepository,
    key::{
        CreateKeyInput, KeyGrantFilter, KeyRepository, KeySearchFilters, KeyVisibility,
        UpdateKeyInput,
    },
    pack::PackRepository,
    trigger::SensorRepository,
    Create, Delete, FindByRef, Update,
};
use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome, PendingAuditEvent},
    models::{key::Key, OwnerType},
    rbac::{Action, AuthorizationContext, ExecutionScopeConstraint, Grant, Resource},
};

use crate::auth::{jwt::TokenType, RequireAuth};
use crate::{
    authz::AuthorizationService,
    dto::{
        common::{PaginatedResponse, PaginationParams},
        key::{CreateKeyRequest, KeyQueryParams, KeyResponse, KeySummary, UpdateKeyRequest},
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

/// List all keys with pagination and optional filters (values redacted)
#[utoipa::path(
    get,
    path = "/api/v1/keys",
    tag = "secrets",
    params(KeyQueryParams),
    responses(
        (status = 200, description = "List of keys (values redacted)", body = PaginatedResponse<KeySummary>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_keys(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<KeyQueryParams>,
) -> ApiResult<impl IntoResponse> {
    // Row-level RBAC visibility is only enforced for access/execution tokens,
    // matching the previous in-memory behavior: sensor/worker tokens see all
    // keys matching the owner filters (they have no effective-grants
    // identity to scope against).
    let visibility = if matches!(
        user.0.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let identity_id = user
            .0
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        let grants = authz.effective_grants(&user.0).await?;

        // Ensure the principal can read at least some key records.
        let can_read_any_key = grants
            .iter()
            .any(|g| g.resource == Resource::Keys && g.actions.contains(&Action::Read));
        if !can_read_any_key {
            return Err(ApiError::Forbidden(
                "Insufficient permissions: keys:read".to_string(),
            ));
        }

        Some(KeyVisibility {
            identity_id,
            grants: compile_key_read_grant_filters(&grants),
        })
    } else {
        None
    };

    // Owner filters, RBAC visibility, and pagination are all pushed into a
    // single filtered SQL query (see `KeyRepository::search`), so totals and
    // pages are always consistent with what's actually visible.
    let filters = KeySearchFilters {
        owner_type: query.owner_type,
        owner: query.owner.clone(),
        limit: query.limit(),
        offset: query.offset(),
        visibility,
    };

    let result = KeyRepository::search(&state.db, &filters).await?;

    let paginated_keys: Vec<KeySummary> = result.rows.into_iter().map(KeySummary::from).collect();

    let pagination_params = PaginationParams {
        page: query.page,
        page_size: query.per_page,
    };

    let response = PaginatedResponse::new(paginated_keys, &pagination_params, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// Compiles a caller's effective RBAC grants into the SQL-translatable
/// [`KeyGrantFilter`] list consumed by `KeyRepository::search`.
///
/// Only grants for `Resource::Keys` + `Action::Read` are considered. Grants
/// whose constraints can never be satisfied for keys are dropped entirely
/// (see [`compile_key_grant_filter`]), which is safe because such a grant
/// would never allow any row via `Grant::constraints_match` either.
fn compile_key_read_grant_filters(grants: &[Grant]) -> Vec<KeyGrantFilter> {
    grants
        .iter()
        .filter(|grant| grant.resource == Resource::Keys && grant.actions.contains(&Action::Read))
        .filter_map(compile_key_grant_filter)
        .collect()
}

/// Translates a single grant's constraints into a [`KeyGrantFilter`],
/// mirroring `Grant::constraints_match` for the fields the key
/// `AuthorizationContext` actually populates (owner/owner_type/owner_ref/
/// ref/id/encrypted).
///
/// Returns `None` when the grant can never match any key row: the key
/// authorization context never sets `pack_ref`, `visibility`, an execution
/// scope owner, or identity attributes, so a grant constrained on those
/// fields (other than `execution_scope: any`) always fails
/// `constraints_match` regardless of the row.
fn compile_key_grant_filter(grant: &Grant) -> Option<KeyGrantFilter> {
    let Some(constraints) = &grant.constraints else {
        // Unconstrained grant: matches everything except (per
        // `constrained_key_grant_allows`) other identities' identity-owned
        // keys, which `owner_scoped: false` handles in SQL.
        return Some(KeyGrantFilter {
            owner_scoped: false,
            ..Default::default()
        });
    };

    let always_excluded = constraints.pack_refs.is_some()
        || constraints.visibility.is_some()
        || matches!(
            constraints.execution_scope,
            Some(ExecutionScopeConstraint::SelfOnly) | Some(ExecutionScopeConstraint::Descendants)
        )
        || constraints
            .attributes
            .as_ref()
            .is_some_and(|attrs| !attrs.is_empty());

    if always_excluded {
        return None;
    }

    let owner_scoped = constraints.owner.is_some()
        || constraints.owner_types.is_some()
        || constraints.owner_refs.is_some()
        || constraints.refs.is_some()
        || constraints.ids.is_some();

    Some(KeyGrantFilter {
        owner_types: constraints.owner_types.clone(),
        owner: constraints.owner,
        owner_refs: constraints.owner_refs.clone(),
        refs: constraints.refs.clone(),
        ids: constraints.ids.clone(),
        encrypted: constraints.encrypted,
        owner_scoped,
    })
}

/// Get a single key by reference (includes decrypted value)
#[utoipa::path(
    get,
    path = "/api/v1/keys/{ref}",
    tag = "secrets",
    params(
        ("ref" = String, Path, description = "Key reference identifier")
    ),
    responses(
        (status = 200, description = "Key details with decrypted value", body = inline(ApiResponse<KeyResponse>)),
        (status = 404, description = "Key not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(key_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let mut key = KeyRepository::find_by_ref(&state.db, &key_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Key '{}' not found", key_ref)))?;

    // For encrypted keys, track whether this caller is permitted to see the value.
    let can_decrypt = if matches!(
        user.0.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let identity_id = user
            .0
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        let grants = authz.effective_grants(&user.0).await?;

        if !key_action_allowed(&grants, Action::Read, identity_id, &key) {
            return Err(ApiError::NotFound(format!("Key '{}' not found", key_ref)));
        }

        // For encrypted keys, separately check keys:decrypt.
        // Failing this is not an error — we just return the value as null.
        if key.encrypted {
            key_action_allowed(&grants, Action::Decrypt, identity_id, &key)
        } else {
            true
        }
    } else {
        true
    };

    // Decrypt value if encrypted and caller has permission.
    // If they lack Keys::Decrypt, return null rather than the ciphertext.
    if key.encrypted {
        if can_decrypt {
            let encryption_key =
                state
                    .config
                    .security
                    .encryption_key
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::InternalServerError(
                            "Encryption key not configured on server".to_string(),
                        )
                    })?;

            let decrypted_value = attune_common::crypto::decrypt_json(&key.value, encryption_key)
                .map_err(|e| {
                tracing::error!("Failed to decrypt key '{}': {}", key_ref, e);
                ApiError::InternalServerError(format!("Failed to decrypt key: {}", e))
            })?;

            key.value = decrypted_value;
        } else {
            key.value = serde_json::Value::Null;
        }
    }

    emit_key_audit(
        &state,
        &user,
        if key.encrypted && can_decrypt {
            event_type::secret::KEY_DECRYPTED
        } else {
            event_type::secret::KEY_READ
        },
        AuditOutcome::Success,
        &key,
        serde_json::json!({
            "encrypted": key.encrypted,
            "decrypted": key.encrypted && can_decrypt,
            "owner_type": key.owner_type,
            "owner_ref": key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            ),
        }),
    );

    let response = ApiResponse::new(KeyResponse::from(key));

    Ok((StatusCode::OK, Json(response)))
}

/// Create a new key/secret
#[utoipa::path(
    post,
    path = "/api/v1/keys",
    tag = "secrets",
    request_body = CreateKeyRequest,
    responses(
        (status = 201, description = "Key created successfully", body = inline(ApiResponse<KeyResponse>)),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Key with same ref already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateKeyRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    if matches!(
        user.0.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let identity_id = user
            .0
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.owner_identity_id = request.owner_identity;
        ctx.owner_type = Some(request.owner_type);
        ctx.owner_ref = requested_key_owner_ref(&request);
        ctx.encrypted = Some(request.encrypted);
        ctx.target_ref = Some(request.r#ref.clone());

        let grants = authz.effective_grants(&user.0).await?;
        let create_allowed = if request.owner_type == OwnerType::Identity
            && request.owner_identity != Some(identity_id)
        {
            constrained_key_grant_allows(&grants, Action::Create, &ctx)
        } else {
            AuthorizationService::is_allowed(&grants, Resource::Keys, Action::Create, &ctx)
        };
        if !create_allowed {
            return Err(ApiError::Forbidden(
                "Insufficient permissions: keys:create".to_string(),
            ));
        }
    }

    // Check if key with same ref already exists
    if KeyRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Key with ref '{}' already exists",
            request.r#ref
        )));
    }

    // Auto-resolve owner IDs from refs when only the ref is provided.
    // This makes the API more ergonomic for sensors and other clients that
    // know the owner ref but not the numeric database ID.
    let mut owner_sensor = request.owner_sensor;
    let mut owner_action = request.owner_action;
    let mut owner_pack = request.owner_pack;

    match request.owner_type {
        OwnerType::Sensor if owner_sensor.is_none() => {
            if let Some(ref sensor_ref) = request.owner_sensor_ref {
                if let Some(sensor) = SensorRepository::find_by_ref(&state.db, sensor_ref).await? {
                    tracing::debug!(
                        "Auto-resolved owner_sensor from ref '{}' to id {}",
                        sensor_ref,
                        sensor.id
                    );
                    owner_sensor = Some(sensor.id);
                } else {
                    return Err(ApiError::BadRequest(format!(
                        "Sensor with ref '{}' not found",
                        sensor_ref
                    )));
                }
            }
        }
        OwnerType::Action if owner_action.is_none() => {
            if let Some(ref action_ref) = request.owner_action_ref {
                if let Some(action) = ActionRepository::find_by_ref(&state.db, action_ref).await? {
                    tracing::debug!(
                        "Auto-resolved owner_action from ref '{}' to id {}",
                        action_ref,
                        action.id
                    );
                    owner_action = Some(action.id);
                } else {
                    return Err(ApiError::BadRequest(format!(
                        "Action with ref '{}' not found",
                        action_ref
                    )));
                }
            }
        }
        OwnerType::Pack if owner_pack.is_none() => {
            if let Some(ref pack_ref) = request.owner_pack_ref {
                if let Some(pack) = PackRepository::find_by_ref(&state.db, pack_ref).await? {
                    tracing::debug!(
                        "Auto-resolved owner_pack from ref '{}' to id {}",
                        pack_ref,
                        pack.id
                    );
                    owner_pack = Some(pack.id);
                } else {
                    return Err(ApiError::BadRequest(format!(
                        "Pack with ref '{}' not found",
                        pack_ref
                    )));
                }
            }
        }
        _ => {}
    }

    // Encrypt value if requested
    let (value, encryption_key_hash) = if request.encrypted {
        let encryption_key = state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "Cannot encrypt: encryption key not configured on server".to_string(),
                )
            })?;

        let encrypted_value = attune_common::crypto::encrypt_json(&request.value, encryption_key)
            .map_err(|e| {
            tracing::error!("Failed to encrypt key value: {}", e);
            ApiError::InternalServerError(format!("Failed to encrypt value: {}", e))
        })?;

        let key_hash = attune_common::crypto::hash_encryption_key(encryption_key);

        (encrypted_value, Some(key_hash))
    } else {
        // Store in plaintext (not recommended for sensitive data)
        (request.value.clone(), None)
    };

    // Create key input
    let key_input = CreateKeyInput {
        r#ref: request.r#ref,
        owner_type: request.owner_type,
        owner: request.owner,
        owner_identity: request.owner_identity,
        owner_pack,
        owner_pack_ref: request.owner_pack_ref,
        owner_action,
        owner_action_ref: request.owner_action_ref,
        owner_sensor,
        owner_sensor_ref: request.owner_sensor_ref,
        name: request.name,
        encrypted: request.encrypted,
        encryption_key_hash,
        value,
    };

    let mut key = KeyRepository::create(&state.db, key_input).await?;

    // Return decrypted value in response
    if key.encrypted {
        let encryption_key = state.config.security.encryption_key.as_ref().unwrap();
        key.value =
            attune_common::crypto::decrypt_json(&key.value, encryption_key).map_err(|e| {
                tracing::error!("Failed to decrypt newly created key: {}", e);
                ApiError::InternalServerError(format!("Failed to decrypt value: {}", e))
            })?;
    }

    emit_key_audit(
        &state,
        &user,
        event_type::secret::KEY_CREATED,
        AuditOutcome::Success,
        &key,
        serde_json::json!({
            "encrypted": key.encrypted,
            "owner_type": key.owner_type,
            "owner_ref": key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            ),
            "value": "***",
        }),
    );

    let response = ApiResponse::with_message(KeyResponse::from(key), "Key created successfully");

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing key/secret
#[utoipa::path(
    put,
    path = "/api/v1/keys/{ref}",
    tag = "secrets",
    params(
        ("ref" = String, Path, description = "Key reference identifier")
    ),
    request_body = UpdateKeyRequest,
    responses(
        (status = 200, description = "Key updated successfully", body = inline(ApiResponse<KeyResponse>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Key not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(key_ref): Path<String>,
    Json(request): Json<UpdateKeyRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Verify key exists
    let existing = KeyRepository::find_by_ref(&state.db, &key_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Key '{}' not found", key_ref)))?;

    if matches!(
        user.0.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let identity_id = user
            .0
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        let grants = authz.effective_grants(&user.0).await?;
        if !key_action_allowed(&grants, Action::Update, identity_id, &existing) {
            return Err(ApiError::Forbidden(
                "Insufficient permissions: keys:update".to_string(),
            ));
        }
    }

    // Handle value update with encryption
    let (value, encrypted, encryption_key_hash) = if let Some(new_value) = request.value {
        let should_encrypt = request.encrypted.unwrap_or(existing.encrypted);

        if should_encrypt {
            let encryption_key =
                state
                    .config
                    .security
                    .encryption_key
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::BadRequest(
                            "Cannot encrypt: encryption key not configured on server".to_string(),
                        )
                    })?;

            let encrypted_value = attune_common::crypto::encrypt_json(&new_value, encryption_key)
                .map_err(|e| {
                tracing::error!("Failed to encrypt key value: {}", e);
                ApiError::InternalServerError(format!("Failed to encrypt value: {}", e))
            })?;

            let key_hash = attune_common::crypto::hash_encryption_key(encryption_key);

            (Some(encrypted_value), Some(should_encrypt), Some(key_hash))
        } else {
            (Some(new_value), Some(false), None)
        }
    } else {
        // No value update, but might be changing encryption status
        (None, request.encrypted, None)
    };

    // Create update input
    let update_input = UpdateKeyInput {
        name: request.name,
        value,
        encrypted,
        encryption_key_hash,
    };

    let mut updated_key = KeyRepository::update(&state.db, existing.id, update_input).await?;

    // Return decrypted value in response
    if updated_key.encrypted {
        let encryption_key = state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or_else(|| {
                ApiError::InternalServerError("Encryption key not configured on server".to_string())
            })?;

        updated_key.value = attune_common::crypto::decrypt_json(&updated_key.value, encryption_key)
            .map_err(|e| {
                tracing::error!("Failed to decrypt updated key '{}': {}", key_ref, e);
                ApiError::InternalServerError(format!("Failed to decrypt value: {}", e))
            })?;
    }

    emit_key_audit(
        &state,
        &user,
        event_type::secret::KEY_UPDATED,
        AuditOutcome::Success,
        &updated_key,
        serde_json::json!({
            "encrypted": updated_key.encrypted,
            "owner_type": updated_key.owner_type,
            "owner_ref": key_owner_ref(
                updated_key.owner_type,
                updated_key.owner.as_deref(),
                updated_key.owner_pack_ref.as_deref(),
                updated_key.owner_action_ref.as_deref(),
                updated_key.owner_sensor_ref.as_deref(),
            ),
            "value_updated": updated_key.value != existing.value,
            "value": "***",
        }),
    );

    let response =
        ApiResponse::with_message(KeyResponse::from(updated_key), "Key updated successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Delete a key/secret
#[utoipa::path(
    delete,
    path = "/api/v1/keys/{ref}",
    tag = "secrets",
    params(
        ("ref" = String, Path, description = "Key reference identifier")
    ),
    responses(
        (status = 200, description = "Key deleted successfully", body = SuccessResponse),
        (status = 404, description = "Key not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(key_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Verify key exists
    let key = KeyRepository::find_by_ref(&state.db, &key_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Key '{}' not found", key_ref)))?;

    if matches!(
        user.0.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let identity_id = user
            .0
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        let grants = authz.effective_grants(&user.0).await?;
        if !key_action_allowed(&grants, Action::Delete, identity_id, &key) {
            return Err(ApiError::Forbidden(
                "Insufficient permissions: keys:delete".to_string(),
            ));
        }
    }

    // Delete the key
    let deleted = KeyRepository::delete(&state.db, key.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Key '{}' not found", key_ref)));
    }

    let response = SuccessResponse::new("Key deleted successfully");

    emit_key_audit(
        &state,
        &user,
        event_type::secret::KEY_DELETED,
        AuditOutcome::Success,
        &key,
        serde_json::json!({
            "encrypted": key.encrypted,
            "owner_type": key.owner_type,
            "owner_ref": key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            ),
        }),
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Register key/secret routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/keys", get(list_keys).post(create_key))
        .route(
            "/keys/{ref}",
            get(get_key).put(update_key).delete(delete_key),
        )
}

fn key_authorization_context(identity_id: i64, key: &Key) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(key.id);
    ctx.target_ref = Some(key.r#ref.clone());
    ctx.owner_identity_id = key.owner_identity;
    ctx.owner_type = Some(key.owner_type);
    ctx.owner_ref = key_owner_ref(
        key.owner_type,
        key.owner.as_deref(),
        key.owner_pack_ref.as_deref(),
        key.owner_action_ref.as_deref(),
        key.owner_sensor_ref.as_deref(),
    );
    ctx.encrypted = Some(key.encrypted);
    ctx
}

fn key_action_allowed(grants: &[Grant], action: Action, identity_id: i64, key: &Key) -> bool {
    let ctx = key_authorization_context(identity_id, key);
    if key.owner_type == OwnerType::Identity && key.owner_identity != Some(identity_id) {
        return constrained_key_grant_allows(grants, action, &ctx);
    }

    AuthorizationService::is_allowed(grants, Resource::Keys, action, &ctx)
}

fn constrained_key_grant_allows(
    grants: &[Grant],
    action: Action,
    ctx: &AuthorizationContext,
) -> bool {
    grants.iter().any(|grant| {
        let Some(constraints) = &grant.constraints else {
            return false;
        };
        let owner_scoped = constraints.owner.is_some()
            || constraints.owner_types.is_some()
            || constraints.owner_refs.is_some()
            || constraints.refs.is_some()
            || constraints.ids.is_some();
        grant.resource == Resource::Keys
            && grant.actions.contains(&action)
            && owner_scoped
            && grant.allows(Resource::Keys, action, ctx)
    })
}

fn requested_key_owner_ref(request: &CreateKeyRequest) -> Option<String> {
    key_owner_ref(
        request.owner_type,
        request.owner.as_deref(),
        request.owner_pack_ref.as_deref(),
        request.owner_action_ref.as_deref(),
        request.owner_sensor_ref.as_deref(),
    )
}

fn key_owner_ref(
    owner_type: OwnerType,
    owner: Option<&str>,
    owner_pack_ref: Option<&str>,
    owner_action_ref: Option<&str>,
    owner_sensor_ref: Option<&str>,
) -> Option<String> {
    match owner_type {
        OwnerType::Pack => owner_pack_ref.map(str::to_string),
        OwnerType::Action => owner_action_ref.map(str::to_string),
        OwnerType::Sensor => owner_sensor_ref.map(str::to_string),
        _ => owner.map(str::to_string),
    }
}

fn emit_key_audit(
    state: &Arc<AppState>,
    user: &RequireAuth,
    event_type: &'static str,
    outcome: AuditOutcome,
    key: &Key,
    details: serde_json::Value,
) {
    state.audit_emitter.emit(build_key_audit_event(
        user, event_type, outcome, key, details,
    ));
}

fn build_key_audit_event(
    user: &RequireAuth,
    event_type: &'static str,
    outcome: AuditOutcome,
    key: &Key,
    details: serde_json::Value,
) -> PendingAuditEvent {
    let mut builder = AuditEventBuilder::new(AuditCategory::Secret, event_type, outcome)
        .resource("key")
        .resource_id(key.id)
        .resource_ref(key.r#ref.clone())
        .with_details(details);

    if let Ok(identity_id) = user.0.identity_id() {
        builder = builder.actor_identity(identity_id);
    }
    builder = builder
        .actor_login(user.0.login().to_string())
        .actor_token_type(format!("{:?}", user.0.claims.token_type).to_lowercase());

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        jwt::{Claims, TokenType},
        middleware::AuthenticatedUser,
    };
    use chrono::Utc;

    fn test_user() -> RequireAuth {
        RequireAuth(AuthenticatedUser {
            claims: Claims {
                sub: "42".to_string(),
                login: "secret-reader@example.test".to_string(),
                iat: 1,
                exp: 999_999,
                token_type: TokenType::Access,
                scope: None,
                metadata: None,
            },
        })
    }

    fn test_key() -> Key {
        let now = Utc::now();
        Key {
            id: 123,
            r#ref: "finance.api_token".to_string(),
            owner_type: OwnerType::Identity,
            owner: Some("finance".to_string()),
            owner_identity: Some(42),
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Finance API token".to_string(),
            encrypted: true,
            encryption_key_hash: Some("sha256:redacted".to_string()),
            value: serde_json::json!("super-secret-token"),
            created: now,
            updated: now,
        }
    }

    #[test]
    fn key_decrypt_audit_event_redacts_secret_value() {
        let event = build_key_audit_event(
            &test_user(),
            event_type::secret::KEY_DECRYPTED,
            AuditOutcome::Success,
            &test_key(),
            serde_json::json!({
                "encrypted": true,
                "decrypted": true,
                "owner_type": OwnerType::Identity,
                "owner_ref": "finance",
                "value": "***",
            }),
        );

        assert_eq!(event.category, AuditCategory::Secret);
        assert_eq!(event.event_type, event_type::secret::KEY_DECRYPTED);
        assert_eq!(event.outcome, AuditOutcome::Success);
        assert_eq!(event.actor_identity, Some(42));
        assert_eq!(event.resource_type.as_deref(), Some("key"));
        assert_eq!(event.resource_id, Some(123));
        assert_eq!(event.resource_ref.as_deref(), Some("finance.api_token"));

        let serialized = serde_json::to_string(&event.details.expect("details")).unwrap();
        assert!(serialized.contains("\"value\":\"***\""));
        assert!(!serialized.contains("super-secret-token"));
        assert!(!serialized.contains("sha256:redacted"));
    }

    // --- KeyGrantFilter compilation / SQL-predicate parity tests ---
    //
    // These tests validate that `compile_key_read_grant_filters` produces
    // filters whose semantics (simulated here in Rust, mirroring the SQL
    // built by `push_grant_clause`) match `key_action_allowed`'s in-memory
    // decision exactly, across representative grant/key combinations.

    use attune_common::rbac::{ExecutionScopeConstraint, GrantConstraints, OwnerConstraint};

    fn key_fixture(
        id: i64,
        r#ref: &str,
        owner_type: OwnerType,
        owner: Option<&str>,
        owner_identity: Option<i64>,
        owner_pack_ref: Option<&str>,
        encrypted: bool,
    ) -> Key {
        let now = Utc::now();
        Key {
            id,
            r#ref: r#ref.to_string(),
            owner_type,
            owner: owner.map(str::to_string),
            owner_identity,
            owner_pack: owner_pack_ref.map(|_| 1),
            owner_pack_ref: owner_pack_ref.map(str::to_string),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "test key".to_string(),
            encrypted,
            encryption_key_hash: None,
            value: serde_json::json!(null),
            created: now,
            updated: now,
        }
    }

    fn read_grant(constraints: Option<GrantConstraints>) -> Grant {
        Grant {
            resource: Resource::Keys,
            actions: vec![Action::Read],
            constraints,
        }
    }

    /// Simulates the SQL predicate that `push_grant_clause` builds for a
    /// single compiled filter, operating purely in Rust for test purposes.
    fn grant_filter_matches(filter: &KeyGrantFilter, identity_id: i64, key: &Key) -> bool {
        if let Some(owner_types) = &filter.owner_types {
            if !owner_types.contains(&key.owner_type) {
                return false;
            }
        }
        if let Some(owner) = filter.owner {
            let owner_match = match owner {
                OwnerConstraint::SelfOnly => key.owner_identity == Some(identity_id),
                OwnerConstraint::Any => true,
                OwnerConstraint::None => key.owner_identity.is_none(),
            };
            if !owner_match {
                return false;
            }
        }
        if let Some(owner_refs) = &filter.owner_refs {
            let owner_ref = key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            );
            match owner_ref {
                Some(owner_ref) if owner_refs.contains(&owner_ref) => {}
                _ => return false,
            }
        }
        if let Some(refs) = &filter.refs {
            if !refs.contains(&key.r#ref) {
                return false;
            }
        }
        if let Some(ids) = &filter.ids {
            if !ids.contains(&key.id) {
                return false;
            }
        }
        if let Some(encrypted) = filter.encrypted {
            if encrypted != key.encrypted {
                return false;
            }
        }
        if !filter.owner_scoped
            && key.owner_type == OwnerType::Identity
            && key.owner_identity != Some(identity_id)
        {
            return false;
        }
        true
    }

    fn filters_allow(filters: &[KeyGrantFilter], identity_id: i64, key: &Key) -> bool {
        filters
            .iter()
            .any(|f| grant_filter_matches(f, identity_id, key))
    }

    fn assert_parity(grants: &[Grant], identity_id: i64, key: &Key, case: &str) {
        let compiled = compile_key_read_grant_filters(grants);
        let expected = key_action_allowed(grants, Action::Read, identity_id, key);
        let actual = filters_allow(&compiled, identity_id, key);
        assert_eq!(
            actual, expected,
            "case `{case}` mismatch: compiled filters gave {actual}, key_action_allowed gave {expected}"
        );
    }

    #[test]
    fn unconstrained_grant_matches_everything_except_other_identity_owned_keys() {
        let identity_id = 42;
        let grants = vec![read_grant(None)];

        let own_identity_key = key_fixture(
            1,
            "me.token",
            OwnerType::Identity,
            None,
            Some(42),
            None,
            true,
        );
        let other_identity_key = key_fixture(
            2,
            "other.token",
            OwnerType::Identity,
            None,
            Some(99),
            None,
            true,
        );
        let pack_key = key_fixture(
            3,
            "pack.token",
            OwnerType::Pack,
            None,
            None,
            Some("some_pack"),
            false,
        );

        assert_parity(
            &grants,
            identity_id,
            &own_identity_key,
            "unconstrained/own-identity",
        );
        assert_parity(
            &grants,
            identity_id,
            &other_identity_key,
            "unconstrained/other-identity",
        );
        assert_parity(&grants, identity_id, &pack_key, "unconstrained/pack");
    }

    #[test]
    fn owner_types_constraint_matches_key_action_allowed() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            owner_types: Some(vec![OwnerType::System, OwnerType::Pack]),
            ..Default::default()
        }))];

        let system_key = key_fixture(1, "sys.token", OwnerType::System, None, None, None, false);
        let pack_key = key_fixture(
            2,
            "pack.token",
            OwnerType::Pack,
            None,
            None,
            Some("p"),
            false,
        );
        let identity_key = key_fixture(
            3,
            "id.token",
            OwnerType::Identity,
            None,
            Some(99),
            None,
            false,
        );

        assert_parity(&grants, identity_id, &system_key, "owner_types/system");
        assert_parity(&grants, identity_id, &pack_key, "owner_types/pack");
        assert_parity(
            &grants,
            identity_id,
            &identity_key,
            "owner_types/identity-excluded",
        );
    }

    #[test]
    fn owner_refs_constraint_uses_owner_type_dependent_column() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            owner_types: Some(vec![OwnerType::Pack]),
            owner_refs: Some(vec!["allowed_pack".to_string()]),
            ..Default::default()
        }))];

        let allowed = key_fixture(
            1,
            "allowed.token",
            OwnerType::Pack,
            None,
            None,
            Some("allowed_pack"),
            false,
        );
        let other = key_fixture(
            2,
            "other.token",
            OwnerType::Pack,
            None,
            None,
            Some("other_pack"),
            false,
        );

        assert_parity(&grants, identity_id, &allowed, "owner_refs/match");
        assert_parity(&grants, identity_id, &other, "owner_refs/no-match");
    }

    #[test]
    fn owner_self_only_constraint_scopes_to_requesting_identity() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            owner: Some(OwnerConstraint::SelfOnly),
            ..Default::default()
        }))];

        let own_key = key_fixture(
            1,
            "me.token",
            OwnerType::Identity,
            None,
            Some(42),
            None,
            false,
        );
        let other_key = key_fixture(
            2,
            "other.token",
            OwnerType::Identity,
            None,
            Some(99),
            None,
            false,
        );

        assert_parity(&grants, identity_id, &own_key, "owner_self_only/own");
        assert_parity(&grants, identity_id, &other_key, "owner_self_only/other");
    }

    #[test]
    fn ids_and_refs_constraints_are_exact_match() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            ids: Some(vec![5]),
            ..Default::default()
        }))];
        let matching = key_fixture(5, "a.token", OwnerType::System, None, None, None, false);
        let non_matching = key_fixture(6, "b.token", OwnerType::System, None, None, None, false);
        assert_parity(&grants, identity_id, &matching, "ids/match");
        assert_parity(&grants, identity_id, &non_matching, "ids/no-match");

        let grants = vec![read_grant(Some(GrantConstraints {
            refs: Some(vec!["a.token".to_string()]),
            ..Default::default()
        }))];
        assert_parity(&grants, identity_id, &matching, "refs/match");
        assert_parity(&grants, identity_id, &non_matching, "refs/no-match");
    }

    #[test]
    fn encrypted_constraint_matches_key_action_allowed() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            encrypted: Some(false),
            ..Default::default()
        }))];
        let plain = key_fixture(1, "a.token", OwnerType::System, None, None, None, false);
        let secret = key_fixture(2, "b.token", OwnerType::System, None, None, None, true);
        assert_parity(
            &grants,
            identity_id,
            &plain,
            "encrypted/false-matches-plain",
        );
        assert_parity(
            &grants,
            identity_id,
            &secret,
            "encrypted/false-excludes-secret",
        );
    }

    #[test]
    fn grants_relying_on_unpopulated_context_fields_are_excluded_or_no_op() {
        // pack_refs is never satisfiable for keys (ctx.pack_ref is never
        // set), so the grant must be compiled away entirely.
        let pack_refs_grant = read_grant(Some(GrantConstraints {
            pack_refs: Some(vec!["some_pack".to_string()]),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&pack_refs_grant).is_none());

        // visibility is never satisfiable either.
        let visibility_grant = read_grant(Some(GrantConstraints {
            visibility: Some(vec![attune_common::models::ArtifactVisibility::Public]),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&visibility_grant).is_none());

        // execution_scope self/descendants can never match (keys have no
        // execution owner in their AuthorizationContext).
        let self_only_grant = read_grant(Some(GrantConstraints {
            execution_scope: Some(ExecutionScopeConstraint::SelfOnly),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&self_only_grant).is_none());

        // execution_scope::Any is a no-op and doesn't exclude the grant.
        let any_scope_grant = read_grant(Some(GrantConstraints {
            execution_scope: Some(ExecutionScopeConstraint::Any),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&any_scope_grant).is_some());

        // Non-empty attributes can never match (identity_attributes is
        // always empty for keys), but an empty attributes map is a no-op.
        let attrs_grant = read_grant(Some(GrantConstraints {
            attributes: Some(std::collections::HashMap::from([(
                "team".to_string(),
                serde_json::json!("platform"),
            )])),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&attrs_grant).is_none());

        let empty_attrs_grant = read_grant(Some(GrantConstraints {
            attributes: Some(std::collections::HashMap::new()),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&empty_attrs_grant).is_some());
    }

    #[test]
    fn non_matching_resource_or_action_grants_are_dropped() {
        let wrong_resource = Grant {
            resource: Resource::Artifacts,
            actions: vec![Action::Read],
            constraints: None,
        };
        let wrong_action = Grant {
            resource: Resource::Keys,
            actions: vec![Action::Update],
            constraints: None,
        };
        let compiled = compile_key_read_grant_filters(&[wrong_resource, wrong_action]);
        assert!(compiled.is_empty());
    }

    #[test]
    fn empty_grants_yield_no_visibility() {
        let identity_id = 42;
        let key = key_fixture(1, "a.token", OwnerType::System, None, None, None, false);
        assert_parity(&[], identity_id, &key, "no-grants");
        assert!(compile_key_read_grant_filters(&[]).is_empty());
    }
}
