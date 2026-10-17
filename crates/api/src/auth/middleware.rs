//! Authentication middleware for protecting routes

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

use attune_common::auth::jwt::{
    extract_token_from_header, validate_token, Claims, JwtConfig, JwtError, TokenType,
};
use attune_common::models::SensorWorkloadFence;
use attune_common::repositories::SensorWorkloadRepository;

use super::oidc::{cookie_authenticated_user, ACCESS_COOKIE_NAME};

/// Authentication middleware state
#[derive(Clone)]
pub struct AuthMiddleware {
    pub jwt_config: Arc<JwtConfig>,
}

impl AuthMiddleware {
    pub fn new(jwt_config: JwtConfig) -> Self {
        Self {
            jwt_config: Arc::new(jwt_config),
        }
    }
}

/// Extension type for storing authenticated claims in request
#[derive(Clone, Debug)]
pub struct AuthenticatedUser {
    pub claims: Claims,
}

impl AuthenticatedUser {
    /// Order-only fence for routes with their own parent-protection protocol.
    /// Taking this lock grants no authority. The route must still validate its
    /// recorded source on the same connection before accessing any target.
    pub async fn lock_recorded_capture_admission(
        &self,
        connection: &mut sqlx::PgConnection,
    ) -> attune_common::Result<()> {
        let binding = self.claims.admitted_execution_credential().map_err(|_| {
            attune_common::Error::PermissionDenied("Invalid recorded execution credential".into())
        })?;
        if let Some(binding) = binding {
            attune_common::repositories::runtime_read::RuntimeReadRepository::lock_open(
                connection,
                binding.admission_id,
            )
            .await?;
        }
        Ok(())
    }

    /// Descriptive IDs for an already authenticated request, never a permit.
    /// Do not copy credentials, policy bodies or input values into audit events.
    pub fn with_recorded_audit_context(&self, details: serde_json::Value) -> serde_json::Value {
        let Ok(Some(binding)) = self.claims.admitted_execution_credential() else {
            return details;
        };
        let mut details = match details {
            serde_json::Value::Object(details) => details,
            value => serde_json::Map::from_iter([("operation_details".into(), value)]),
        };
        details.insert("runtime_authority".into(), serde_json::json!({
            "authority_source":"pack", "execution_id":binding.execution_id,
            "admission_id":binding.admission_id, "sponsor_identity_id":binding.sponsor_identity_id,
            "attempt":binding.attempt,
        }));
        serde_json::Value::Object(details)
    }

    pub async fn recorded_runtime_authority(
        &self,
        connection: &mut sqlx::PgConnection,
    ) -> attune_common::Result<
        Option<attune_common::repositories::runtime_authority::ValidatedRuntimeAuthority>,
    > {
        let Some(authority) = self.validate_recorded_runtime_authority(connection).await? else {
            return Ok(None);
        };
        // Acquire before route targets, and after canonical source validation.
        attune_common::repositories::runtime_read::RuntimeReadRepository::lock_open(
            connection,
            authority.credential_binding().admission_id,
        )
        .await?;
        authority.ensure_fresh_on(connection).await?;
        Ok(Some(authority))
    }

    /// Live acquisition orders both capture fences after canonical source
    /// validation. Taking capture before source would invert ordinary SDK reads.
    pub(crate) async fn validate_recorded_runtime_authority(
        &self,
        connection: &mut sqlx::PgConnection,
    ) -> attune_common::Result<
        Option<attune_common::repositories::runtime_authority::ValidatedRuntimeAuthority>,
    > {
        let credential = self.claims.admitted_execution_credential().map_err(|_| {
            attune_common::Error::PermissionDenied("Invalid recorded execution credential".into())
        })?;
        let Some(credential) = credential else {
            return Ok(None);
        };
        let selected = self.claims.admitted_permission_set_refs().map_err(|_| {
            attune_common::Error::PermissionDenied("Invalid recorded execution selection".into())
        })?;
        let authority = attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::validate_execution_credential(
            connection, &credential,
        ).await?;
        if authority.authority().selected_refs != selected {
            return Err(attune_common::Error::PermissionDenied(
                "Execution credential selection differs from admitted authority".into(),
            ));
        }
        Ok(Some(authority))
    }

    pub(crate) fn from_claims(claims: Claims) -> Self {
        Self { claims }
    }

    pub fn identity_id(&self) -> Result<i64, std::num::ParseIntError> {
        self.claims.sub.parse()
    }

    pub fn login(&self) -> &str {
        &self.claims.login
    }

    /// Returns the execution ID this token is scoped to, if any.
    ///
    /// Only `Execution` tokens carry an execution scope. The execution ID is
    /// stored in the `metadata.execution_id` claim. Returns `None` for all
    /// other token types or when the metadata is missing/malformed.
    pub fn execution_id(&self) -> Option<i64> {
        if self.claims.token_type != TokenType::Execution {
            return None;
        }
        self.claims
            .metadata
            .as_ref()?
            .get("execution_id")
            .and_then(|v| v.as_i64())
    }

    /// Returns trigger refs this sensor token is scoped to.
    ///
    /// Non-sensor tokens always return an empty list.
    pub fn sensor_trigger_types(&self) -> Vec<String> {
        if self.claims.token_type != TokenType::Sensor {
            return Vec::new();
        }

        self.claims
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("trigger_types"))
            .and_then(|value| value.as_array())
            .map(|trigger_types| {
                trigger_types
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn sensor_workload_fence(&self) -> Result<SensorWorkloadFence, JwtError> {
        self.claims.sensor_workload_fence()
    }
}

/// Middleware function that validates JWT tokens
pub async fn require_auth(
    State(auth): State<AuthMiddleware>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let claims = extract_claims(request.headers(), &auth.jwt_config)?;

    // Add claims to request extensions
    request
        .extensions_mut()
        .insert(AuthenticatedUser::from_claims(claims));

    // Continue to next middleware/handler
    Ok(next.run(request).await)
}

/// Protected-route extractor. The default remains JWT/user authentication.
/// Purpose-specific contexts must supply their own authenticated extractor;
/// they cannot manufacture or fall back to an AuthenticatedUser.
pub struct RequireAuth<T = AuthenticatedUser>(pub T);

/// Trusted route marker. A recorded token must not fall through an unconverted
/// consumer that relies only on its token type or mutable human IAM.
#[derive(Clone, Copy)]
pub(crate) enum RecordedRuntimeConsumer {
    ExecutionStream,
    ExecutionCreate,
    ExecutionMetadata,
    ExecutionControl,
    QueueOperations,
    EventIngress,
    KeyRead,
    KeyList,
    KeyUpdate,
    KeyDelete,
    KeyCreate,
    SchemaMetadata,
    CacheRead,
    CacheDataMutation,
    KeySigning,
    ArtifactMetadataRead,
    ArtifactVersionRead,
    ArtifactCreate,
    ArtifactDataMutation,
    ArtifactMetadataMutation,
}

impl axum::extract::FromRequestParts<crate::state::SharedState> for RequireAuth<AuthenticatedUser> {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::state::SharedState,
    ) -> Result<Self, Self::Rejection> {
        let claims = if let Some(user) = parts.extensions.get::<AuthenticatedUser>() {
            user.claims.clone()
        } else if let Some(user) =
            cookie_authenticated_user(&parts.headers, state).map_err(map_cookie_auth_error)?
        {
            user.claims
        } else {
            extract_claims(&parts.headers, &state.jwt_config)?
        };

        // Allow access, sensor, execution, and worker tokens
        if claims.scope.as_deref() == Some("worker_delivery") {
            attune_common::worker_delivery::credential(&claims)
                .map_err(|_| AuthError::InvalidToken)?;
            if parts
                .extensions
                .get::<crate::recorded_files::WorkerDeliveryConsumer>()
                .is_none()
            {
                return Err(AuthError::Unauthorized);
            }
        }
        if claims.token_type != TokenType::Access
            && claims.token_type != TokenType::Sensor
            && claims.token_type != TokenType::Execution
            && claims.token_type != TokenType::Worker
        {
            return Err(AuthError::InvalidToken);
        }

        if claims.token_type == TokenType::Sensor {
            let fence = claims
                .sensor_workload_fence()
                .map_err(|_| AuthError::InvalidToken)?;
            let current = match (claims.sensor_ref(), claims.sensor_pack_ref()) {
                (Ok(sensor_ref), Ok(pack_ref)) => {
                    SensorWorkloadRepository::is_current_fence_for_sensor_scope(
                        &state.db, fence, sensor_ref, pack_ref,
                    )
                    .await
                }
                _ => SensorWorkloadRepository::is_current_fence(&state.db, fence).await,
            }
            .map_err(|error| {
                tracing::error!(%error, "Failed to validate sensor workload fence");
                AuthError::Internal
            })?;
            if !current {
                return Err(AuthError::InvalidToken);
            }
        }

        let user = AuthenticatedUser::from_claims(claims);
        if user
            .claims
            .admitted_execution_credential()
            .map_err(|_| AuthError::InvalidToken)?
            .is_some()
        {
            let mut transaction = state.db.begin().await.map_err(|_| AuthError::Internal)?;
            user.recorded_runtime_authority(&mut transaction)
                .await
                .map_err(|error| match error {
                    attune_common::Error::Database(_) => AuthError::Internal,
                    _ => AuthError::InvalidToken,
                })?;
            transaction
                .commit()
                .await
                .map_err(|_| AuthError::Internal)?;
            if parts.extensions.get::<RecordedRuntimeConsumer>().is_none() {
                return Err(AuthError::Unauthorized);
            }
        }
        Ok(RequireAuth(user))
    }
}

fn extract_claims(headers: &HeaderMap, jwt_config: &JwtConfig) -> Result<Claims, AuthError> {
    if let Some(auth_header) = headers.get(AUTHORIZATION).and_then(|h| h.to_str().ok()) {
        let token = extract_token_from_header(auth_header).ok_or(AuthError::InvalidToken)?;
        return validate_token(token, jwt_config).map_err(|e| match e {
            super::jwt::JwtError::Expired => AuthError::ExpiredToken,
            _ => AuthError::InvalidToken,
        });
    }

    if headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|cookies| cookies.contains(ACCESS_COOKIE_NAME))
    {
        return Err(AuthError::InvalidToken);
    }

    Err(AuthError::MissingToken)
}

fn map_cookie_auth_error(error: crate::middleware::error::ApiError) -> AuthError {
    match error {
        crate::middleware::error::ApiError::Unauthorized(_) => AuthError::InvalidToken,
        _ => AuthError::InvalidToken,
    }
}

/// Authentication errors
#[derive(Debug)]
pub enum AuthError {
    MissingToken,
    InvalidToken,
    ExpiredToken,
    Unauthorized,
    Internal,
}

/// Error details returned when authentication fails before a route handler runs.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthErrorDetail {
    pub code: u16,
    pub message: String,
}

/// Authentication rejection envelope.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthErrorResponse {
    pub error: AuthErrorDetail,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::MissingToken => (StatusCode::UNAUTHORIZED, "Missing authentication token"),
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid authentication token"),
            AuthError::ExpiredToken => (StatusCode::UNAUTHORIZED, "Authentication token expired"),
            AuthError::Unauthorized => (StatusCode::FORBIDDEN, "Insufficient permissions"),
            AuthError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Authentication service unavailable",
            ),
        };

        let body = Json(AuthErrorResponse {
            error: AuthErrorDetail {
                code: status.as_u16(),
                message: message.to_string(),
            },
        });

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_audit_context_contains_binding_ids_without_copying_credential_metadata() {
        let attempt = uuid::Uuid::new_v4();
        let user = AuthenticatedUser::from_claims(Claims {
            sub: "33".into(),
            login: "execution:11".into(),
            iat: 1,
            exp: 2,
            token_type: TokenType::Execution,
            scope: Some("execution".into()),
            metadata: Some(
                serde_json::json!({"authority_source":"pack", "execution_id":11,
                "action_ref":"private.action", "permission_set_refs":["private.policy"],
                "unrelated_token_secret":"not-for-audit", "runtime_authority":{
                    "execution_id":11,"admission_id":22,"sponsor_identity_id":33,
                    "action_ref":"private.action","attempt":attempt
                }}),
            ),
        });
        let details = user.with_recorded_audit_context(
            serde_json::json!({"operation":"sign_jwt", "runtime_authority":{"forged":true}}),
        );
        assert_eq!(
            details,
            serde_json::json!({"operation":"sign_jwt", "runtime_authority":{
                "authority_source":"pack", "execution_id":11,"admission_id":22,"sponsor_identity_id":33,"attempt":attempt
            }})
        );
        assert!(!details.to_string().contains("private"));
        assert!(!details.to_string().contains("not-for-audit"));
    }

    #[test]
    fn test_authenticated_user() {
        let claims = Claims {
            sub: "123".to_string(),
            login: "testuser".to_string(),
            iat: 1234567890,
            exp: 1234571490,
            token_type: TokenType::Access,
            scope: None,
            metadata: None,
        };

        let auth_user = AuthenticatedUser::from_claims(claims);

        assert_eq!(auth_user.identity_id().unwrap(), 123);
        assert_eq!(auth_user.login(), "testuser");
    }

    #[test]
    fn test_extract_token_from_header() {
        let token = extract_token_from_header("Bearer test.token.here");
        assert_eq!(token, Some("test.token.here"));

        let no_bearer = extract_token_from_header("test.token.here");
        assert_eq!(no_bearer, None);
    }

    #[test]
    fn sensor_trigger_types_returns_scoped_refs_for_sensor_tokens() {
        let auth_user = AuthenticatedUser {
            claims: Claims {
                sub: "1".to_string(),
                login: "sensor.demo".to_string(),
                iat: 1,
                exp: 2,
                token_type: TokenType::Sensor,
                scope: Some("sensor".to_string()),
                metadata: Some(serde_json::json!({
                    "trigger_types": [" core.timer ", "", "core.webhook"]
                })),
            },
        };

        assert_eq!(
            auth_user.sensor_trigger_types(),
            vec!["core.timer".to_string(), "core.webhook".to_string()]
        );
    }

    #[test]
    fn sensor_trigger_types_returns_empty_for_non_sensor_tokens() {
        let auth_user = AuthenticatedUser {
            claims: Claims {
                sub: "1".to_string(),
                login: "testuser".to_string(),
                iat: 1,
                exp: 2,
                token_type: TokenType::Access,
                scope: None,
                metadata: Some(serde_json::json!({
                    "trigger_types": ["core.timer"]
                })),
            },
        };

        assert!(auth_user.sensor_trigger_types().is_empty());
    }

    #[test]
    fn sensor_workload_fence_delegates_to_claims() {
        let worker_instance = uuid::Uuid::new_v4();
        let auth_user = AuthenticatedUser {
            claims: Claims {
                sub: "1".to_string(),
                login: "sensor.demo".to_string(),
                iat: 1,
                exp: 2,
                token_type: TokenType::Sensor,
                scope: Some("sensor".to_string()),
                metadata: Some(serde_json::json!({
                    "workload_fence": {
                        "workload_id": 11,
                        "worker_id": 22,
                        "worker_instance": worker_instance,
                        "generation": 3
                    }
                })),
            },
        };

        assert_eq!(
            auth_user.sensor_workload_fence().unwrap(),
            SensorWorkloadFence {
                workload_id: 11,
                worker_id: 22,
                worker_instance,
                generation: 3,
            }
        );
    }
}
