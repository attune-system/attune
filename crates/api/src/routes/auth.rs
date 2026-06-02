//! Authentication routes

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};

use validator::Validate;

use attune_common::auth::hash_integration_token;
use attune_common::models::{Identity, IntegrationToken};
use attune_common::rbac::{Action, Grant, Resource};
use attune_common::repositories::{
    identity::{
        CreateIdentityInput, IdentityRepository, IdentityRoleAssignmentRepository,
        PermissionSetRepository,
    },
    Create, FindById, IntegrationTokenRepository,
};

use crate::{
    auth::{
        hash_password,
        jwt::{
            generate_access_token, generate_integration_refresh_token, generate_refresh_token,
            generate_sensor_token, validate_token, TokenType,
        },
        middleware::RequireAuth,
        oidc::{
            apply_cookies_to_headers, build_login_redirect, build_logout_redirect,
            cookie_authenticated_user, get_cookie_value, has_oidc_session,
            oidc_callback_redirect_response, OidcCallbackQuery, REFRESH_COOKIE_NAME,
        },
        verify_password,
    },
    authz::AuthorizationService,
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
use std::collections::{BTreeMap, BTreeSet};
use utoipa::ToSchema;

/// Request body for creating sensor tokens
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateSensorTokenRequest {
    /// Sensor reference (e.g., "core.timer")
    #[validate(length(min = 1, max = 255))]
    pub sensor_ref: String,

    /// List of trigger types this sensor can create events for
    #[validate(length(min = 1))]
    pub trigger_types: Vec<String>,

    /// Optional TTL in seconds (default: 86400 = 24 hours, max: 259200 = 72 hours)
    #[validate(range(min = 3600, max = 259200))]
    pub ttl_seconds: Option<i64>,
}

/// Response for sensor token creation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SensorTokenResponse {
    pub identity_id: i64,
    pub sensor_ref: String,
    pub token: String,
    pub expires_at: String,
    pub trigger_types: Vec<String>,
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
        .route("/sensor-token", post(create_sensor_token))
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
    let mut by_resource: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for grant in grants {
        let resource = resource_name(grant.resource).to_string();
        let actions = by_resource.entry(resource).or_default();
        actions.extend(
            grant
                .actions
                .into_iter()
                .map(action_name)
                .map(str::to_string),
        );
    }

    by_resource
        .into_iter()
        .map(|(resource, actions)| EffectivePermissionResponse {
            resource,
            actions: actions.into_iter().collect(),
        })
        .collect()
}

fn resource_name(resource: Resource) -> &'static str {
    match resource {
        Resource::Packs => "packs",
        Resource::Actions => "actions",
        Resource::Queues => "queues",
        Resource::Rules => "rules",
        Resource::Triggers => "triggers",
        Resource::Executions => "executions",
        Resource::Events => "events",
        Resource::Enforcements => "enforcements",
        Resource::Inquiries => "inquiries",
        Resource::Keys => "keys",
        Resource::Artifacts => "artifacts",
        Resource::Runtimes => "runtimes",
        Resource::Workers => "workers",
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

    let grants = AuthorizationService::new(state.db.clone())
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
    let grants = AuthorizationService::new(state.db.clone())
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

/// Create sensor token endpoint (internal use by sensor service)
///
/// POST /auth/sensor-token
#[utoipa::path(
    post,
    path = "/auth/sensor-token",
    tag = "auth",
    request_body = CreateSensorTokenRequest,
    responses(
        (status = 200, description = "Sensor token created successfully", body = inline(ApiResponse<SensorTokenResponse>)),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
pub async fn create_sensor_token(
    State(state): State<SharedState>,
    RequireAuth(_user): RequireAuth,
    Json(payload): Json<CreateSensorTokenRequest>,
) -> Result<Json<ApiResponse<SensorTokenResponse>>, ApiError> {
    create_sensor_token_impl(state, payload).await
}

/// Create sensor token endpoint for internal service use (no auth required)
///
/// POST /auth/internal/sensor-token
///
/// This endpoint is intended for internal use by the sensor service to provision
/// tokens for standalone sensors. In production, this should be restricted by
/// network policies or replaced with proper service-to-service authentication.
#[utoipa::path(
    post,
    path = "/auth/internal/sensor-token",
    tag = "auth",
    request_body = CreateSensorTokenRequest,
    responses(
        (status = 200, description = "Sensor token created successfully", body = inline(ApiResponse<SensorTokenResponse>)),
        (status = 400, description = "Validation error")
    )
)]
pub async fn create_sensor_token_internal(
    State(state): State<SharedState>,
    Json(payload): Json<CreateSensorTokenRequest>,
) -> Result<Json<ApiResponse<SensorTokenResponse>>, ApiError> {
    create_sensor_token_impl(state, payload).await
}

/// Shared implementation for sensor token creation
async fn create_sensor_token_impl(
    state: SharedState,
    payload: CreateSensorTokenRequest,
) -> Result<Json<ApiResponse<SensorTokenResponse>>, ApiError> {
    // Validate request
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid sensor token request: {}", e)))?;

    // Create or find sensor identity
    let sensor_login = format!("sensor:{}", payload.sensor_ref);

    let identity = match IdentityRepository::find_by_login(&state.db, &sensor_login).await? {
        Some(identity) => identity,
        None => {
            // Create new sensor identity
            let input = CreateIdentityInput {
                login: sensor_login.clone(),
                display_name: Some(format!("Sensor: {}", payload.sensor_ref)),
                password_hash: None, // Sensors don't use passwords
                attributes: serde_json::json!({
                    "type": "sensor",
                    "sensor_ref": payload.sensor_ref,
                    "trigger_types": payload.trigger_types,
                }),
            };
            IdentityRepository::create(&state.db, input).await?
        }
    };

    // Generate sensor token
    let ttl_seconds = payload.ttl_seconds.unwrap_or(86400); // Default: 24 hours
    let token = generate_sensor_token(
        identity.id,
        &payload.sensor_ref,
        payload.trigger_types.clone(),
        &state.jwt_config,
        Some(ttl_seconds),
    )?;

    // Calculate expiration time
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds);

    let response = SensorTokenResponse {
        identity_id: identity.id,
        sensor_ref: payload.sensor_ref,
        token,
        expires_at: expires_at.to_rfc3339(),
        trigger_types: payload.trigger_types,
    };

    Ok(Json(ApiResponse::new(response)))
}
