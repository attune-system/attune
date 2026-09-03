use chrono::Utc;
use std::sync::Mutex;
use uuid::Uuid;

use super::jwt::{generate_worker_token_with_instance, JwtConfig, JwtError};

const DEFAULT_WORKER_TOKEN_TTL_SECONDS: i64 = 86_400;
const DEFAULT_WORKER_TOKEN_REFRESH_BEFORE_SECONDS: i64 = 300;

#[derive(Debug, Clone)]
struct WorkerTokenState {
    token: String,
    refresh_at_unix: i64,
}

#[derive(Debug)]
struct WorkerTokenProviderState {
    worker_id: String,
    token: Option<WorkerTokenState>,
}

/// Lazily refreshes internal worker/sensor-service auth tokens.
#[derive(Debug)]
pub struct WorkerTokenProvider {
    identity_id: i64,
    instance_id: Uuid,
    jwt_config: JwtConfig,
    ttl_seconds: i64,
    refresh_before_seconds: i64,
    state: Mutex<WorkerTokenProviderState>,
}

impl WorkerTokenProvider {
    /// Create a provider with default TTL and refresh window.
    pub fn new(identity_id: i64, worker_id: impl Into<String>, jwt_config: JwtConfig) -> Self {
        Self::new_with_options(
            identity_id,
            worker_id,
            jwt_config,
            DEFAULT_WORKER_TOKEN_TTL_SECONDS,
            DEFAULT_WORKER_TOKEN_REFRESH_BEFORE_SECONDS,
        )
    }

    /// Create a provider with custom token lifetime options.
    pub fn new_with_options(
        identity_id: i64,
        worker_id: impl Into<String>,
        jwt_config: JwtConfig,
        ttl_seconds: i64,
        refresh_before_seconds: i64,
    ) -> Self {
        Self {
            identity_id,
            instance_id: Uuid::new_v4(),
            jwt_config,
            ttl_seconds: ttl_seconds.max(1),
            refresh_before_seconds: refresh_before_seconds.max(0),
            state: Mutex::new(WorkerTokenProviderState {
                worker_id: worker_id.into(),
                token: None,
            }),
        }
    }

    /// Return the process instance ID embedded in every token from this provider.
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    /// Change the worker identity used in future tokens and discard any cached token.
    pub fn set_worker_id(&self, worker_id: impl Into<String>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.worker_id = worker_id.into();
        state.token = None;
    }

    /// Return a valid worker token, refreshing before expiry when needed.
    pub fn token(&self) -> Result<String, JwtError> {
        self.resolve_token(false)
    }

    /// Force token regeneration and return the new token.
    pub fn force_refresh(&self) -> Result<String, JwtError> {
        self.resolve_token(true)
    }

    fn resolve_token(&self, force_refresh: bool) -> Result<String, JwtError> {
        let now = Utc::now().timestamp();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let needs_refresh = force_refresh
            || state
                .token
                .as_ref()
                .map(|current| now >= current.refresh_at_unix)
                .unwrap_or(true);

        if needs_refresh {
            let worker_id = state.worker_id.clone();
            state.token = Some(self.generate_state(now, &worker_id)?);
        }

        state
            .token
            .as_ref()
            .map(|current| current.token.clone())
            .ok_or(JwtError::Invalid)
    }

    fn generate_state(&self, now_unix: i64, worker_id: &str) -> Result<WorkerTokenState, JwtError> {
        let token = generate_worker_token_with_instance(
            self.identity_id,
            worker_id,
            self.instance_id,
            &self.jwt_config,
            Some(self.ttl_seconds),
        )?;

        let refresh_before = self
            .refresh_before_seconds
            .min(self.ttl_seconds.saturating_sub(1));
        let refresh_at_unix = now_unix + self.ttl_seconds - refresh_before;

        Ok(WorkerTokenState {
            token,
            refresh_at_unix,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{crypto_provider, jwt::validate_token};

    fn test_config() -> JwtConfig {
        crypto_provider::install();
        JwtConfig {
            secret: "worker-token-provider-test-secret".to_string(),
            access_token_expiration: 3600,
            refresh_token_expiration: 604800,
        }
    }

    #[test]
    fn refreshes_token_before_expiry_window() {
        let provider = WorkerTokenProvider::new_with_options(1, "sensor-test", test_config(), 2, 1);

        let token_a = provider.token().expect("initial token");
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let token_b = provider.token().expect("refreshed token");

        assert_ne!(token_a, token_b);
    }

    #[test]
    fn changing_worker_id_invalidates_the_cached_token() {
        let config = test_config();
        let provider = WorkerTokenProvider::new(1, "unregistered", config.clone());

        let token_a = provider.token().expect("initial token");
        provider.set_worker_id("42");
        let token_b = provider.token().expect("token for registered worker");
        let claims = validate_token(&token_b, &config).expect("valid token");

        assert_ne!(token_a, token_b);
        assert_eq!(
            claims.metadata.and_then(|metadata| metadata
                .get("worker_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)),
            Some("42".to_string())
        );
    }

    #[test]
    fn provider_keeps_one_instance_id_across_tokens() {
        let config = test_config();
        let provider = WorkerTokenProvider::new(1, "worker-1", config.clone());
        let instance_id = provider.instance_id();

        let token_a = provider.token().expect("initial token");
        provider.set_worker_id("worker-2");
        let token_b = provider.token().expect("replacement token");

        for token in [token_a, token_b] {
            let claims = validate_token(&token, &config).expect("valid worker token");
            assert_eq!(
                claims.metadata.expect("worker metadata")["worker_instance"],
                serde_json::json!(instance_id)
            );
        }
        assert_eq!(provider.instance_id(), instance_id);
    }
}
