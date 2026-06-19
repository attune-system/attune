//! API Client for Attune Platform
//!
//! Provides methods for interacting with the Attune API, including:
//! - Health checks
//! - Event creation
//! - Rule fetching

use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// API client for communicating with Attune
#[derive(Clone)]
pub struct ApiClient {
    inner: Arc<ApiClientInner>,
}

struct ApiClientInner {
    base_url: String,
    token: RwLock<String>,
    client: Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManagedRule {
    pub id: i64,
    pub r#ref: String,
    pub trigger_ref: String,
    pub trigger_params: serde_json::Value,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
struct PaginatedRulesResponse {
    items: Vec<ManagedRule>,
    pagination: PaginationMeta,
}

#[derive(Debug, Deserialize)]
struct PaginationMeta {
    has_next: bool,
}

/// Request to create an event
#[derive(Debug, Clone, Serialize)]
pub struct CreateEventRequest {
    pub trigger_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_instance_id: Option<String>,
}

/// Response from creating an event
#[derive(Debug, Deserialize)]
pub struct CreateEventResponse {
    pub data: EventData,
}

#[derive(Debug, Deserialize)]
pub struct EventData {
    pub id: i64,
}

/// API wrapper response shape.
#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

/// Response data from sensor-token reissue.
#[derive(Debug, Deserialize)]
pub struct RefreshTokenResponse {
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Serialize)]
struct RefreshSensorTokenRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct JwtExpiryClaims {
    #[serde(default)]
    exp: i64,
}

impl ApiClient {
    /// Create a new API client
    pub fn new(base_url: String, token: String) -> Self {
        // Remove trailing slash from base URL if present
        let base_url = base_url.trim_end_matches('/').to_string();

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            inner: Arc::new(ApiClientInner {
                base_url,
                token: RwLock::new(token),
                client,
            }),
        }
    }

    /// Get the current token (for reading)
    pub async fn get_token(&self) -> String {
        self.inner.token.read().await.clone()
    }

    /// Update the token (for refresh)
    async fn set_token(&self, new_token: String) {
        let mut token = self.inner.token.write().await;
        *token = new_token;
    }

    fn current_unix_timestamp() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0)
    }

    fn token_is_expired(token: &str) -> Option<bool> {
        let payload = token.split('.').nth(1)?;
        let decoded = general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .or_else(|_| general_purpose::STANDARD.decode(payload))
            .ok()?;

        let claims: JwtExpiryClaims = serde_json::from_slice(&decoded).ok()?;
        if claims.exp <= 0 {
            return None;
        }

        Some(Self::current_unix_timestamp() >= claims.exp)
    }

    async fn send_with_auth_refresh_retry<F, Fut>(
        &self,
        request_name: &str,
        mut send: F,
    ) -> Result<reqwest::Response>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = std::result::Result<reqwest::Response, reqwest::Error>>,
    {
        let mut refreshed = false;

        loop {
            let token = self.get_token().await;
            let response = send(token.clone())
                .await
                .with_context(|| format!("Failed to send {}", request_name))?;

            if response.status() != StatusCode::UNAUTHORIZED || refreshed {
                return Ok(response);
            }

            match Self::token_is_expired(&token) {
                Some(true) => {
                    warn!(
                        "Received 401 for {}, token is already expired; skipping refresh retry and requiring sensor re-provisioning",
                        request_name
                    );
                    return Ok(response);
                }
                None => {
                    warn!(
                        "Received 401 for {}, token expiry could not be determined; skipping refresh retry",
                        request_name
                    );
                    return Ok(response);
                }
                Some(false) => {}
            }

            warn!(
                "Received 401 for {}, refreshing sensor token and retrying once",
                request_name
            );
            self.refresh_token().await.with_context(|| {
                format!(
                    "Failed to refresh sensor token after unauthorized {}",
                    request_name
                )
            })?;

            refreshed = true;
        }
    }

    /// Perform health check
    pub async fn health_check(&self) -> Result<()> {
        let url = format!("{}/health", self.inner.base_url);

        debug!("Health check: GET {}", url);

        let response = self
            .inner
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send health check request")?;

        if response.status().is_success() {
            info!("Health check succeeded");
            Ok(())
        } else {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read response>".to_string());
            error!("Health check failed: {} - {}", status, body);
            Err(anyhow::anyhow!("Health check failed: {}", status))
        }
    }

    /// Create an event
    pub async fn create_event(&self, request: CreateEventRequest) -> Result<i64> {
        let url = format!("{}/api/v1/events", self.inner.base_url);

        debug!(
            "Creating event: POST {} (trigger_ref={})",
            url, request.trigger_ref
        );

        let response = self
            .send_with_auth_refresh_retry("create event request", |token| {
                self.inner
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .json(&request)
                    .send()
            })
            .await?;

        let status = response.status();

        if status.is_success() {
            let event_response: CreateEventResponse = response
                .json()
                .await
                .context("Failed to parse create event response")?;

            info!(
                "Event created successfully: id={}, trigger_ref={}",
                event_response.data.id, request.trigger_ref
            );

            Ok(event_response.data.id)
        } else {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read response>".to_string());

            error!("Failed to create event: {} - {}", status, body);

            // Special handling for 403 Forbidden (trigger type not allowed)
            if status == StatusCode::FORBIDDEN {
                return Err(anyhow::anyhow!(
                    "Insufficient permissions to create event for trigger ref '{}'. \
                     This sensor token may not be authorized for this trigger type.",
                    request.trigger_ref
                ));
            }

            Err(anyhow::anyhow!(
                "Failed to create event: {} - {}",
                status,
                body
            ))
        }
    }

    /// Create event with retry logic
    pub async fn create_event_with_retry(&self, request: CreateEventRequest) -> Result<i64> {
        const MAX_RETRIES: u32 = 3;
        const INITIAL_BACKOFF_MS: u64 = 100;

        let mut attempt = 0;
        let mut last_error = None;

        while attempt < MAX_RETRIES {
            match self.create_event(request.clone()).await {
                Ok(event_id) => return Ok(event_id),
                Err(e) => {
                    // Don't retry on 403 Forbidden (authorization error)
                    if e.to_string().contains("Insufficient permissions") {
                        return Err(e);
                    }

                    attempt += 1;
                    last_error = Some(e);

                    if attempt < MAX_RETRIES {
                        let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                        warn!(
                            "Event creation failed (attempt {}/{}), retrying in {}ms",
                            attempt, MAX_RETRIES, backoff_ms
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Event creation failed after retries")))
    }

    /// Refresh the current token
    pub async fn refresh_token(&self) -> Result<String> {
        let current_token = self.get_token().await;
        let url = format!("{}/auth/internal/sensor-token", self.inner.base_url);
        debug!("Reissuing sensor token: POST {}", url);

        let request = RefreshSensorTokenRequest { ttl_seconds: None };

        let response = self
            .inner
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", current_token))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send sensor token reissue request")?;

        let status = response.status();

        if status.is_success() {
            let refresh_response: ApiResponse<RefreshTokenResponse> = response
                .json()
                .await
                .context("Failed to parse token refresh response")?;

            info!(
                "Sensor token refreshed successfully, expires at: {}",
                refresh_response.data.expires_at
            );

            // Update stored token
            self.set_token(refresh_response.data.token.clone()).await;

            Ok(refresh_response.data.token)
        } else {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read response>".to_string());

            error!("Failed to refresh sensor token: {} - {}", status, body);

            Err(anyhow::anyhow!(
                "Failed to refresh sensor token: {} - {}",
                status,
                body
            ))
        }
    }

    /// List active rules for the provided trigger refs.
    pub async fn list_active_rules_by_trigger_refs(
        &self,
        trigger_refs: &[&str],
    ) -> Result<Vec<ManagedRule>> {
        let mut rules = Vec::new();
        for trigger_ref in trigger_refs {
            rules.extend(self.list_active_rules_by_trigger_ref(trigger_ref).await?);
        }
        Ok(rules)
    }

    async fn list_active_rules_by_trigger_ref(
        &self,
        trigger_ref: &str,
    ) -> Result<Vec<ManagedRule>> {
        let mut page = 1_u32;
        let mut rules = Vec::new();

        loop {
            let url = format!(
                "{}/api/v1/rules?trigger_ref={}&enabled=true&page={}&page_size=100",
                self.inner.base_url, trigger_ref, page
            );
            let request_name = format!("active rules fetch request for trigger {}", trigger_ref);
            let response = self
                .send_with_auth_refresh_retry(&request_name, |token| {
                    self.inner
                        .client
                        .get(&url)
                        .header("Authorization", format!("Bearer {}", token))
                        .send()
                })
                .await?;

            let status = response.status();
            if !status.is_success() {
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "<unable to read response>".to_string());
                return Err(anyhow::anyhow!(
                    "Failed to fetch active rules for trigger {}: {} - {}",
                    trigger_ref,
                    status,
                    body
                ));
            }

            let page_response: PaginatedRulesResponse =
                response.json().await.with_context(|| {
                    format!(
                        "Failed to parse active rules response for trigger {}",
                        trigger_ref
                    )
                })?;

            rules.extend(page_response.items.into_iter().filter(|rule| rule.enabled));
            if !page_response.pagination.has_next {
                break;
            }
            page += 1;
        }

        Ok(rules)
    }
}

impl CreateEventRequest {
    /// Create a new event request
    pub fn new(trigger_ref: String, payload: serde_json::Value) -> Self {
        Self {
            trigger_ref,
            payload: Some(payload),
            config: None,
            trigger_instance_id: None,
        }
    }

    /// Set trigger instance ID (typically rule_id)
    pub fn with_trigger_instance_id(mut self, id: String) -> Self {
        self.trigger_instance_id = Some(id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn authorization_header(request: &str) -> Option<String> {
        request.lines().find_map(|line| {
            let (header_name, header_value) = line.split_once(':')?;
            if header_name.eq_ignore_ascii_case("authorization") {
                Some(header_value.trim().to_string())
            } else {
                None
            }
        })
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];

        loop {
            let read = socket.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        String::from_utf8(buffer).unwrap()
    }

    async fn write_json_response(
        socket: &mut tokio::net::TcpStream,
        status: StatusCode,
        body: &str,
    ) {
        let status_text = status.canonical_reason().unwrap_or("Unknown");
        let response = format!(
            "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            status.as_u16(),
            status_text,
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    }

    fn jwt_with_expiration(exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"sub":"42","login":"core.timer_sensor","iat":1,"exp":{exp}}}"#
        ));
        format!("{header}.{payload}.signature")
    }

    fn non_expired_sensor_token() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        jwt_with_expiration(now + 3600)
    }

    fn expired_sensor_token() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        jwt_with_expiration(now - 60)
    }

    #[test]
    fn test_create_event_request() {
        let payload = serde_json::json!({
            "timestamp": "2025-01-27T12:34:56Z",
            "scheduled_time": "2025-01-27T12:34:56Z"
        });

        let request = CreateEventRequest::new("core.timer".to_string(), payload.clone());

        assert_eq!(request.trigger_ref, "core.timer");
        assert_eq!(request.payload, Some(payload));
        assert!(request.trigger_instance_id.is_none());
    }

    #[test]
    fn test_create_event_request_with_instance_id() {
        let payload = serde_json::json!({
            "timestamp": "2025-01-27T12:34:56Z"
        });

        let request = CreateEventRequest::new("core.timer".to_string(), payload)
            .with_trigger_instance_id("rule_123".to_string());

        assert_eq!(request.trigger_instance_id, Some("rule_123".to_string()));
    }

    #[test]
    fn test_base_url_trailing_slash_removed() {
        let client = ApiClient::new("http://localhost:8080/".to_string(), "token".to_string());
        assert_eq!(client.inner.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_paginated_rules_response_deserialization() {
        let payload = serde_json::json!({
            "items": [
                {
                    "id": 42,
                    "ref": "core.timer_rule",
                    "trigger_ref": "core.intervaltimer",
                    "trigger_params": { "interval": 5, "unit": "seconds" },
                    "enabled": true
                }
            ],
            "pagination": {
                "has_next": false
            }
        });

        let response: PaginatedRulesResponse = serde_json::from_value(payload).unwrap();
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].id, 42);
        assert!(!response.pagination.has_next);
    }

    #[test]
    fn test_refresh_token_response_deserialization() {
        let payload = serde_json::json!({
            "data": {
                "identity_id": 42,
                "sensor_ref": "core.timer_sensor",
                "token": "new-token",
                "expires_at": "2026-06-20T00:00:00Z",
                "trigger_types": ["core.intervaltimer"]
            }
        });

        let response: ApiResponse<RefreshTokenResponse> = serde_json::from_value(payload).unwrap();
        assert_eq!(response.data.token, "new-token");
        assert_eq!(response.data.expires_at, "2026-06-20T00:00:00Z");
    }

    #[test]
    fn test_refresh_request_serialization_only_includes_ttl() {
        let request = RefreshSensorTokenRequest {
            ttl_seconds: Some(7200),
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value, serde_json::json!({ "ttl_seconds": 7200 }));
    }

    #[tokio::test]
    async fn test_create_event_refreshes_token_after_401_when_token_not_expired() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let initial_token = non_expired_sensor_token();
        let initial_authorization = format!("Bearer {}", initial_token);

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with("POST /api/v1/events "));
            assert_eq!(
                authorization_header(&request).as_deref(),
                Some(initial_authorization.as_str())
            );
            write_json_response(
                &mut socket,
                StatusCode::UNAUTHORIZED,
                r#"{"error":"unauthorized"}"#,
            )
            .await;

            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with("POST /auth/internal/sensor-token "));
            assert_eq!(
                authorization_header(&request).as_deref(),
                Some(initial_authorization.as_str())
            );
            write_json_response(
                &mut socket,
                StatusCode::OK,
                r#"{"data":{"token":"fresh-token","expires_at":"2026-06-20T00:00:00Z"}}"#,
            )
            .await;

            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with("POST /api/v1/events "));
            assert_eq!(
                authorization_header(&request).as_deref(),
                Some("Bearer fresh-token")
            );
            write_json_response(&mut socket, StatusCode::OK, r#"{"data":{"id":42}}"#).await;
        });

        let client = ApiClient::new(format!("http://{}", addr), initial_token);
        let request = CreateEventRequest::new("core.timer".to_string(), serde_json::json!({}));

        let event_id = client.create_event(request).await.unwrap();
        assert_eq!(event_id, 42);
        assert_eq!(client.get_token().await, "fresh-token");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_list_rules_refreshes_token_after_401_when_token_not_expired() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let initial_token = non_expired_sensor_token();
        let initial_authorization = format!("Bearer {}", initial_token);

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(
                request.starts_with(
                    "GET /api/v1/rules?trigger_ref=core.intervaltimer&enabled=true&page=1&page_size=100 "
                )
            );
            assert_eq!(
                authorization_header(&request).as_deref(),
                Some(initial_authorization.as_str())
            );
            write_json_response(
                &mut socket,
                StatusCode::UNAUTHORIZED,
                r#"{"error":"unauthorized"}"#,
            )
            .await;

            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with("POST /auth/internal/sensor-token "));
            assert_eq!(
                authorization_header(&request).as_deref(),
                Some(initial_authorization.as_str())
            );
            write_json_response(
                &mut socket,
                StatusCode::OK,
                r#"{"data":{"token":"fresh-token","expires_at":"2026-06-20T00:00:00Z"}}"#,
            )
            .await;

            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(
                request.starts_with(
                    "GET /api/v1/rules?trigger_ref=core.intervaltimer&enabled=true&page=1&page_size=100 "
                )
            );
            assert_eq!(
                authorization_header(&request).as_deref(),
                Some("Bearer fresh-token")
            );
            write_json_response(
                &mut socket,
                StatusCode::OK,
                r#"{"items":[{"id":7,"ref":"core.timer_rule","trigger_ref":"core.intervaltimer","trigger_params":{"interval":5,"unit":"seconds"},"enabled":true}],"pagination":{"has_next":false}}"#,
            )
            .await;
        });

        let client = ApiClient::new(format!("http://{}", addr), initial_token);
        let rules = client
            .list_active_rules_by_trigger_refs(&["core.intervaltimer"])
            .await
            .unwrap();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, 7);
        assert_eq!(client.get_token().await, "fresh-token");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_create_event_does_not_refresh_token_after_401_when_token_expired() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let expired_token = expired_sensor_token();
        let expired_authorization = format!("Bearer {}", expired_token);

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            assert!(request.starts_with("POST /api/v1/events "));
            assert_eq!(
                authorization_header(&request).as_deref(),
                Some(expired_authorization.as_str())
            );
            write_json_response(
                &mut socket,
                StatusCode::UNAUTHORIZED,
                r#"{"error":"unauthorized"}"#,
            )
            .await;
        });

        let client = ApiClient::new(format!("http://{}", addr), expired_token.clone());
        let request = CreateEventRequest::new("core.timer".to_string(), serde_json::json!({}));
        let error = client
            .create_event(request)
            .await
            .expect_err("expired tokens should not trigger refresh retry");

        assert!(
            error.to_string().contains("Failed to create event: 401"),
            "unexpected error: {error}"
        );
        assert_eq!(client.get_token().await, expired_token);

        server.await.unwrap();
    }
}
