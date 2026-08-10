//! JWT token generation and validation
//!
//! Shared across all Attune services. Token types:
//! - **Access**: Standard user login tokens (1h default)
//! - **Refresh**: Long-lived refresh tokens (7d default)
//! - **Sensor**: Sensor service tokens with trigger type metadata (24h default)
//! - **Execution**: Short-lived tokens scoped to a single execution (matching execution timeout)

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rbac::Grant;

pub const STANDARD_EXECUTION_ACCESS_REF: &str = "standard";

#[derive(Debug, Error)]
pub enum JwtError {
    #[error("Failed to encode JWT: {0}")]
    EncodeError(String),
    #[error("Failed to decode JWT: {0}")]
    DecodeError(String),
    #[error("Token has expired")]
    Expired,
    #[error("Invalid token")]
    Invalid,
}

/// JWT Claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (identity ID)
    pub sub: String,
    /// Identity login (or descriptor like "execution:123")
    pub login: String,
    /// Issued at (Unix timestamp)
    pub iat: i64,
    /// Expiration time (Unix timestamp)
    pub exp: i64,
    /// Token type (access, refresh, sensor, or execution)
    #[serde(default)]
    pub token_type: TokenType,
    /// Optional scope (e.g., "sensor", "execution")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Optional metadata (e.g., trigger_types for sensors, execution_id for execution tokens)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TokenType {
    #[default]
    Access,
    Refresh,
    Sensor,
    Execution,
    /// Long-lived token for internal worker/sensor → API file operations.
    Worker,
}

/// Configuration for JWT tokens
#[derive(Debug, Clone)]
pub struct JwtConfig {
    /// Secret key for signing tokens
    pub secret: String,
    /// Access token expiration duration (in seconds)
    pub access_token_expiration: i64,
    /// Refresh token expiration duration (in seconds)
    pub refresh_token_expiration: i64,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret: "insecure_default_secret_change_in_production".to_string(),
            access_token_expiration: 3600,    // 1 hour
            refresh_token_expiration: 604800, // 7 days
        }
    }
}

/// Generate a JWT access token
pub fn generate_access_token(
    identity_id: i64,
    login: &str,
    config: &JwtConfig,
) -> Result<String, JwtError> {
    generate_token(identity_id, login, config, TokenType::Access)
}

/// Generate a JWT refresh token
pub fn generate_refresh_token(
    identity_id: i64,
    login: &str,
    config: &JwtConfig,
) -> Result<String, JwtError> {
    generate_token(identity_id, login, config, TokenType::Refresh)
}

/// Generate a revokable refresh JWT backed by an integration token record.
///
/// The refresh token subject is the `integration_token.id`. The API refresh
/// route resolves that record to an identity and fails closed after revocation,
/// expiry, deletion, or identity freeze. Access JWTs minted from this refresh
/// flow continue to use the identity ID as their subject.
pub fn generate_integration_refresh_token(
    integration_token_id: i64,
    identity_id: i64,
    login: &str,
    config: &JwtConfig,
) -> Result<String, JwtError> {
    let now = Utc::now();
    let exp = (now + Duration::seconds(config.refresh_token_expiration)).timestamp();

    let claims = Claims {
        sub: integration_token_id.to_string(),
        login: login.to_string(),
        iat: now.timestamp(),
        exp,
        token_type: TokenType::Refresh,
        scope: Some("integration_token".to_string()),
        metadata: Some(serde_json::json!({
            "identity_id": identity_id,
            "integration_token_id": integration_token_id,
            "auth_method": "integration_token",
        })),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|e| JwtError::EncodeError(e.to_string()))
}

/// Generate a JWT token with a specific type
pub fn generate_token(
    identity_id: i64,
    login: &str,
    config: &JwtConfig,
    token_type: TokenType,
) -> Result<String, JwtError> {
    let now = Utc::now();
    let expiration = match token_type {
        TokenType::Access => config.access_token_expiration,
        TokenType::Refresh => config.refresh_token_expiration,
        // Sensor and Execution tokens are generated via their own dedicated functions
        // with explicit TTLs; this fallback should not normally be reached.
        TokenType::Sensor => 86400,
        TokenType::Execution => 300,
        TokenType::Worker => 86400, // 24h, renewed on heartbeat
    };

    let exp = (now + Duration::seconds(expiration)).timestamp();

    let claims = Claims {
        sub: identity_id.to_string(),
        login: login.to_string(),
        iat: now.timestamp(),
        exp,
        token_type,
        scope: None,
        metadata: None,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|e| JwtError::EncodeError(e.to_string()))
}

/// Generate a sensor token with specific trigger types
///
/// # Arguments
/// * `identity_id` - The identity ID for the sensor
/// * `sensor_ref` - The sensor reference (e.g., "sensor:core.timer")
/// * `trigger_types` - List of trigger types this sensor can create events for
/// * `config` - JWT configuration
/// * `ttl_seconds` - Time to live in seconds (default: 24 hours)
pub fn generate_sensor_token(
    identity_id: i64,
    sensor_ref: &str,
    trigger_types: Vec<String>,
    config: &JwtConfig,
    ttl_seconds: Option<i64>,
) -> Result<String, JwtError> {
    generate_sensor_token_with_cache_authority(
        identity_id,
        sensor_ref,
        trigger_types,
        None,
        &[],
        &[],
        config,
        ttl_seconds,
    )
}

/// Generate a sensor token with an exact signed pack/cache authority snapshot.
///
/// Cache routes never infer a sensor identity's ordinary RBAC roles. They use
/// only the `cache_grants` embedded here by the API after resolving the
/// registered sensor and its explicit permission-set configuration.
#[allow(clippy::too_many_arguments)]
pub fn generate_sensor_token_with_cache_authority(
    identity_id: i64,
    sensor_ref: &str,
    trigger_types: Vec<String>,
    pack_ref: Option<&str>,
    permission_set_refs: &[String],
    cache_grants: &[Grant],
    config: &JwtConfig,
    ttl_seconds: Option<i64>,
) -> Result<String, JwtError> {
    let now = Utc::now();
    let expiration = ttl_seconds.unwrap_or(86400); // Default: 24 hours
    let exp = (now + Duration::seconds(expiration)).timestamp();

    let metadata = serde_json::json!({
        "trigger_types": trigger_types,
        "sensor_ref": sensor_ref,
        "pack_ref": pack_ref,
        "permission_set_refs": permission_set_refs,
        "cache_grants": cache_grants,
    });

    let claims = Claims {
        sub: identity_id.to_string(),
        login: sensor_ref.to_string(),
        iat: now.timestamp(),
        exp,
        token_type: TokenType::Sensor,
        scope: Some("sensor".to_string()),
        metadata: Some(metadata),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|e| JwtError::EncodeError(e.to_string()))
}

/// Generate a worker/sensor-service token for internal file operations.
///
/// These long-lived tokens allow workers and sensors to upload/download
/// artifact files via the API when they don't share a filesystem volume.
///
/// # Arguments
/// * `identity_id` - The system identity ID (typically 1)
/// * `worker_id` - A human-readable worker identifier
/// * `config` - JWT configuration
/// * `ttl_seconds` - Time to live in seconds (default: 24 hours)
pub fn generate_worker_token(
    identity_id: i64,
    worker_id: &str,
    config: &JwtConfig,
    ttl_seconds: Option<i64>,
) -> Result<String, JwtError> {
    let now = Utc::now();
    let expiration = ttl_seconds.unwrap_or(86400);
    let exp = (now + Duration::seconds(expiration)).timestamp();

    let metadata = serde_json::json!({
        "worker_id": worker_id,
    });

    let claims = Claims {
        sub: identity_id.to_string(),
        login: format!("worker:{}", worker_id),
        iat: now.timestamp(),
        exp,
        token_type: TokenType::Worker,
        scope: Some("internal_files".to_string()),
        metadata: Some(metadata),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|e| JwtError::EncodeError(e.to_string()))
}

/// Generate an execution-scoped token.
///
/// These tokens are short-lived (matching the execution timeout) and scoped
/// to a single execution. They allow actions to call back into the Attune API
/// (e.g., to create artifacts, update progress) without full user credentials.
///
/// The token is automatically invalidated when it expires. The TTL defaults to
/// the execution timeout plus a 60-second grace period to account for cleanup.
///
/// # Arguments
/// * `identity_id` - The identity ID that triggered the execution
/// * `execution_id` - The execution ID this token is scoped to
/// * `action_ref` - The action reference for audit/logging
/// * `config` - JWT configuration (uses the same signing secret as all tokens)
/// * `ttl_seconds` - Time to live in seconds (defaults to 360 = 5 min timeout + 60s grace)
pub fn generate_execution_token(
    identity_id: i64,
    execution_id: i64,
    action_ref: &str,
    config: &JwtConfig,
    ttl_seconds: Option<i64>,
) -> Result<String, JwtError> {
    generate_execution_token_with_permission_sets(
        identity_id,
        execution_id,
        action_ref,
        config,
        ttl_seconds,
        &[],
    )
}

/// Generate an execution-scoped token constrained to explicit permission sets.
///
/// Execution tokens must not infer permissions from the triggering identity's
/// roles. The listed permission set refs are embedded in token metadata and
/// become the token's complete effective RBAC surface.
pub fn generate_execution_token_with_permission_sets(
    identity_id: i64,
    execution_id: i64,
    action_ref: &str,
    config: &JwtConfig,
    ttl_seconds: Option<i64>,
    permission_set_refs: &[String],
) -> Result<String, JwtError> {
    generate_execution_token_with_permission_sets_and_standard_access(
        identity_id,
        execution_id,
        action_ref,
        config,
        ttl_seconds,
        permission_set_refs,
        &[],
    )
}

/// Generate an execution-scoped token with explicit permission sets and
/// standard action/pack access context.
///
/// `standard_access_action_refs` is meaningful only when
/// [`STANDARD_EXECUTION_ACCESS_REF`] is present in `permission_set_refs`.
pub fn generate_execution_token_with_permission_sets_and_standard_access(
    identity_id: i64,
    execution_id: i64,
    action_ref: &str,
    config: &JwtConfig,
    ttl_seconds: Option<i64>,
    permission_set_refs: &[String],
    standard_access_action_refs: &[String],
) -> Result<String, JwtError> {
    let now = Utc::now();
    let expiration = ttl_seconds.unwrap_or(360); // Default: 6 minutes (5 min timeout + grace)
    let exp = (now + Duration::seconds(expiration)).timestamp();

    let standard_access_pack_refs = standard_access_action_refs
        .iter()
        .filter_map(|action_ref| action_ref.split_once('.').map(|(pack_ref, _)| pack_ref))
        .map(str::to_string)
        .collect::<Vec<_>>();

    let metadata = serde_json::json!({
        "execution_id": execution_id,
        "action_ref": action_ref,
        "permission_set_refs": permission_set_refs,
        "standard_access_action_refs": standard_access_action_refs,
        "standard_access_pack_refs": standard_access_pack_refs,
    });

    let claims = Claims {
        sub: identity_id.to_string(),
        login: format!("execution:{}", execution_id),
        iat: now.timestamp(),
        exp,
        token_type: TokenType::Execution,
        scope: Some("execution".to_string()),
        metadata: Some(metadata),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|e| JwtError::EncodeError(e.to_string()))
}

/// Validate and decode a JWT token
pub fn validate_token(token: &str, config: &JwtConfig) -> Result<Claims, JwtError> {
    let mut validation = Validation::default();
    validation.algorithms = vec![jsonwebtoken::Algorithm::HS256];

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|e| {
        if e.to_string().contains("ExpiredSignature") {
            JwtError::Expired
        } else {
            JwtError::DecodeError(e.to_string())
        }
    })
}

/// Extract token from Authorization header
pub fn extract_token_from_header(auth_header: &str) -> Option<&str> {
    auth_header.strip_prefix("Bearer ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::crypto_provider;

    fn test_config() -> JwtConfig {
        crypto_provider::install();
        JwtConfig {
            secret: "test_secret_key_for_testing".to_string(),
            access_token_expiration: 3600,
            refresh_token_expiration: 604800,
        }
    }

    #[test]
    fn test_generate_and_validate_access_token() {
        let config = test_config();

        let token =
            generate_access_token(123, "testuser", &config).expect("Failed to generate token");

        let claims = validate_token(&token, &config).expect("Failed to validate token");

        assert_eq!(claims.sub, "123");
        assert_eq!(claims.login, "testuser");
        assert_eq!(claims.token_type, TokenType::Access);
    }

    #[test]
    fn test_generate_and_validate_refresh_token() {
        let config = test_config();
        let token =
            generate_refresh_token(456, "anotheruser", &config).expect("Failed to generate token");

        let claims = validate_token(&token, &config).expect("Failed to validate token");

        assert_eq!(claims.sub, "456");
        assert_eq!(claims.login, "anotheruser");
        assert_eq!(claims.token_type, TokenType::Refresh);
    }

    #[test]
    fn test_invalid_token() {
        let config = test_config();
        let result = validate_token("invalid.token.here", &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_token_with_wrong_secret() {
        let config = test_config();

        let token = generate_access_token(789, "user", &config).expect("Failed to generate token");

        let wrong_config = JwtConfig {
            secret: "different_secret".to_string(),
            ..config
        };

        let result = validate_token(&token, &wrong_config);
        assert!(result.is_err());
    }

    #[test]
    fn test_expired_token() {
        crypto_provider::install();
        let now = Utc::now().timestamp();
        let expired_claims = Claims {
            sub: "999".to_string(),
            login: "expireduser".to_string(),
            iat: now - 3600,
            exp: now - 1800,
            token_type: TokenType::Access,
            scope: None,
            metadata: None,
        };

        let config = test_config();

        let expired_token = encode(
            &Header::default(),
            &expired_claims,
            &EncodingKey::from_secret(config.secret.as_bytes()),
        )
        .expect("Failed to encode token");

        let result = validate_token(&expired_token, &config);
        assert!(matches!(result, Err(JwtError::Expired)));
    }

    #[test]
    fn test_extract_token_from_header() {
        let header = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let token = extract_token_from_header(header);
        assert_eq!(token, Some("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));

        let invalid_header = "Token abc123";
        let token = extract_token_from_header(invalid_header);
        assert_eq!(token, None);

        let no_token = "Bearer ";
        let token = extract_token_from_header(no_token);
        assert_eq!(token, Some(""));
    }

    #[test]
    fn test_claims_serialization() {
        let claims = Claims {
            sub: "123".to_string(),
            login: "testuser".to_string(),
            iat: 1234567890,
            exp: 1234571490,
            token_type: TokenType::Access,
            scope: None,
            metadata: None,
        };

        let json = serde_json::to_string(&claims).expect("Failed to serialize");
        let deserialized: Claims = serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(claims.sub, deserialized.sub);
        assert_eq!(claims.login, deserialized.login);
        assert_eq!(claims.token_type, deserialized.token_type);
    }

    #[test]
    fn test_generate_sensor_token() {
        let config = test_config();
        let trigger_types = vec!["core.timer".to_string(), "core.webhook".to_string()];

        let token = generate_sensor_token(
            999,
            "sensor:core.timer",
            trigger_types.clone(),
            &config,
            Some(86400),
        )
        .expect("Failed to generate sensor token");

        let claims = validate_token(&token, &config).expect("Failed to validate token");

        assert_eq!(claims.sub, "999");
        assert_eq!(claims.login, "sensor:core.timer");
        assert_eq!(claims.token_type, TokenType::Sensor);
        assert_eq!(claims.scope, Some("sensor".to_string()));

        let metadata = claims.metadata.expect("Metadata should be present");
        let trigger_types_from_token = metadata["trigger_types"]
            .as_array()
            .expect("trigger_types should be an array");

        assert_eq!(trigger_types_from_token.len(), 2);
    }

    #[test]
    fn test_sensor_cache_authority_is_signed_into_metadata() {
        let config = test_config();
        let grants = vec![Grant {
            resource: crate::rbac::Resource::Caches,
            actions: vec![crate::rbac::Action::Read],
            constraints: Some(crate::rbac::GrantConstraints {
                owner_types: Some(vec![crate::models::OwnerType::Pack]),
                owner_refs: Some(vec!["salesforce".to_string()]),
                ..Default::default()
            }),
        }];
        let permission_set_refs = vec!["standard".to_string()];
        let token = generate_sensor_token_with_cache_authority(
            999,
            "salesforce.reader",
            vec!["salesforce.updated".to_string()],
            Some("salesforce"),
            &permission_set_refs,
            &grants,
            &config,
            Some(3600),
        )
        .expect("generate sensor token");

        let metadata = validate_token(&token, &config)
            .expect("validate sensor token")
            .metadata
            .expect("sensor metadata");
        assert_eq!(metadata["sensor_ref"], "salesforce.reader");
        assert_eq!(metadata["pack_ref"], "salesforce");
        assert_eq!(
            metadata["permission_set_refs"],
            serde_json::json!(["standard"])
        );
        assert_eq!(metadata["cache_grants"][0]["resource"], "caches");
        assert_eq!(
            metadata["cache_grants"][0]["actions"],
            serde_json::json!(["read"])
        );
    }

    #[test]
    fn test_generate_execution_token() {
        let config = test_config();

        let token =
            generate_execution_token(42, 12345, "python_example.artifact_demo", &config, None)
                .expect("Failed to generate execution token");

        let claims = validate_token(&token, &config).expect("Failed to validate token");

        assert_eq!(claims.sub, "42");
        assert_eq!(claims.login, "execution:12345");
        assert_eq!(claims.token_type, TokenType::Execution);
        assert_eq!(claims.scope, Some("execution".to_string()));

        let metadata = claims.metadata.expect("Metadata should be present");
        assert_eq!(metadata["execution_id"], 12345);
        assert_eq!(metadata["action_ref"], "python_example.artifact_demo");
    }

    #[test]
    fn test_execution_token_custom_ttl() {
        let config = test_config();

        let token = generate_execution_token(1, 100, "core.echo", &config, Some(600))
            .expect("Failed to generate execution token");

        let claims = validate_token(&token, &config).expect("Failed to validate token");

        // Should expire roughly 600 seconds from now
        let now = Utc::now().timestamp();
        let diff = claims.exp - now;
        assert!(
            diff > 590 && diff <= 600,
            "TTL should be ~600s, got {}s",
            diff
        );
    }

    #[test]
    fn test_generate_execution_token_with_permission_sets() {
        let config = test_config();
        let refs = vec![
            "core.agent_reader".to_string(),
            "core.agent_writer".to_string(),
        ];

        let token = generate_execution_token_with_permission_sets(
            1,
            100,
            "core.agent",
            &config,
            Some(600),
            &refs,
        )
        .expect("Failed to generate execution token");

        let claims = validate_token(&token, &config).expect("Failed to validate token");
        assert_eq!(claims.token_type, TokenType::Execution);
        assert_eq!(
            claims.metadata.expect("metadata")["permission_set_refs"],
            serde_json::json!(refs)
        );
    }

    #[test]
    fn test_generate_execution_token_with_standard_access_context() {
        let config = test_config();
        let refs = vec![STANDARD_EXECUTION_ACCESS_REF.to_string()];
        let standard_action_refs = vec!["sql.query".to_string(), "workflow_pack.sync".to_string()];

        let token = generate_execution_token_with_permission_sets_and_standard_access(
            1,
            100,
            "sql.query",
            &config,
            Some(600),
            &refs,
            &standard_action_refs,
        )
        .expect("Failed to generate execution token");

        let claims = validate_token(&token, &config).expect("Failed to validate token");
        let metadata = claims.metadata.expect("metadata");
        assert_eq!(metadata["permission_set_refs"], serde_json::json!(refs));
        assert_eq!(
            metadata["standard_access_action_refs"],
            serde_json::json!(standard_action_refs)
        );
        assert_eq!(
            metadata["standard_access_pack_refs"],
            serde_json::json!(["sql", "workflow_pack"])
        );
    }

    #[test]
    fn test_token_type_serialization() {
        // Ensure all token types round-trip through JSON correctly
        for tt in [
            TokenType::Access,
            TokenType::Refresh,
            TokenType::Sensor,
            TokenType::Execution,
        ] {
            let json = serde_json::to_string(&tt).expect("Failed to serialize");
            let deserialized: TokenType =
                serde_json::from_str(&json).expect("Failed to deserialize");
            assert_eq!(tt, deserialized);
        }
    }
}
