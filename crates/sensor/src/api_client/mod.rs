//! API Client for Sensor Service
//!
//! This module provides an HTTP client for the sensor service to communicate
//! with the Attune API for token provisioning and other operations.

use anyhow::{Context, Result};
use attune_common::auth::WorkerTokenProvider;
use attune_common::models::SensorWorkloadFence;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// API client for sensor service
#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    client: Client,
    /// Optional admin token for authentication (if available)
    admin_token: Option<String>,
    worker_token_provider: Option<Arc<WorkerTokenProvider>>,
}

/// Request to create a sensor token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSensorTokenRequest {
    pub sensor_ref: String,
    pub pack_ref: String,
    pub trigger_types: Vec<String>,
    #[serde(default)]
    pub permission_set_refs: Vec<String>,
    pub workload_id: i64,
    pub assignment_generation: i64,
    pub worker_instance: uuid::Uuid,
    pub ttl_seconds: Option<i64>,
}

/// Exact registered scope requested for a managed sensor token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorTokenScope {
    pub sensor_ref: String,
    pub pack_ref: String,
    pub trigger_types: Vec<String>,
    pub permission_set_refs: Vec<String>,
    pub workload_fence: SensorWorkloadFence,
}

impl SensorTokenScope {
    fn validate(&self) -> Result<()> {
        if self.sensor_ref.trim().is_empty() {
            return Err(anyhow::anyhow!("Sensor token scope requires sensor_ref"));
        }
        if self.pack_ref.trim().is_empty() {
            return Err(anyhow::anyhow!("Sensor token scope requires pack_ref"));
        }
        if canonical_refs(&self.trigger_types).is_empty() {
            return Err(anyhow::anyhow!(
                "Sensor token scope requires at least one trigger type"
            ));
        }
        if self
            .permission_set_refs
            .iter()
            .any(|permission_ref| permission_ref.trim().is_empty())
        {
            return Err(anyhow::anyhow!(
                "Sensor token permission-set refs must be non-empty"
            ));
        }
        if self.workload_fence.workload_id <= 0
            || self.workload_fence.worker_id <= 0
            || self.workload_fence.generation <= 0
        {
            return Err(anyhow::anyhow!(
                "Sensor token scope requires a current workload fence"
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_response(&self, response: &SensorTokenResponse) -> Result<()> {
        if response.sensor_ref != self.sensor_ref {
            return Err(anyhow::anyhow!(
                "Sensor token response scope mismatch: requested sensor {}, received {}",
                self.sensor_ref,
                response.sensor_ref
            ));
        }
        if canonical_refs(&response.trigger_types) != canonical_refs(&self.trigger_types) {
            return Err(anyhow::anyhow!(
                "Sensor token response trigger scope does not match the registered sensor"
            ));
        }

        let requested_permissions = canonical_refs(&self.permission_set_refs);
        let returned_permissions = canonical_refs(&response.permission_set_refs);
        if returned_permissions != requested_permissions {
            return Err(anyhow::anyhow!(
                "Sensor token response cache authority does not match the explicit permission-set request"
            ));
        }
        if response.workload_fence != Some(self.workload_fence) {
            return Err(anyhow::anyhow!(
                "Sensor token response workload fence does not match the current process authority"
            ));
        }

        match response.pack_ref.as_deref() {
            Some(pack_ref) if pack_ref != self.pack_ref => Err(anyhow::anyhow!(
                "Sensor token response scope mismatch: requested pack {}, received {}",
                self.pack_ref,
                pack_ref
            )),
            None if !requested_permissions.is_empty() => Err(anyhow::anyhow!(
                "Sensor token response omitted pack scope for explicit cache authority"
            )),
            _ => Ok(()),
        }
    }
}

fn canonical_refs(refs: &[String]) -> Vec<String> {
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

/// Response from sensor token creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorTokenResponse {
    pub identity_id: i64,
    pub sensor_ref: String,
    #[serde(default)]
    pub pack_ref: Option<String>,
    pub token: String,
    pub expires_at: String,
    pub trigger_types: Vec<String>,
    #[serde(default)]
    pub permission_set_refs: Vec<String>,
    #[serde(default)]
    pub workload_fence: Option<SensorWorkloadFence>,
}

/// Wrapper for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub data: T,
}

impl ApiClient {
    /// Create a new API client
    pub fn new(
        base_url: String,
        admin_token: Option<String>,
        worker_token_provider: Option<Arc<WorkerTokenProvider>>,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::new(),
            admin_token,
            worker_token_provider,
        }
    }

    fn authorization_token(&self) -> Result<String> {
        if let Some(provider) = &self.worker_token_provider {
            return provider
                .token()
                .context("Failed to acquire worker auth token");
        }
        self.admin_token.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Sensor token provisioning requires authenticated worker or admin authority"
            )
        })
    }

    pub fn set_worker_id(&self, worker_id: i64) {
        if let Some(provider) = &self.worker_token_provider {
            provider.set_worker_id(worker_id.to_string());
        }
    }

    /// Create a sensor token via the API
    ///
    /// This is used internally by the sensor service to provision tokens
    /// for standalone sensors when they are started.
    pub async fn create_sensor_token(
        &self,
        scope: &SensorTokenScope,
        ttl_seconds: Option<i64>,
    ) -> Result<SensorTokenResponse> {
        scope.validate()?;
        let url = format!("{}/auth/internal/sensor-token", self.base_url);

        let request = CreateSensorTokenRequest {
            sensor_ref: scope.sensor_ref.clone(),
            pack_ref: scope.pack_ref.clone(),
            trigger_types: scope.trigger_types.clone(),
            permission_set_refs: scope.permission_set_refs.clone(),
            workload_id: scope.workload_fence.workload_id,
            assignment_generation: scope.workload_fence.generation,
            worker_instance: scope.workload_fence.worker_instance,
            ttl_seconds,
        };

        let token = self.authorization_token()?;
        let req = self
            .client
            .post(&url)
            .json(&request)
            .header("Authorization", format!("Bearer {}", token));

        let mut response = req
            .send()
            .await
            .context("Failed to send sensor token creation request")?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(provider) = &self.worker_token_provider {
                let refreshed = provider
                    .force_refresh()
                    .context("Failed to refresh worker auth token after 401")?;
                response = self
                    .client
                    .post(&url)
                    .json(&request)
                    .header("Authorization", format!("Bearer {}", refreshed))
                    .send()
                    .await
                    .context("Failed to retry sensor token creation request after 401")?;
            }
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "API request failed with status {}: {}",
                status,
                body
            ));
        }

        let api_response: ApiResponse<SensorTokenResponse> = response
            .json()
            .await
            .context("Failed to parse sensor token response")?;

        scope.validate_response(&api_response.data)?;

        Ok(api_response.data)
    }

    /// Health check endpoint
    pub async fn health_check(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send health check request")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Health check failed with status: {}",
                response.status()
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workload_fence() -> SensorWorkloadFence {
        SensorWorkloadFence {
            workload_id: 10,
            worker_id: 20,
            worker_instance: uuid::Uuid::nil(),
            generation: 3,
        }
    }

    #[test]
    fn test_api_client_creation() {
        let client = ApiClient::new("http://localhost:8080".to_string(), None, None);
        assert_eq!(client.base_url, "http://localhost:8080");
        assert!(client.authorization_token().is_err());
    }

    #[test]
    fn test_api_client_normalizes_trailing_slash() {
        let client = ApiClient::new("http://localhost:8080/".to_string(), None, None);
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_api_client_with_token() {
        let client = ApiClient::new(
            "http://localhost:8080".to_string(),
            Some("test_token".to_string()),
            None,
        );
        assert_eq!(client.admin_token, Some("test_token".to_string()));
    }

    #[test]
    fn sensor_token_request_serializes_exact_cache_scope() {
        let request = CreateSensorTokenRequest {
            sensor_ref: "salesforce.account_sensor".to_string(),
            pack_ref: "salesforce".to_string(),
            trigger_types: vec!["salesforce.account_changed".to_string()],
            permission_set_refs: vec![
                "standard".to_string(),
                "salesforce.cache_writer".to_string(),
            ],
            workload_id: 10,
            assignment_generation: 3,
            worker_instance: uuid::Uuid::nil(),
            ttl_seconds: Some(3600),
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["sensor_ref"], "salesforce.account_sensor");
        assert_eq!(value["pack_ref"], "salesforce");
        assert_eq!(
            value["permission_set_refs"],
            serde_json::json!(["standard", "salesforce.cache_writer"])
        );
        assert!(value.get("roles").is_none());
        assert!(value.get("identity_roles").is_none());
    }

    #[test]
    fn sensor_token_scope_rejects_missing_or_mismatched_cache_authority() {
        let scope = SensorTokenScope {
            sensor_ref: "salesforce.account_sensor".to_string(),
            pack_ref: "salesforce".to_string(),
            trigger_types: vec!["salesforce.account_changed".to_string()],
            permission_set_refs: vec!["standard".to_string()],
            workload_fence: workload_fence(),
        };
        let missing_authority = SensorTokenResponse {
            identity_id: 1,
            sensor_ref: scope.sensor_ref.clone(),
            pack_ref: None,
            token: "token".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            trigger_types: scope.trigger_types.clone(),
            permission_set_refs: Vec::new(),
            workload_fence: Some(scope.workload_fence),
        };
        assert!(scope.validate_response(&missing_authority).is_err());

        let exact = SensorTokenResponse {
            pack_ref: Some(scope.pack_ref.clone()),
            permission_set_refs: scope.permission_set_refs.clone(),
            ..missing_authority
        };
        scope.validate_response(&exact).unwrap();
    }

    #[test]
    fn sensor_token_scope_rejects_a_response_without_its_workload_fence() {
        let scope = SensorTokenScope {
            sensor_ref: "core.timer_sensor".to_string(),
            pack_ref: "core".to_string(),
            trigger_types: vec!["core.timer".to_string()],
            permission_set_refs: Vec::new(),
            workload_fence: workload_fence(),
        };
        let response = SensorTokenResponse {
            identity_id: 1,
            sensor_ref: scope.sensor_ref.clone(),
            pack_ref: None,
            token: "token".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
            trigger_types: scope.trigger_types.clone(),
            permission_set_refs: Vec::new(),
            workload_fence: None,
        };

        assert!(scope.validate_response(&response).is_err());
    }
}
