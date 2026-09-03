//! Authentication routes

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};

use sqlx::PgConnection;
use validator::Validate;

use attune_common::auth::{
    hash_integration_token, jwt::generate_sensor_token_with_cache_authority_and_workload_fence,
};
use attune_common::models::{Identity, IntegrationToken, OwnerType, Sensor, SensorWorkloadFence};
use attune_common::rbac::{Action, Grant, GrantConstraints, Resource};
use attune_common::repositories::{
    identity::{
        CreateIdentityInput, IdentityRepository, IdentityRoleAssignmentRepository,
        PermissionSetRepository, UpdateIdentityInput,
    },
    sensor_admission::SensorAdmissionRepository,
    sensor_workload::SensorWorkloadRepository,
    trigger::{SensorRepository, TriggerRepository},
    Create, FindById, FindByRef, IntegrationTokenRepository, Update,
};

use crate::{
    auth::{
        hash_password,
        jwt::{
            generate_access_token, generate_integration_refresh_token, generate_refresh_token,
            validate_token, TokenType,
        },
        middleware::RequireAuth,
        oidc::{
            apply_cookies_to_headers, build_login_redirect, build_logout_redirect,
            cookie_authenticated_user, get_cookie_value, has_oidc_session,
            oidc_callback_redirect_response, OidcCallbackQuery, REFRESH_COOKIE_NAME,
        },
        verify_password,
    },
    dto::{
        ApiResponse, AuthSettingsResponse, ChangePasswordRequest, CurrentUserResponse,
        EffectivePermissionResponse, LoginRequest, ProviderProfileResponse, RefreshTokenRequest,
        RegisterRequest, SuccessResponse, TokenLoginRequest, TokenResponse,
        UpdateCurrentUserRequest,
    },
    middleware::error::ApiError,
    state::SharedState,
};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use utoipa::ToSchema;

/// Request body for creating sensor tokens
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateSensorTokenRequest {
    /// Sensor reference (e.g., "core.timer")
    #[validate(length(min = 1, max = 255))]
    pub sensor_ref: String,

    /// Registered pack reference. Internal worker callers must provide it;
    /// public callers may omit it and let the API resolve it.
    #[serde(default)]
    pub pack_ref: Option<String>,

    /// List of trigger types this sensor can create events for
    #[validate(length(min = 1))]
    pub trigger_types: Vec<String>,

    /// Explicit sensor cache permission-set refs. `standard` grants read-only
    /// access to the registered sensor and pack cache scopes.
    #[serde(default)]
    pub permission_set_refs: Vec<String>,

    /// Optional TTL in seconds (default: 86400 = 24 hours, max: 259200 = 72 hours)
    #[validate(range(min = 3600, max = 259200))]
    pub ttl_seconds: Option<i64>,
}

/// Request body for internal sensor token creation/reissue.
///
/// Worker/service tokens must provide `sensor_ref` and `trigger_types`.
/// Sensor-token refresh calls may omit those fields; the server will derive them
/// from authenticated identity state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate, ToSchema)]
pub struct InternalCreateSensorTokenRequest {
    /// Sensor reference (required for worker/service callers)
    #[validate(length(min = 1, max = 255))]
    pub sensor_ref: Option<String>,

    /// Registered pack reference (required for worker/service callers).
    #[validate(length(min = 1, max = 255))]
    pub pack_ref: Option<String>,

    /// List of trigger types this sensor can create events for (required for worker/service callers)
    #[validate(length(min = 1))]
    pub trigger_types: Option<Vec<String>>,

    /// Explicit cache permission-set refs (required, though it may be empty,
    /// for worker/service callers).
    pub permission_set_refs: Option<Vec<String>>,

    /// Assigned sensor workload ID (required for worker/service callers).
    pub workload_id: Option<i64>,

    /// Current sensor workload assignment generation (required for worker/service callers).
    pub assignment_generation: Option<i64>,

    /// Worker process instance that owns the assignment (required for worker/service callers).
    pub worker_instance: Option<uuid::Uuid>,

    /// Optional TTL in seconds (default: 86400 = 24 hours, max: 259200 = 72 hours)
    #[validate(range(min = 3600, max = 259200))]
    pub ttl_seconds: Option<i64>,
}

/// Response for sensor token creation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SensorTokenResponse {
    pub identity_id: i64,
    pub sensor_ref: String,
    pub pack_ref: Option<String>,
    pub token: String,
    pub expires_at: String,
    pub trigger_types: Vec<String>,
    pub permission_set_refs: Vec<String>,
    #[schema(value_type = Option<Object>)]
    pub workload_fence: Option<SensorWorkloadFence>,
}

/// Create authentication routes
pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/settings", get(auth_settings))
        .route("/login", post(login))
        .route("/token-login", post(token_login))
        .route("/oidc/login", get(oidc_login))
        .route("/callback", get(oidc_callback))
        .route("/ldap/login", post(ldap_login))
        .route("/logout", get(logout))
        .route("/register", post(register))
        .route("/refresh", post(refresh_token))
        .route("/me", get(get_current_user).put(update_current_user))
        .route("/change-password", post(change_password))
        .route("/internal/sensor-token", post(create_sensor_token_internal))
}

fn identity_auth_provider(identity: &Identity) -> &'static str {
    if identity.attributes.get("oidc").is_some() {
        "oidc"
    } else if identity.attributes.get("ldap").is_some() {
        "ldap"
    } else {
        "local"
    }
}

fn current_user_response(
    identity: Identity,
    effective_permissions: Vec<EffectivePermissionResponse>,
    assigned_permission_set_refs: Vec<String>,
) -> CurrentUserResponse {
    let auth_provider = identity_auth_provider(&identity).to_string();
    let is_local = auth_provider == "local";
    let can_change_password = is_local && identity.password_hash.is_some();
    let provider_profile = provider_profile_response(&identity);

    CurrentUserResponse {
        id: identity.id,
        login: identity.login,
        display_name: identity.display_name,
        auth_provider,
        is_local,
        can_change_password,
        provider_profile,
        effective_permissions,
        assigned_permission_set_refs,
    }
}

async fn assigned_permission_set_refs(
    state: &SharedState,
    identity_id: i64,
) -> Result<Vec<String>, ApiError> {
    let mut permission_sets =
        PermissionSetRepository::find_by_identity(&state.db, identity_id).await?;
    let roles =
        IdentityRoleAssignmentRepository::find_role_names_by_identity(&state.db, identity_id)
            .await?;
    permission_sets.extend(PermissionSetRepository::find_by_roles(&state.db, &roles).await?);

    let mut refs = BTreeSet::new();
    for permission_set in permission_sets {
        refs.insert(permission_set.r#ref);
    }

    Ok(refs.into_iter().collect())
}

fn effective_permissions_response(grants: Vec<Grant>) -> Vec<EffectivePermissionResponse> {
    let mut seen = HashSet::new();
    let mut permissions = Vec::new();

    for grant in grants {
        let resource = resource_name(grant.resource).to_string();
        let actions: Vec<String> = grant
            .actions
            .into_iter()
            .map(action_name)
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if actions.is_empty() {
            continue;
        }

        let constraints = grant
            .constraints
            .and_then(|value| serde_json::to_value(value).ok());

        let dedupe_key = format!(
            "{}|{}|{}",
            resource,
            actions.join(","),
            constraints
                .as_ref()
                .map(serde_json::Value::to_string)
                .unwrap_or_default()
        );
        if seen.insert(dedupe_key) {
            permissions.push(EffectivePermissionResponse {
                resource,
                actions,
                constraints,
            });
        }
    }

    permissions.sort_by(|a, b| {
        a.resource
            .cmp(&b.resource)
            .then_with(|| a.actions.cmp(&b.actions))
            .then_with(|| {
                a.constraints
                    .as_ref()
                    .map(serde_json::Value::to_string)
                    .unwrap_or_default()
                    .cmp(
                        &b.constraints
                            .as_ref()
                            .map(serde_json::Value::to_string)
                            .unwrap_or_default(),
                    )
            })
    });
    permissions
}

fn resource_name(resource: Resource) -> &'static str {
    match resource {
        Resource::Packs => "packs",
        Resource::Actions => "actions",
        Resource::Policies => "policies",
        Resource::Queues => "queues",
        Resource::QueueItems => "queue_items",
        Resource::Rules => "rules",
        Resource::Triggers => "triggers",
        Resource::Executions => "executions",
        Resource::Events => "events",
        Resource::Enforcements => "enforcements",
        Resource::Inquiries => "inquiries",
        Resource::Keys => "keys",
        Resource::Caches => "caches",
        Resource::Artifacts => "artifacts",
        Resource::Runtimes => "runtimes",
        Resource::Workers => "workers",
        Resource::Dashboards => "dashboards",
        Resource::Retention => "retention",
        Resource::Identities => "identities",
        Resource::Permissions => "permissions",
        Resource::AuditLog => "audit_log",
    }
}

fn action_name(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Create => "create",
        Action::Install => "install",
        Action::Configure => "configure",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Execute => "execute",
        Action::Cancel => "cancel",
        Action::Respond => "respond",
        Action::Manage => "manage",
        Action::Decrypt => "decrypt",
    }
}

fn string_attr(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

fn bool_attr(value: &serde_json::Value, key: &str) -> Option<bool> {
    value.get(key).and_then(|value| value.as_bool())
}

fn groups_attr(value: &serde_json::Value) -> Vec<String> {
    value
        .get("groups")
        .and_then(|value| value.as_array())
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn provider_profile_response(identity: &Identity) -> Option<ProviderProfileResponse> {
    if let Some(oidc) = identity.attributes.get("oidc") {
        return Some(ProviderProfileResponse {
            provider: "oidc".to_string(),
            display_name: string_attr(oidc, "name").or_else(|| identity.display_name.clone()),
            login: string_attr(oidc, "preferred_username"),
            email: string_attr(oidc, "email"),
            email_verified: bool_attr(oidc, "email_verified"),
            subject: string_attr(oidc, "sub"),
            issuer: string_attr(oidc, "issuer"),
            distinguished_name: None,
            groups: groups_attr(oidc),
        });
    }

    identity
        .attributes
        .get("ldap")
        .map(|ldap| ProviderProfileResponse {
            provider: "ldap".to_string(),
            display_name: string_attr(ldap, "display_name")
                .or_else(|| identity.display_name.clone()),
            login: string_attr(ldap, "login"),
            email: string_attr(ldap, "email"),
            email_verified: None,
            subject: None,
            issuer: None,
            distinguished_name: string_attr(ldap, "dn"),
            groups: groups_attr(ldap),
        })
}

fn require_access_token(user: &crate::auth::middleware::AuthenticatedUser) -> Result<(), ApiError> {
    if user.claims.token_type != TokenType::Access {
        return Err(ApiError::Forbidden(
            "User profile changes require a user access token".to_string(),
        ));
    }
    Ok(())
}

/// Authentication settings endpoint
///
/// GET /auth/settings
#[utoipa::path(
    get,
    path = "/auth/settings",
    tag = "auth",
    responses(
        (status = 200, description = "Authentication settings", body = inline(ApiResponse<AuthSettingsResponse>))
    )
)]
pub async fn auth_settings(
    State(state): State<SharedState>,
) -> Result<Json<ApiResponse<AuthSettingsResponse>>, ApiError> {
    let oidc = state
        .config
        .security
        .oidc
        .as_ref()
        .filter(|oidc| oidc.enabled);

    let ldap = state
        .config
        .security
        .ldap
        .as_ref()
        .filter(|ldap| ldap.enabled);

    let response = AuthSettingsResponse {
        authentication_enabled: state.config.security.enable_auth,
        local_password_enabled: state.config.security.enable_auth,
        local_password_visible_by_default: state.config.security.enable_auth
            && state.config.security.login_page.show_local_login,
        oidc_enabled: oidc.is_some(),
        oidc_visible_by_default: oidc.is_some() && state.config.security.login_page.show_oidc_login,
        oidc_provider_name: oidc.map(|oidc| oidc.provider_name.clone()),
        oidc_provider_label: oidc.map(|oidc| {
            oidc.provider_label
                .clone()
                .unwrap_or_else(|| oidc.provider_name.clone())
        }),
        oidc_provider_icon_url: oidc.and_then(|oidc| oidc.provider_icon_url.clone()),
        ldap_enabled: ldap.is_some(),
        ldap_visible_by_default: ldap.is_some() && state.config.security.login_page.show_ldap_login,
        ldap_provider_name: ldap.map(|ldap| ldap.provider_name.clone()),
        ldap_provider_label: ldap.map(|ldap| {
            ldap.provider_label
                .clone()
                .unwrap_or_else(|| ldap.provider_name.clone())
        }),
        ldap_provider_icon_url: ldap.and_then(|ldap| ldap.provider_icon_url.clone()),
        self_registration_enabled: state.config.security.allow_self_registration,
    };

    Ok(Json(ApiResponse::new(response)))
}

/// Login endpoint
///
/// POST /auth/login
#[utoipa::path(
    post,
    path = "/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Successfully logged in", body = inline(ApiResponse<TokenResponse>)),
        (status = 401, description = "Invalid credentials"),
        (status = 400, description = "Validation error")
    )
)]
pub async fn login(
    State(state): State<SharedState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    use attune_common::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};

    let emit_failure = |reason: &str| {
        let event = AuditEventBuilder::new(
            AuditCategory::Auth,
            "auth.login.failure",
            AuditOutcome::Failure,
        )
        .actor_login(payload.login.clone())
        .with_details(serde_json::json!({ "reason": reason }))
        .build();
        state.audit_emitter.emit(event);
    };

    // Validate request
    if let Err(e) = payload.validate() {
        emit_failure("validation_error");
        return Err(ApiError::ValidationError(format!(
            "Invalid login request: {}",
            e
        )));
    }

    // Find identity by login
    let identity = match IdentityRepository::find_by_login(&state.db, &payload.login).await? {
        Some(i) => i,
        None => {
            emit_failure("unknown_user");
            return Err(ApiError::Unauthorized(
                "Invalid login or password".to_string(),
            ));
        }
    };

    if identity.frozen {
        emit_failure("frozen");
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot authenticate".to_string(),
        ));
    }

    // Check if identity has a password set
    let password_hash = match identity.password_hash.as_ref() {
        Some(h) => h,
        None => {
            emit_failure("no_password");
            return Err(ApiError::Unauthorized(
                "Invalid login or password".to_string(),
            ));
        }
    };

    // Verify password
    let is_valid = verify_password(&payload.password, password_hash).map_err(|_| {
        emit_failure("password_verify_error");
        ApiError::Unauthorized("Invalid login or password".to_string())
    })?;

    if !is_valid {
        emit_failure("invalid_password");
        return Err(ApiError::Unauthorized(
            "Invalid login or password".to_string(),
        ));
    }

    // Generate tokens
    let access_token = generate_access_token(identity.id, &identity.login, &state.jwt_config)?;
    let refresh_token = generate_refresh_token(identity.id, &identity.login, &state.jwt_config)?;

    let response = TokenResponse::new(
        access_token,
        refresh_token,
        state.jwt_config.access_token_expiration,
    )
    .with_user(
        identity.id,
        identity.login.clone(),
        identity.display_name.clone(),
    );

    // Audit success
    state.audit_emitter.emit(
        AuditEventBuilder::new(
            AuditCategory::Auth,
            "auth.login.success",
            AuditOutcome::Success,
        )
        .actor_identity(identity.id)
        .actor_login(identity.login.clone())
        .build(),
    );

    let mut http_response = Json(ApiResponse::new(response)).into_response();
    apply_cookies_to_headers(
        http_response.headers_mut(),
        &crate::auth::oidc::clear_auth_cookies(&state),
    )?;
    Ok(http_response)
}

/// Passwordless integration-token login endpoint.
///
/// POST /auth/token-login
#[utoipa::path(
    post,
    path = "/auth/token-login",
    tag = "auth",
    request_body = TokenLoginRequest,
    responses(
        (status = 200, description = "Successfully logged in with integration token", body = inline(ApiResponse<TokenResponse>)),
        (status = 401, description = "Invalid integration token"),
        (status = 400, description = "Validation error")
    )
)]
pub async fn token_login(
    State(state): State<SharedState>,
    Json(payload): Json<TokenLoginRequest>,
) -> Result<Json<ApiResponse<TokenResponse>>, ApiError> {
    use attune_common::audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome};

    let emit_failure = |reason: &str| {
        state.audit_emitter.emit(
            AuditEventBuilder::new(
                AuditCategory::Auth,
                event_type::auth::TOKEN_LOGIN_FAILURE,
                AuditOutcome::Failure,
            )
            .with_details(serde_json::json!({ "reason": reason }))
            .build(),
        );
    };

    if let Err(e) = payload.validate() {
        emit_failure("validation_error");
        return Err(ApiError::ValidationError(format!(
            "Invalid token login request: {}",
            e
        )));
    }

    let token_hash = hash_integration_token(&payload.token);
    let integration_token =
        match IntegrationTokenRepository::find_by_hash(&state.db, &token_hash).await? {
            Some(token) if integration_token_is_active(&token) => token,
            Some(_) => {
                emit_failure("inactive_token");
                return Err(ApiError::Unauthorized("Invalid token".to_string()));
            }
            None => {
                emit_failure("unknown_token");
                return Err(ApiError::Unauthorized("Invalid token".to_string()));
            }
        };

    let identity = match active_identity_for_integration_token(&state, &integration_token).await {
        Ok(identity) => identity,
        Err(err) => {
            emit_failure("invalid_identity");
            return Err(err);
        }
    };

    IntegrationTokenRepository::touch_last_used(&state.db, integration_token.id, None).await?;

    let response = integration_token_response(&identity, integration_token.id, &state.jwt_config)?;

    state.audit_emitter.emit(
        AuditEventBuilder::new(
            AuditCategory::Auth,
            event_type::auth::TOKEN_LOGIN_SUCCESS,
            AuditOutcome::Success,
        )
        .actor_identity(identity.id)
        .actor_login(identity.login.clone())
        .resource("integration_token")
        .resource_id(integration_token.id)
        .resource_ref(integration_token.label)
        .build(),
    );

    Ok(Json(ApiResponse::new(response)))
}

fn integration_token_is_active(token: &IntegrationToken) -> bool {
    token.revoked_at.is_none()
        && token
            .expires_at
            .map(|expires_at| expires_at > chrono::Utc::now())
            .unwrap_or(true)
}

async fn active_identity_for_integration_token(
    state: &SharedState,
    token: &IntegrationToken,
) -> Result<Identity, ApiError> {
    let identity = IdentityRepository::find_by_id(&state.db, token.identity)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("Invalid token".to_string()))?;

    if identity.frozen {
        return Err(ApiError::Unauthorized("Invalid token".to_string()));
    }

    Ok(identity)
}

fn integration_token_response(
    identity: &Identity,
    integration_token_id: i64,
    jwt_config: &crate::auth::jwt::JwtConfig,
) -> Result<TokenResponse, ApiError> {
    let access_token = generate_access_token(identity.id, &identity.login, jwt_config)?;
    let refresh_token = generate_integration_refresh_token(
        integration_token_id,
        identity.id,
        &identity.login,
        jwt_config,
    )?;

    Ok(TokenResponse::new(
        access_token,
        refresh_token,
        jwt_config.access_token_expiration,
    )
    .with_user(
        identity.id,
        identity.login.clone(),
        identity.display_name.clone(),
    ))
}

/// Register endpoint
///
/// POST /auth/register
#[utoipa::path(
    post,
    path = "/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    responses(
        (status = 200, description = "Successfully registered", body = inline(ApiResponse<TokenResponse>)),
        (status = 409, description = "User already exists"),
        (status = 400, description = "Validation error")
    )
)]
pub async fn register(
    State(state): State<SharedState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<ApiResponse<TokenResponse>>, ApiError> {
    if !state.config.security.allow_self_registration {
        return Err(ApiError::Forbidden(
            "Self-service registration is disabled; identities must be provisioned by an administrator or identity provider".to_string(),
        ));
    }

    // Validate request
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid registration request: {}", e)))?;

    // Check if login already exists
    if IdentityRepository::find_by_login(&state.db, &payload.login)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Identity with login '{}' already exists",
            payload.login
        )));
    }

    // Hash password
    let password_hash = hash_password(&payload.password)?;

    // Registration creates an identity only; permission assignments are managed separately.
    let input = CreateIdentityInput {
        login: payload.login.clone(),
        display_name: payload.display_name,
        password_hash: Some(password_hash),
        attributes: serde_json::json!({}),
    };

    let identity = IdentityRepository::create(&state.db, input).await?;

    // Generate tokens
    let access_token = generate_access_token(identity.id, &identity.login, &state.jwt_config)?;
    let refresh_token = generate_refresh_token(identity.id, &identity.login, &state.jwt_config)?;

    let response = TokenResponse::new(
        access_token,
        refresh_token,
        state.jwt_config.access_token_expiration,
    )
    .with_user(
        identity.id,
        identity.login.clone(),
        identity.display_name.clone(),
    );

    Ok(Json(ApiResponse::new(response)))
}

/// Refresh token endpoint
///
/// POST /auth/refresh
#[utoipa::path(
    post,
    path = "/auth/refresh",
    tag = "auth",
    request_body = RefreshTokenRequest,
    responses(
        (status = 200, description = "Successfully refreshed token", body = inline(ApiResponse<TokenResponse>)),
        (status = 401, description = "Invalid or expired refresh token"),
        (status = 400, description = "Validation error")
    )
)]
pub async fn refresh_token(
    State(state): State<SharedState>,
    headers: HeaderMap,
    payload: Option<Json<RefreshTokenRequest>>,
) -> Result<Response, ApiError> {
    let browser_cookie_refresh = payload.is_none();
    let refresh_token = if let Some(Json(payload)) = payload {
        payload.validate().map_err(|e| {
            ApiError::ValidationError(format!("Invalid refresh token request: {}", e))
        })?;
        payload.refresh_token
    } else {
        get_cookie_value(&headers, REFRESH_COOKIE_NAME)
            .ok_or_else(|| ApiError::Unauthorized("Missing refresh token".to_string()))?
    };

    // Validate refresh token
    let claims = validate_token(&refresh_token, &state.jwt_config)
        .map_err(|_| ApiError::Unauthorized("Invalid or expired refresh token".to_string()))?;

    // Ensure it's a refresh token
    if claims.token_type != TokenType::Refresh {
        return Err(ApiError::Unauthorized("Invalid token type".to_string()));
    }

    if claims.scope.as_deref() == Some("integration_token") {
        return refresh_integration_token(state, claims, browser_cookie_refresh).await;
    }

    // Parse identity ID
    let identity_id: i64 = claims
        .sub
        .parse()
        .map_err(|_| ApiError::Unauthorized("Invalid token".to_string()))?;

    // Verify identity still exists
    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("Identity not found".to_string()))?;

    if identity.frozen {
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot authenticate".to_string(),
        ));
    }

    // Generate new tokens
    let access_token = generate_access_token(identity.id, &identity.login, &state.jwt_config)?;
    let refresh_token = generate_refresh_token(identity.id, &identity.login, &state.jwt_config)?;

    let response = TokenResponse::new(
        access_token,
        refresh_token,
        state.jwt_config.access_token_expiration,
    );
    let response_body = Json(ApiResponse::new(response.clone()));

    if browser_cookie_refresh {
        let mut http_response = response_body.into_response();
        apply_cookies_to_headers(
            http_response.headers_mut(),
            &crate::auth::oidc::build_auth_cookies(&state, &response, ""),
        )?;
        return Ok(http_response);
    }

    Ok(response_body.into_response())
}

async fn refresh_integration_token(
    state: SharedState,
    claims: attune_common::auth::jwt::Claims,
    browser_cookie_refresh: bool,
) -> Result<Response, ApiError> {
    let integration_token_id: i64 = claims
        .sub
        .parse()
        .map_err(|_| ApiError::Unauthorized("Invalid or expired refresh token".to_string()))?;

    let integration_token = IntegrationTokenRepository::find_by_id(&state.db, integration_token_id)
        .await?
        .filter(integration_token_is_active)
        .ok_or_else(|| ApiError::Unauthorized("Invalid or expired refresh token".to_string()))?;

    let identity = active_identity_for_integration_token(&state, &integration_token)
        .await
        .map_err(|_| ApiError::Unauthorized("Invalid or expired refresh token".to_string()))?;

    IntegrationTokenRepository::touch_last_used(&state.db, integration_token.id, None).await?;

    let response = integration_token_response(&identity, integration_token.id, &state.jwt_config)?;
    let response_body = Json(ApiResponse::new(response.clone()));

    if browser_cookie_refresh {
        let mut http_response = response_body.into_response();
        apply_cookies_to_headers(
            http_response.headers_mut(),
            &crate::auth::oidc::build_auth_cookies(&state, &response, ""),
        )?;
        return Ok(http_response);
    }

    Ok(response_body.into_response())
}

/// Get current user endpoint
///
/// GET /auth/me
#[utoipa::path(
    get,
    path = "/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "Current user information", body = inline(ApiResponse<CurrentUserResponse>)),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Identity not found")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
pub async fn get_current_user(
    State(state): State<SharedState>,
    headers: HeaderMap,
    user: Result<RequireAuth, crate::auth::middleware::AuthError>,
) -> Result<Json<ApiResponse<CurrentUserResponse>>, ApiError> {
    let authenticated_user = match user {
        Ok(RequireAuth(user)) => user,
        Err(_) => cookie_authenticated_user(&headers, &state)?
            .ok_or_else(|| ApiError::Unauthorized("Unauthorized".to_string()))?,
    };
    let identity_id = authenticated_user.identity_id()?;

    // Fetch identity from database
    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Identity not found".to_string()))?;

    if identity.frozen {
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot authenticate".to_string(),
        ));
    }

    let grants = state
        .authorization_service()
        .effective_grants(&authenticated_user)
        .await?;
    let assigned_permission_set_refs = assigned_permission_set_refs(&state, identity_id).await?;
    let response = current_user_response(
        identity,
        effective_permissions_response(grants),
        assigned_permission_set_refs,
    );

    Ok(Json(ApiResponse::new(response)))
}

/// Update current user profile endpoint
///
/// PUT /auth/me
#[utoipa::path(
    put,
    path = "/auth/me",
    tag = "auth",
    request_body = UpdateCurrentUserRequest,
    responses(
        (status = 200, description = "Current user profile updated", body = inline(ApiResponse<CurrentUserResponse>)),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Profile is managed by an external provider"),
        (status = 404, description = "Identity not found")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
pub async fn update_current_user(
    State(state): State<SharedState>,
    RequireAuth(user): RequireAuth,
    Json(payload): Json<UpdateCurrentUserRequest>,
) -> Result<Json<ApiResponse<CurrentUserResponse>>, ApiError> {
    require_access_token(&user)?;
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid profile update request: {}", e)))?;

    let identity_id = user.identity_id()?;
    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Identity not found".to_string()))?;

    if identity.frozen {
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot update its profile".to_string(),
        ));
    }

    if identity_auth_provider(&identity) != "local" {
        return Err(ApiError::Forbidden(
            "Profile details are managed by the configured identity provider".to_string(),
        ));
    }

    let normalized_display_name = payload
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let identity =
        IdentityRepository::update_display_name(&state.db, identity_id, normalized_display_name)
            .await?;
    let grants = state
        .authorization_service()
        .effective_grants(&user)
        .await?;
    let assigned_permission_set_refs = assigned_permission_set_refs(&state, identity_id).await?;

    Ok(Json(ApiResponse::new(current_user_response(
        identity,
        effective_permissions_response(grants),
        assigned_permission_set_refs,
    ))))
}

/// Request body for LDAP login.
#[derive(Debug, Serialize, Deserialize, Validate, ToSchema)]
pub struct LdapLoginRequest {
    /// User login name (uid, sAMAccountName, etc.)
    #[validate(length(min = 1, max = 255))]
    pub login: String,
    /// User password
    #[validate(length(min = 1, max = 512))]
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct OidcLoginParams {
    pub redirect_to: Option<String>,
    /// Optional local callback URI for CLI SSO login (must be http://localhost or http://127.0.0.1).
    pub cli_redirect_uri: Option<String>,
}

/// Begin browser OIDC login by redirecting to the provider.
#[utoipa::path(
    get,
    path = "/auth/oidc/login",
    tag = "auth",
    params(
        ("redirect_to" = Option<String>, Query, description = "Application path to return to after login"),
        ("cli_redirect_uri" = Option<String>, Query, description = "Local CLI callback URI"),
    ),
    responses(
        (status = 307, description = "Redirect to the configured OIDC provider"),
        (status = 501, description = "OIDC is not configured"),
    )
)]
pub async fn oidc_login(
    State(state): State<SharedState>,
    Query(params): Query<OidcLoginParams>,
) -> Result<Response, ApiError> {
    let login_redirect = build_login_redirect(
        &state,
        params.redirect_to.as_deref(),
        params.cli_redirect_uri.as_deref(),
    )
    .await?;
    let mut response = Redirect::temporary(&login_redirect.authorization_url).into_response();
    apply_cookies_to_headers(response.headers_mut(), &login_redirect.cookies)?;
    Ok(response)
}

/// Handle the OIDC authorization code callback.
#[utoipa::path(
    get,
    path = "/auth/callback",
    tag = "auth",
    responses(
        (status = 307, description = "Redirect to the application or CLI callback"),
        (status = 400, description = "Invalid OIDC callback"),
        (status = 401, description = "OIDC authentication failed"),
    )
)]
pub async fn oidc_callback(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<OidcCallbackQuery>,
) -> Result<Response, ApiError> {
    let redirect_to = get_cookie_value(&headers, crate::auth::oidc::OIDC_REDIRECT_COOKIE_NAME);
    let cli_redirect_uri =
        get_cookie_value(&headers, crate::auth::oidc::OIDC_CLI_REDIRECT_COOKIE_NAME);
    let authenticated = crate::auth::oidc::handle_callback(&state, &headers, &query).await?;
    oidc_callback_redirect_response(
        &state,
        &authenticated.token_response,
        redirect_to,
        &authenticated.id_token,
        cli_redirect_uri,
    )
}

/// Authenticate via LDAP directory.
///
/// POST /auth/ldap/login
#[utoipa::path(
    post,
    path = "/auth/ldap/login",
    tag = "auth",
    request_body = LdapLoginRequest,
    responses(
        (status = 200, description = "Successfully authenticated via LDAP", body = inline(ApiResponse<TokenResponse>)),
        (status = 401, description = "Invalid LDAP credentials"),
        (status = 501, description = "LDAP not configured")
    )
)]
pub async fn ldap_login(
    State(state): State<SharedState>,
    Json(payload): Json<LdapLoginRequest>,
) -> Result<Response, ApiError> {
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid LDAP login request: {e}")))?;

    let authenticated =
        crate::auth::ldap::authenticate(&state, &payload.login, &payload.password).await?;

    let mut response = Json(ApiResponse::new(authenticated.token_response)).into_response();
    apply_cookies_to_headers(
        response.headers_mut(),
        &crate::auth::oidc::clear_auth_cookies(&state),
    )?;
    Ok(response)
}

/// Logout the current browser session and optionally redirect through the provider logout flow.
#[utoipa::path(
    get,
    path = "/auth/logout",
    tag = "auth",
    responses(
        (status = 307, description = "Redirect after clearing the browser session")
    )
)]
pub async fn logout(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let oidc_enabled = state
        .config
        .security
        .oidc
        .as_ref()
        .is_some_and(|oidc| oidc.enabled);

    let response = if oidc_enabled && has_oidc_session(&headers) {
        let logout_redirect = build_logout_redirect(&state, &headers).await?;
        let mut response = Redirect::temporary(&logout_redirect.redirect_url).into_response();
        apply_cookies_to_headers(response.headers_mut(), &logout_redirect.cookies)?;
        response
    } else {
        let mut response = Redirect::temporary("/login").into_response();
        apply_cookies_to_headers(
            response.headers_mut(),
            &crate::auth::oidc::clear_auth_cookies(&state),
        )?;
        response
    };

    Ok(response)
}

/// Change password endpoint
///
/// POST /auth/change-password
#[utoipa::path(
    post,
    path = "/auth/change-password",
    tag = "auth",
    request_body = ChangePasswordRequest,
    responses(
        (status = 200, description = "Password changed successfully", body = inline(ApiResponse<SuccessResponse>)),
        (status = 401, description = "Invalid current password or unauthorized"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Identity not found")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
pub async fn change_password(
    State(state): State<SharedState>,
    RequireAuth(user): RequireAuth,
    Json(payload): Json<ChangePasswordRequest>,
) -> Result<Json<ApiResponse<SuccessResponse>>, ApiError> {
    require_access_token(&user)?;

    // Validate request
    payload.validate().map_err(|e| {
        ApiError::ValidationError(format!("Invalid change password request: {}", e))
    })?;

    let identity_id = user.identity_id()?;

    // Fetch identity from database
    let identity = IdentityRepository::find_by_id(&state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Identity not found".to_string()))?;

    if identity.frozen {
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot change its password".to_string(),
        ));
    }

    if identity_auth_provider(&identity) != "local" {
        return Err(ApiError::Forbidden(
            "Passwords for this identity are managed by the configured identity provider"
                .to_string(),
        ));
    }

    // Get current password hash
    let current_password_hash = identity
        .password_hash
        .as_ref()
        .ok_or_else(|| ApiError::Unauthorized("No password set".to_string()))?;

    // Verify current password
    let is_valid = verify_password(&payload.current_password, current_password_hash)
        .map_err(|_| ApiError::Unauthorized("Invalid current password".to_string()))?;

    if !is_valid {
        return Err(ApiError::Unauthorized(
            "Invalid current password".to_string(),
        ));
    }

    // Hash new password
    let new_password_hash = hash_password(&payload.new_password)?;

    // Update identity in database with new password hash
    use attune_common::repositories::identity::UpdateIdentityInput;
    use attune_common::repositories::Update;

    let update_input = UpdateIdentityInput {
        display_name: None,
        password_hash: Some(new_password_hash),
        attributes: None,
        frozen: None,
    };

    IdentityRepository::update(&state.db, identity_id, update_input).await?;

    Ok(Json(ApiResponse::new(SuccessResponse::new(
        "Password changed successfully",
    ))))
}

/// Create sensor token endpoint for internal service-to-service use.
///
/// POST /auth/internal/sensor-token
///
/// Worker/service callers can provision tokens by supplying `sensor_ref` and
/// `trigger_types`. Sensor callers can refresh their own tokens; `sensor_ref`
/// and `trigger_types` are derived from authenticated sensor identity state.
#[utoipa::path(
    post,
    path = "/auth/internal/sensor-token",
    tag = "auth",
    request_body = InternalCreateSensorTokenRequest,
    responses(
        (status = 200, description = "Sensor token created successfully", body = inline(ApiResponse<SensorTokenResponse>)),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
pub async fn create_sensor_token_internal(
    State(state): State<SharedState>,
    RequireAuth(user): RequireAuth,
    Json(payload): Json<InternalCreateSensorTokenRequest>,
) -> Result<Json<ApiResponse<SensorTokenResponse>>, ApiError> {
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid sensor token request: {}", e)))?;

    match user.claims.token_type {
        TokenType::Sensor => {
            let workload_fence = user.sensor_workload_fence().map_err(|_| {
                ApiError::Unauthorized("Sensor token is missing a valid workload fence".to_string())
            })?;
            let identity_id = user
                .identity_id()
                .map_err(|_| ApiError::Unauthorized("Invalid sensor token subject".to_string()))?;

            let identity = IdentityRepository::find_by_id(&state.db, identity_id)
                .await?
                .ok_or_else(|| {
                    ApiError::Unauthorized(
                        "Sensor identity for token refresh was not found".to_string(),
                    )
                })?;
            ensure_identity_not_frozen_for_authentication(&identity)?;

            let request = refresh_request_from_sensor_identity(
                &state,
                &user.claims,
                &identity,
                payload.ttl_seconds,
            )
            .await?;
            create_sensor_token_impl(state, request, true, workload_fence).await
        }
        TokenType::Worker => {
            let workload_fence = payload.workload_fence_from_worker_claims(&user.claims)?;
            let request = payload.into_create_request()?;
            create_sensor_token_impl(state, request, true, workload_fence).await
        }
        _ => Err(ApiError::Unauthorized(
            "Only worker or sensor tokens can access this endpoint".to_string(),
        )),
    }
}

struct RegisteredSensorAuthority {
    sensor: Sensor,
    pack_ref: String,
    trigger_types: Vec<String>,
    permission_set_refs: Vec<String>,
    cache_grants: Vec<Grant>,
}

fn canonical_string_refs(refs: &[String]) -> Vec<String> {
    let mut refs = refs
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    refs
}

fn registered_sensor_permission_set_refs(sensor: &Sensor) -> Result<Vec<String>, ApiError> {
    let Some(config) = sensor
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(Vec::new());
    };
    let Some(value) = config.get("cache_permission_set_refs") else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| {
        ApiError::ValidationError(format!(
            "Sensor '{}' cache_permission_set_refs must be an array of strings",
            sensor.r#ref
        ))
    })?;
    let refs = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    ApiError::ValidationError(format!(
                        "Sensor '{}' cache_permission_set_refs entries must be non-empty strings",
                        sensor.r#ref
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(canonical_string_refs(&refs))
}

async fn sensor_cache_grant_snapshot(
    connection: &mut PgConnection,
    sensor_ref: &str,
    pack_ref: &str,
    permission_set_refs: &[String],
) -> Result<Vec<Grant>, ApiError> {
    let mut grants = Vec::new();
    if permission_set_refs
        .iter()
        .any(|permission_ref| permission_ref == "standard")
    {
        for (owner_type, owner_ref) in
            [(OwnerType::Sensor, sensor_ref), (OwnerType::Pack, pack_ref)]
        {
            grants.push(Grant {
                resource: Resource::Caches,
                actions: vec![Action::Read],
                constraints: Some(GrantConstraints {
                    owner_types: Some(vec![owner_type]),
                    owner_refs: Some(vec![owner_ref.to_string()]),
                    ..Default::default()
                }),
            });
        }
    }

    let named_refs = permission_set_refs
        .iter()
        .filter(|permission_ref| permission_ref.as_str() != "standard")
        .cloned()
        .collect::<Vec<_>>();
    let permission_sets =
        PermissionSetRepository::find_by_refs(&mut *connection, &named_refs).await?;
    if permission_sets.len() != named_refs.len() {
        return Err(ApiError::ValidationError(
            "One or more sensor cache permission sets do not exist".to_string(),
        ));
    }
    for permission_set in permission_sets {
        let permission_grants: Vec<Grant> =
            serde_json::from_value(permission_set.grants).map_err(|err| {
                ApiError::ValidationError(format!(
                    "Sensor cache permission set '{}' has invalid grants: {err}",
                    permission_set.r#ref
                ))
            })?;
        grants.extend(
            permission_grants
                .into_iter()
                .filter(|grant| grant.resource == Resource::Caches),
        );
    }
    Ok(grants)
}

async fn resolve_registered_sensor_authority(
    connection: &mut PgConnection,
    payload: &CreateSensorTokenRequest,
    require_exact_request_scope: bool,
) -> Result<RegisteredSensorAuthority, ApiError> {
    let sensor = SensorRepository::find_by_ref(&mut *connection, &payload.sensor_ref)
        .await?
        .ok_or_else(|| {
            ApiError::ValidationError(format!(
                "Registered sensor '{}' was not found",
                payload.sensor_ref
            ))
        })?;
    if !sensor.enabled {
        return Err(ApiError::Forbidden(format!(
            "Sensor '{}' is disabled",
            sensor.r#ref
        )));
    }
    let pack_ref = sensor.pack_ref.clone().ok_or_else(|| {
        ApiError::ValidationError(format!(
            "Sensor '{}' has no registered pack_ref",
            sensor.r#ref
        ))
    })?;
    if let Some(requested_pack_ref) = payload.pack_ref.as_deref() {
        if requested_pack_ref != pack_ref {
            return Err(ApiError::Forbidden(
                "Sensor token pack scope does not match the registered sensor".to_string(),
            ));
        }
    } else if require_exact_request_scope {
        return Err(ApiError::ValidationError(
            "pack_ref is required for internal sensor token provisioning".to_string(),
        ));
    }

    let registered_triggers =
        TriggerRepository::find_by_sensor(&mut *connection, sensor.id).await?;
    let registered_trigger_refs = canonical_string_refs(
        &registered_triggers
            .into_iter()
            .map(|trigger| trigger.r#ref)
            .collect::<Vec<_>>(),
    );
    if canonical_string_refs(&payload.trigger_types) != registered_trigger_refs {
        return Err(ApiError::Forbidden(
            "Sensor token trigger scope does not match the registered sensor".to_string(),
        ));
    }

    let permission_set_refs = registered_sensor_permission_set_refs(&sensor)?;
    let requested_permission_set_refs = canonical_string_refs(&payload.permission_set_refs);
    if require_exact_request_scope && requested_permission_set_refs != permission_set_refs {
        return Err(ApiError::Forbidden(
            "Sensor token cache authority does not match the registered sensor configuration"
                .to_string(),
        ));
    }
    if !require_exact_request_scope
        && !requested_permission_set_refs.is_empty()
        && requested_permission_set_refs != permission_set_refs
    {
        return Err(ApiError::Forbidden(
            "Requested sensor cache authority does not match the registered sensor configuration"
                .to_string(),
        ));
    }

    let cache_grants =
        sensor_cache_grant_snapshot(connection, &sensor.r#ref, &pack_ref, &permission_set_refs)
            .await?;
    Ok(RegisteredSensorAuthority {
        sensor,
        pack_ref,
        trigger_types: registered_trigger_refs,
        permission_set_refs,
        cache_grants,
    })
}

/// Shared implementation for sensor token creation
async fn create_sensor_token_impl(
    state: SharedState,
    payload: CreateSensorTokenRequest,
    require_exact_request_scope: bool,
    workload_fence: SensorWorkloadFence,
) -> Result<Json<ApiResponse<SensorTokenResponse>>, ApiError> {
    // Validate request
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid sensor token request: {}", e)))?;

    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    let authority =
        resolve_registered_sensor_authority(&mut tx, &payload, require_exact_request_scope).await?;
    if !SensorWorkloadRepository::lock_current_fence(&mut tx, authority.sensor.id, workload_fence)
        .await?
    {
        return Err(ApiError::Forbidden(
            "Sensor workload assignment is stale or expired".to_string(),
        ));
    }
    let sensor_ref = authority.sensor.r#ref.clone();
    let pack_ref = authority.pack_ref.clone();
    let trigger_types = authority.trigger_types.clone();
    let permission_set_refs = authority.permission_set_refs.clone();
    let sensor_login = format!("sensor:{}", sensor_ref);
    let sensor_identity_attributes = serde_json::json!({
        "type": "sensor",
        "sensor_ref": sensor_ref.clone(),
        "pack_ref": pack_ref.clone(),
        "trigger_types": trigger_types.clone(),
        "cache_permission_set_refs": permission_set_refs.clone(),
    });

    let identity = match IdentityRepository::find_by_login(&mut *tx, &sensor_login).await? {
        Some(identity) => {
            ensure_identity_not_frozen_for_authentication(&identity)?;
            if identity.attributes != sensor_identity_attributes {
                IdentityRepository::update(
                    &mut *tx,
                    identity.id,
                    UpdateIdentityInput {
                        attributes: Some(sensor_identity_attributes.clone()),
                        ..Default::default()
                    },
                )
                .await?
            } else {
                identity
            }
        }
        None => {
            // Create new sensor identity
            let input = CreateIdentityInput {
                login: sensor_login.clone(),
                display_name: Some(format!("Sensor: {}", sensor_ref)),
                password_hash: None, // Sensors don't use passwords
                attributes: sensor_identity_attributes.clone(),
            };
            IdentityRepository::create(&mut *tx, input).await?
        }
    };

    let ttl_seconds = payload.ttl_seconds.unwrap_or(86400); // Default: 24 hours
    let token = generate_sensor_token_with_cache_authority_and_workload_fence(
        identity.id,
        &sensor_ref,
        trigger_types.clone(),
        Some(&pack_ref),
        &permission_set_refs,
        &authority.cache_grants,
        workload_fence,
        &state.jwt_config,
        Some(ttl_seconds),
    )?;
    tx.commit().await?;

    // Calculate expiration time
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds);

    let response = SensorTokenResponse {
        identity_id: identity.id,
        sensor_ref,
        pack_ref: Some(pack_ref),
        token,
        expires_at: expires_at.to_rfc3339(),
        trigger_types,
        permission_set_refs,
        workload_fence: Some(workload_fence),
    };

    Ok(Json(ApiResponse::new(response)))
}

fn ensure_identity_not_frozen_for_authentication(identity: &Identity) -> Result<(), ApiError> {
    if identity.frozen {
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot authenticate".to_string(),
        ));
    }
    Ok(())
}

impl InternalCreateSensorTokenRequest {
    fn workload_fence_from_worker_claims(
        &self,
        claims: &crate::auth::jwt::Claims,
    ) -> Result<SensorWorkloadFence, ApiError> {
        let workload_id = self.workload_id.ok_or_else(|| {
            ApiError::ValidationError(
                "workload_id is required for worker token sensor provisioning".to_string(),
            )
        })?;
        let generation = self.assignment_generation.ok_or_else(|| {
            ApiError::ValidationError(
                "assignment_generation is required for worker token sensor provisioning"
                    .to_string(),
            )
        })?;
        let worker_instance = self.worker_instance.ok_or_else(|| {
            ApiError::ValidationError(
                "worker_instance is required for worker token sensor provisioning".to_string(),
            )
        })?;
        let metadata = worker_token_metadata(claims)?;
        if worker_instance != metadata.worker_instance {
            return Err(ApiError::Forbidden(
                "Worker token instance does not match the requested sensor workload assignment"
                    .to_string(),
            ));
        }

        Ok(SensorWorkloadFence {
            workload_id,
            worker_id: metadata.worker_id,
            worker_instance,
            generation,
        })
    }

    fn into_create_request(self) -> Result<CreateSensorTokenRequest, ApiError> {
        let sensor_ref = self.sensor_ref.ok_or_else(|| {
            ApiError::ValidationError(
                "sensor_ref is required for worker token sensor provisioning".to_string(),
            )
        })?;

        let trigger_types = self.trigger_types.ok_or_else(|| {
            ApiError::ValidationError(
                "trigger_types are required for worker token sensor provisioning".to_string(),
            )
        })?;
        let pack_ref = self.pack_ref.ok_or_else(|| {
            ApiError::ValidationError(
                "pack_ref is required for worker token sensor provisioning".to_string(),
            )
        })?;
        let permission_set_refs = self.permission_set_refs.ok_or_else(|| {
            ApiError::ValidationError(
                "permission_set_refs is required for worker token sensor provisioning".to_string(),
            )
        })?;

        let request = CreateSensorTokenRequest {
            sensor_ref,
            pack_ref: Some(pack_ref),
            trigger_types,
            permission_set_refs,
            ttl_seconds: self.ttl_seconds,
        };

        request.validate().map_err(|e| {
            ApiError::ValidationError(format!("Invalid sensor token request: {}", e))
        })?;

        Ok(request)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkerTokenMetadata {
    worker_id: i64,
    worker_instance: uuid::Uuid,
}

fn worker_token_metadata(
    claims: &crate::auth::jwt::Claims,
) -> Result<WorkerTokenMetadata, ApiError> {
    if claims.token_type != TokenType::Worker {
        return Err(ApiError::Unauthorized(
            "Worker token metadata requires a worker token".to_string(),
        ));
    }
    let metadata = claims.metadata.as_ref().ok_or_else(|| {
        ApiError::Unauthorized("Worker token is missing signed metadata".to_string())
    })?;
    let worker_id = metadata
        .get("worker_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|worker_id| *worker_id > 0)
        .ok_or_else(|| {
            ApiError::Unauthorized("Worker token has invalid worker_id metadata".to_string())
        })?;
    let worker_instance = metadata
        .get("worker_instance")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            ApiError::Unauthorized("Worker token has invalid worker_instance metadata".to_string())
        })?;

    Ok(WorkerTokenMetadata {
        worker_id,
        worker_instance,
    })
}

async fn refresh_request_from_sensor_identity(
    state: &SharedState,
    claims: &crate::auth::jwt::Claims,
    identity: &Identity,
    ttl_seconds: Option<i64>,
) -> Result<CreateSensorTokenRequest, ApiError> {
    let sensor_ref = sensor_ref_from_refresh_identity(claims, identity)?;
    let sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
        .await?
        .ok_or_else(|| {
            ApiError::Unauthorized("Registered sensor for token refresh was not found".to_string())
        })?;
    let pack_ref = sensor
        .pack_ref
        .clone()
        .ok_or_else(|| ApiError::Unauthorized("Registered sensor has no pack scope".to_string()))?;
    let triggers = TriggerRepository::find_by_sensor(&state.db, sensor.id).await?;
    let trigger_types = canonical_string_refs(
        &triggers
            .into_iter()
            .map(|trigger| trigger.r#ref)
            .collect::<Vec<_>>(),
    );
    if trigger_types.is_empty() {
        return Err(ApiError::Unauthorized(
            "Registered sensor has no trigger scope".to_string(),
        ));
    }
    let permission_set_refs = registered_sensor_permission_set_refs(&sensor)?;

    Ok(CreateSensorTokenRequest {
        sensor_ref,
        pack_ref: Some(pack_ref),
        trigger_types,
        permission_set_refs,
        ttl_seconds,
    })
}

fn sensor_ref_from_refresh_identity(
    claims: &crate::auth::jwt::Claims,
    identity: &Identity,
) -> Result<String, ApiError> {
    ensure_identity_not_frozen_for_authentication(identity)?;

    let sensor_ref = identity
        .login
        .strip_prefix("sensor:")
        .ok_or_else(|| {
            ApiError::Unauthorized("Token identity is not a sensor identity".to_string())
        })?
        .to_string();

    if claims.login != sensor_ref {
        return Err(ApiError::Unauthorized(
            "Sensor token login does not match identity login".to_string(),
        ));
    }
    Ok(sensor_ref)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sensor_identity(ref_name: &str, trigger_types: serde_json::Value) -> Identity {
        Identity {
            id: 42,
            login: format!("sensor:{}", ref_name),
            display_name: Some("Sensor".to_string()),
            password_hash: None,
            attributes: serde_json::json!({
                "type": "sensor",
                "sensor_ref": ref_name,
                "trigger_types": trigger_types,
            }),
            frozen: false,
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    #[test]
    fn test_internal_worker_payload_requires_scope_fields() {
        let payload = InternalCreateSensorTokenRequest {
            sensor_ref: None,
            pack_ref: None,
            trigger_types: Some(vec!["core.intervaltimer".to_string()]),
            permission_set_refs: Some(Vec::new()),
            workload_id: None,
            assignment_generation: None,
            worker_instance: None,
            ttl_seconds: None,
        };

        let result = payload.into_create_request();
        assert!(result.is_err());
    }

    #[test]
    fn internal_worker_payload_parses_workload_assignment_fields() {
        let worker_instance = uuid::Uuid::new_v4();
        let payload: InternalCreateSensorTokenRequest = serde_json::from_value(serde_json::json!({
            "sensor_ref": "core.timer_sensor",
            "pack_ref": "core",
            "trigger_types": ["core.intervaltimer"],
            "permission_set_refs": [],
            "workload_id": 17,
            "assignment_generation": 4,
            "worker_instance": worker_instance,
            "ttl_seconds": 3600
        }))
        .unwrap();

        assert_eq!(payload.workload_id, Some(17));
        assert_eq!(payload.assignment_generation, Some(4));
        assert_eq!(payload.worker_instance, Some(worker_instance));
    }

    #[test]
    fn worker_token_metadata_parses_signed_worker_identity() {
        let worker_instance = uuid::Uuid::new_v4();
        let claims = crate::auth::jwt::Claims {
            sub: "1".to_string(),
            login: "worker:42".to_string(),
            iat: 100,
            exp: 200,
            token_type: TokenType::Worker,
            scope: Some("internal_files".to_string()),
            metadata: Some(serde_json::json!({
                "worker_id": "42",
                "worker_instance": worker_instance,
            })),
        };

        assert_eq!(
            worker_token_metadata(&claims).unwrap(),
            WorkerTokenMetadata {
                worker_id: 42,
                worker_instance,
            }
        );
    }

    #[test]
    fn worker_token_metadata_rejects_invalid_signed_values() {
        let claims = crate::auth::jwt::Claims {
            sub: "1".to_string(),
            login: "worker:invalid".to_string(),
            iat: 100,
            exp: 200,
            token_type: TokenType::Worker,
            scope: Some("internal_files".to_string()),
            metadata: Some(serde_json::json!({
                "worker_id": "not-a-database-id",
                "worker_instance": uuid::Uuid::new_v4(),
            })),
        };

        assert!(matches!(
            worker_token_metadata(&claims),
            Err(ApiError::Unauthorized(message))
                if message == "Worker token has invalid worker_id metadata"
        ));
    }

    #[test]
    fn test_refresh_request_uses_sensor_identity_state() {
        let claims = crate::auth::jwt::Claims {
            sub: "42".to_string(),
            login: "core.timer_sensor".to_string(),
            iat: 100,
            exp: 200,
            token_type: TokenType::Sensor,
            scope: Some("sensor".to_string()),
            metadata: Some(serde_json::json!({
                "trigger_types": ["malicious.override"],
            })),
        };
        let identity = sensor_identity(
            "core.timer_sensor",
            serde_json::json!(["core.intervaltimer", "core.crontimer"]),
        );

        let sensor_ref = sensor_ref_from_refresh_identity(&claims, &identity)
            .expect("refresh identity should resolve its registered sensor ref");
        assert_eq!(sensor_ref, "core.timer_sensor");
    }

    #[test]
    fn test_refresh_request_rejects_identity_login_mismatch() {
        let claims = crate::auth::jwt::Claims {
            sub: "42".to_string(),
            login: "core.timer_sensor".to_string(),
            iat: 100,
            exp: 200,
            token_type: TokenType::Sensor,
            scope: Some("sensor".to_string()),
            metadata: None,
        };
        let identity = sensor_identity(
            "core.other_sensor",
            serde_json::json!(["core.intervaltimer"]),
        );

        let result = sensor_ref_from_refresh_identity(&claims, &identity);
        assert!(result.is_err());
    }

    #[test]
    fn test_refresh_request_rejects_frozen_sensor_identity() {
        let claims = crate::auth::jwt::Claims {
            sub: "42".to_string(),
            login: "core.timer_sensor".to_string(),
            iat: 100,
            exp: 200,
            token_type: TokenType::Sensor,
            scope: Some("sensor".to_string()),
            metadata: None,
        };
        let mut identity = sensor_identity(
            "core.timer_sensor",
            serde_json::json!(["core.intervaltimer"]),
        );
        identity.frozen = true;

        let result = sensor_ref_from_refresh_identity(&claims, &identity);
        assert!(matches!(
            result,
            Err(ApiError::Forbidden(message))
                if message == "Identity is frozen and cannot authenticate"
        ));
    }

    #[test]
    fn test_frozen_identity_check_allows_active_identity() {
        let identity = sensor_identity(
            "core.timer_sensor",
            serde_json::json!(["core.intervaltimer"]),
        );

        let result = ensure_identity_not_frozen_for_authentication(&identity);
        assert!(result.is_ok());
    }

    #[test]
    fn test_frozen_identity_check_rejects_frozen_identity() {
        let mut identity = sensor_identity(
            "core.timer_sensor",
            serde_json::json!(["core.intervaltimer"]),
        );
        identity.frozen = true;

        let result = ensure_identity_not_frozen_for_authentication(&identity);
        assert!(matches!(
            result,
            Err(ApiError::Forbidden(message))
                if message == "Identity is frozen and cannot authenticate"
        ));
    }

    #[test]
    fn effective_permissions_response_preserves_constraints_granularity() {
        use attune_common::rbac::{
            Action as RbacAction, GrantConstraints, Resource as RbacResource,
        };

        let permissions = effective_permissions_response(vec![
            Grant {
                resource: RbacResource::Actions,
                actions: vec![RbacAction::Read],
                constraints: Some(GrantConstraints {
                    pack_refs: Some(vec!["core".to_string()]),
                    ..Default::default()
                }),
            },
            Grant {
                resource: RbacResource::Actions,
                actions: vec![RbacAction::Read],
                constraints: Some(GrantConstraints {
                    pack_refs: Some(vec!["ops".to_string()]),
                    ..Default::default()
                }),
            },
        ]);

        assert_eq!(permissions.len(), 2);
        assert_eq!(permissions[0].resource, "actions");
        assert_eq!(permissions[0].actions, vec!["read"]);
        assert_eq!(
            permissions[0].constraints,
            Some(serde_json::json!({"pack_refs": ["core"]}))
        );
        assert_eq!(
            permissions[1].constraints,
            Some(serde_json::json!({"pack_refs": ["ops"]}))
        );
    }

    #[test]
    fn effective_permissions_response_dedupes_identical_grants() {
        use attune_common::rbac::{Action as RbacAction, Resource as RbacResource};

        let permissions = effective_permissions_response(vec![
            Grant {
                resource: RbacResource::Queues,
                actions: vec![RbacAction::Read, RbacAction::Read],
                constraints: None,
            },
            Grant {
                resource: RbacResource::Queues,
                actions: vec![RbacAction::Read],
                constraints: None,
            },
        ]);

        assert_eq!(permissions.len(), 1);
        assert_eq!(permissions[0].resource, "queues");
        assert_eq!(permissions[0].actions, vec!["read"]);
        assert_eq!(permissions[0].constraints, None);
    }
}
