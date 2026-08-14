use anyhow::{Context, Result};
use reqwest::{header, multipart, Client as HttpClient, Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use std::env;
use std::path::PathBuf;
use std::time::Duration;

use crate::config::CliConfig;

/// API client for interacting with Attune API
pub struct ApiClient {
    client: HttpClient,
    base_url: String,
    auth_token: Option<String>,
    refresh_token: Option<String>,
    config_path: Option<PathBuf>,
}

/// Standard API response wrapper
#[derive(Debug, serde::Deserialize)]
pub struct ApiResponse<T> {
    pub data: T,
}

#[derive(Debug, serde::Deserialize)]
struct PaginatedResponse<T> {
    items: Vec<T>,
}

/// API error response
#[derive(Debug, serde::Deserialize)]
pub struct ApiError {
    pub error: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub _details: Option<serde_json::Value>,
}

fn build_http_client(timeout: Duration) -> HttpClient {
    let builder = HttpClient::builder().timeout(timeout);
    match builder.build() {
        Ok(client) => client,
        Err(err) => {
            let certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
                .iter()
                .map(|cert| reqwest::Certificate::from_der(cert.as_ref()))
                .collect::<std::result::Result<Vec<_>, _>>()
                .expect("Failed to load bundled root certificates");

            HttpClient::builder()
                .timeout(timeout)
                .tls_certs_only(certs)
                .build()
                .unwrap_or_else(|fallback_err| {
                    panic!(
                        "Failed to build HTTP client. default builder error: {err:?}; bundled-root fallback error: {fallback_err:?}"
                    )
                })
        }
    }
}

fn parse_json_response<T: DeserializeOwned>(body: &str, description: &str) -> Result<T> {
    let mut deserializer = serde_json::Deserializer::from_str(body);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|err| {
        let snippet = response_snippet(body);
        anyhow::anyhow!(
            "{} at JSON path '{}': {}. Response body starts with: {}",
            description,
            err.path(),
            err.inner(),
            snippet
        )
    })
}

fn response_snippet(body: &str) -> String {
    const MAX_CHARS: usize = 1200;
    let mut snippet: String = body.chars().take(MAX_CHARS).collect();
    if body.chars().count() > MAX_CHARS {
        snippet.push('…');
    }
    snippet
}

impl ApiClient {
    pub fn from_config_with_timeout(
        config: &CliConfig,
        api_url_override: &Option<String>,
        timeout: Duration,
    ) -> Self {
        let mut client = Self::from_config(config, api_url_override);
        client.client = build_http_client(timeout);
        // A completion request must not refresh or persist credentials.
        client.refresh_token = None;
        client.config_path = None;
        client
    }

    /// Create a new API client from configuration
    pub fn from_config(config: &CliConfig, api_url_override: &Option<String>) -> Self {
        let base_url = config.effective_api_url(api_url_override);
        let auth_token = env::var("ATTUNE_API_TOKEN")
            .ok()
            .or_else(|| env::var("ATTUNE_AUTH_TOKEN").ok())
            .or_else(|| config.auth_token().ok().flatten());
        let refresh_token = env::var("ATTUNE_REFRESH_TOKEN")
            .ok()
            .or_else(|| config.refresh_token().ok().flatten());
        let config_path = CliConfig::config_path().ok();

        Self {
            client: build_http_client(Duration::from_secs(300)),
            base_url,
            auth_token,
            refresh_token,
            config_path,
        }
    }

    /// Create a new API client
    /// Return the base URL this client is configured to talk to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    #[cfg(test)]
    pub fn new(base_url: String, auth_token: Option<String>) -> Self {
        Self {
            client: build_http_client(Duration::from_secs(300)),
            base_url,
            auth_token,
            refresh_token: None,
            config_path: None,
        }
    }

    /// Set the authentication token
    pub fn set_auth_token(&mut self, token: String) {
        self.auth_token = Some(token);
    }

    /// Replace both auth and refresh tokens (e.g. after re-login).
    pub fn set_tokens(&mut self, access_token: String, refresh_token: String) {
        self.auth_token = Some(access_token);
        self.refresh_token = Some(refresh_token);
    }

    /// Clear the authentication token
    #[cfg(test)]
    pub fn clear_auth_token(&mut self) {
        self.auth_token = None;
    }

    /// Refresh the authentication token using the refresh token.
    ///
    /// Returns `Ok(true)` if refresh succeeded, `Ok(false)` if no refresh token
    /// is available or the server rejected it.
    async fn refresh_auth_token(&mut self) -> Result<bool> {
        let refresh_token = match &self.refresh_token {
            Some(token) => token.clone(),
            None => return Ok(false),
        };

        #[derive(Serialize)]
        struct RefreshRequest {
            refresh_token: String,
        }

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
        }

        let url = format!("{}/auth/refresh", self.base_url);
        let req = self
            .client
            .post(&url)
            .json(&RefreshRequest { refresh_token });

        let response = req.send().await.context("Failed to refresh token")?;

        if !response.status().is_success() {
            // Refresh failed — clear tokens so we don't keep retrying
            self.auth_token = None;
            self.refresh_token = None;
            return Ok(false);
        }

        let api_response: ApiResponse<TokenResponse> = response
            .json()
            .await
            .context("Failed to parse refresh response")?;

        // Update in-memory tokens
        self.auth_token = Some(api_response.data.access_token.clone());
        self.refresh_token = Some(api_response.data.refresh_token.clone());

        // Persist to config file
        if self.config_path.is_some() {
            if let Ok(mut config) = CliConfig::load() {
                let _ = config.set_auth(
                    api_response.data.access_token,
                    api_response.data.refresh_token,
                );
            }
        }

        Ok(true)
    }

    // ── Request building helpers ────────────────────────────────────────

    /// Build a full URL from a path.
    fn url_for(&self, path: &str) -> String {
        if path.starts_with("/auth") {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}/api/v1{}", self.base_url, path)
        }
    }

    /// Build a `RequestBuilder` with auth header applied.
    fn build_request(&self, method: Method, path: &str) -> RequestBuilder {
        let url = self.url_for(path);
        let mut req = self.client.request(method, &url);
        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }
        req
    }

    // ── Core execute-with-retry machinery ──────────────────────────────

    /// Send a request that carries a JSON body.  On a 401 response the token
    /// is refreshed and the request is rebuilt & retried exactly once.
    async fn execute_json<T, B>(
        &mut self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        // First attempt
        let req = self.attach_body(self.build_request(method.clone(), path), body);
        let response = req.send().await.context("Failed to send request to API")?;

        if response.status() == StatusCode::UNAUTHORIZED
            && self.refresh_token.is_some()
            && self.refresh_auth_token().await?
        {
            // Retry with new token
            let req = self.attach_body(self.build_request(method, path), body);
            let response = req
                .send()
                .await
                .context("Failed to send request to API (retry)")?;
            return self.handle_response(response).await;
        }

        self.handle_response(response).await
    }

    /// Send a request that carries a JSON body and expects no response body.
    async fn execute_json_no_response<B: Serialize>(
        &mut self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<()> {
        let req = self.attach_body(self.build_request(method.clone(), path), body);
        let response = req.send().await.context("Failed to send request to API")?;

        if response.status() == StatusCode::UNAUTHORIZED
            && self.refresh_token.is_some()
            && self.refresh_auth_token().await?
        {
            let req = self.attach_body(self.build_request(method, path), body);
            let response = req
                .send()
                .await
                .context("Failed to send request to API (retry)")?;
            return self.handle_empty_response(response).await;
        }

        self.handle_empty_response(response).await
    }

    /// Optionally attach a JSON body to a request builder.
    fn attach_body<B: Serialize>(&self, req: RequestBuilder, body: Option<&B>) -> RequestBuilder {
        match body {
            Some(b) => req.json(b),
            None => req,
        }
    }

    // ── Response handling ──────────────────────────────────────────────

    /// Parse a successful API response or return a descriptive error.
    async fn handle_response<T: DeserializeOwned>(&self, response: reqwest::Response) -> Result<T> {
        let status = response.status();

        if status.is_success() {
            let body = response
                .text()
                .await
                .context("Failed to read API response body")?;
            let api_response: ApiResponse<T> =
                parse_json_response(&body, "Failed to parse API response")?;
            Ok(api_response.data)
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            if let Ok(api_error) = serde_json::from_str::<ApiError>(&error_text) {
                anyhow::bail!("API error ({}): {}", status, api_error.error);
            } else {
                anyhow::bail!("API error ({}): {}", status, error_text);
            }
        }
    }

    /// Parse a cache API response.
    ///
    /// Cache endpoints use purpose-specific envelopes for cursor metadata and
    /// bulk lifecycle responses. During the API transition, accept either the
    /// normal `{ "data": ... }` envelope or an endpoint-specific top-level
    /// response without making the cache command depend on a second client.
    async fn handle_cache_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read cache API response body")?;

        if !status.is_success() {
            if let Ok(api_error) = serde_json::from_str::<ApiError>(&body) {
                if let Some(code) = api_error.code {
                    anyhow::bail!("Cache API error ({status}, {code}): {}", api_error.error);
                }
                anyhow::bail!("Cache API error ({}): {}", status, api_error.error);
            }
            anyhow::bail!("Cache API error ({}): {}", status, body);
        }

        let value: serde_json::Value =
            parse_json_response(&body, "Failed to parse cache API response")?;
        let payload = value.get("data").unwrap_or(&value);
        serde_json::from_value(payload.clone()).context("Failed to parse cache API response data")
    }

    async fn execute_cache_json<T, B>(
        &mut self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        let req = self.attach_body(self.build_request(method.clone(), path), body);
        let response = req
            .send()
            .await
            .context("Failed to send request to cache API")?;

        if response.status() == StatusCode::UNAUTHORIZED
            && self.refresh_token.is_some()
            && self.refresh_auth_token().await?
        {
            let req = self.attach_body(self.build_request(method, path), body);
            let response = req
                .send()
                .await
                .context("Failed to send request to cache API (retry)")?;
            return self.handle_cache_response(response).await;
        }

        self.handle_cache_response(response).await
    }

    async fn handle_paginated_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<Vec<T>> {
        let status = response.status();
        if status.is_success() {
            let body = response
                .text()
                .await
                .context("Failed to read paginated API response body")?;
            let paginated: PaginatedResponse<T> =
                parse_json_response(&body, "Failed to parse paginated API response")?;
            Ok(paginated.items)
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            if let Ok(api_error) = serde_json::from_str::<ApiError>(&error_text) {
                anyhow::bail!("API error ({}): {}", status, api_error.error);
            } else {
                anyhow::bail!("API error ({}): {}", status, error_text);
            }
        }
    }

    /// Handle a response where we only care about success/failure, not a body.
    async fn handle_empty_response(&self, response: reqwest::Response) -> Result<()> {
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            if let Ok(api_error) = serde_json::from_str::<ApiError>(&error_text) {
                anyhow::bail!("API error ({}): {}", status, api_error.error);
            } else {
                anyhow::bail!("API error ({}): {}", status, error_text);
            }
        }
    }

    // ── Public convenience methods ─────────────────────────────────────

    /// GET request
    pub async fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        self.execute_json::<T, ()>(Method::GET, path, None).await
    }

    pub async fn get_paginated<T: DeserializeOwned>(&mut self, path: &str) -> Result<Vec<T>> {
        let req = self.build_request(Method::GET, path);
        let response = req.send().await.context("Failed to send request to API")?;

        if response.status() == StatusCode::UNAUTHORIZED
            && self.refresh_token.is_some()
            && self.refresh_auth_token().await?
        {
            let req = self.build_request(Method::GET, path);
            let response = req
                .send()
                .await
                .context("Failed to send request to API (retry)")?;
            return self.handle_paginated_response(response).await;
        }

        self.handle_paginated_response(response).await
    }

    /// GET request with query parameters (query string must be in path)
    ///
    /// Part of REST client API - reserved for future advanced filtering/search features.
    /// Example: `client.get_with_query("/actions?enabled=true&pack=core").await`
    #[allow(dead_code)]
    pub async fn get_with_query<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        self.execute_json::<T, ()>(Method::GET, path, None).await
    }

    /// POST request with JSON body
    pub async fn post<T: DeserializeOwned, B: Serialize>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.execute_json(Method::POST, path, Some(body)).await
    }

    /// PUT request with JSON body
    ///
    /// Part of REST client API - will be used for update operations
    pub async fn put<T: DeserializeOwned, B: Serialize>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.execute_json(Method::PUT, path, Some(body)).await
    }

    /// PATCH request with JSON body
    pub async fn patch<T: DeserializeOwned, B: Serialize>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.execute_json(Method::PATCH, path, Some(body)).await
    }

    /// GET a cache API response. Cache handlers may return a direct payload or
    /// the standard API `data` envelope.
    pub async fn cache_get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        self.execute_cache_json::<T, ()>(Method::GET, path, None)
            .await
    }

    /// POST a cache API request. Refresh chunk replays intentionally retain
    /// their supplied idempotency fields on the one auth-refresh retry.
    pub async fn cache_post<T: DeserializeOwned, B: Serialize>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.execute_cache_json(Method::POST, path, Some(body))
            .await
    }

    /// PATCH cache namespace policy fields.
    pub async fn cache_patch<T: DeserializeOwned, B: Serialize>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.execute_cache_json(Method::PATCH, path, Some(body))
            .await
    }

    /// PUT a bounded cache ingest chunk.
    pub async fn cache_put<T: DeserializeOwned, B: Serialize>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.execute_cache_json(Method::PUT, path, Some(body)).await
    }

    /// DELETE a cache resource. A successful empty body is accepted.
    pub async fn cache_delete(&mut self, path: &str) -> Result<()> {
        let req = self.build_request(Method::DELETE, path);
        let response = req
            .send()
            .await
            .context("Failed to send request to cache API")?;

        if response.status() == StatusCode::UNAUTHORIZED
            && self.refresh_token.is_some()
            && self.refresh_auth_token().await?
        {
            let response = self
                .build_request(Method::DELETE, path)
                .send()
                .await
                .context("Failed to send request to cache API (retry)")?;
            return self.handle_cache_delete_response(response).await;
        }

        self.handle_cache_delete_response(response).await
    }

    async fn handle_cache_delete_response(&self, response: reqwest::Response) -> Result<()> {
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        if let Ok(api_error) = serde_json::from_str::<ApiError>(&body) {
            if let Some(code) = api_error.code {
                anyhow::bail!("Cache API error ({status}, {code}): {}", api_error.error);
            }
            anyhow::bail!("Cache API error ({}): {}", status, api_error.error);
        }
        anyhow::bail!("Cache API error ({}): {}", status, body);
    }

    /// DELETE request with response parsing
    ///
    /// Part of REST client API - reserved for delete operations that return data.
    /// Currently we use `delete_no_response()` for all delete operations.
    /// This method is kept for API completeness and future use cases where
    /// delete operations return metadata (e.g., cascade deletion summaries).
    #[allow(dead_code)]
    pub async fn delete<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        self.execute_json::<T, ()>(Method::DELETE, path, None).await
    }

    /// POST request without expecting response body
    ///
    /// Part of REST client API - reserved for fire-and-forget operations.
    /// Example use cases: webhook notifications, event submissions, audit logging.
    /// Kept for API completeness even though not currently used.
    #[allow(dead_code)]
    pub async fn post_no_response<B: Serialize>(&mut self, path: &str, body: &B) -> Result<()> {
        self.execute_json_no_response(Method::POST, path, Some(body))
            .await
    }

    /// DELETE request without expecting response body
    pub async fn delete_no_response(&mut self, path: &str) -> Result<()> {
        self.execute_json_no_response::<()>(Method::DELETE, path, None)
            .await
    }

    /// GET request that returns raw bytes and optional filename from Content-Disposition.
    ///
    /// Used for downloading binary content (e.g., artifact files).
    /// Returns `(bytes, content_type, optional_filename)`.
    pub async fn download_bytes(
        &mut self,
        path: &str,
    ) -> Result<(Vec<u8>, String, Option<String>)> {
        // First attempt
        let req = self.build_request(Method::GET, path);
        let response = req.send().await.context("Failed to send request to API")?;

        if response.status() == StatusCode::UNAUTHORIZED
            && self.refresh_token.is_some()
            && self.refresh_auth_token().await?
        {
            // Retry with new token
            let req = self.build_request(Method::GET, path);
            let response = req
                .send()
                .await
                .context("Failed to send request to API (retry)")?;
            return self.handle_bytes_response(response).await;
        }

        self.handle_bytes_response(response).await
    }

    /// Parse a binary response, extracting content type and optional filename.
    async fn handle_bytes_response(
        &self,
        response: reqwest::Response,
    ) -> Result<(Vec<u8>, String, Option<String>)> {
        let status = response.status();

        if status.is_success() {
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();

            let filename = response
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    // Parse filename from Content-Disposition: attachment; filename="name.ext"
                    v.split("filename=")
                        .nth(1)
                        .map(|f| f.trim_matches('"').trim_matches('\'').to_string())
                });

            let bytes = response
                .bytes()
                .await
                .context("Failed to read response bytes")?;

            Ok((bytes.to_vec(), content_type, filename))
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            if let Ok(api_error) = serde_json::from_str::<ApiError>(&error_text) {
                anyhow::bail!("API error ({}): {}", status, api_error.error);
            } else {
                anyhow::bail!("API error ({}): {}", status, error_text);
            }
        }
    }

    /// POST a multipart/form-data request with a file field and optional text fields.
    ///
    /// - `file_field_name`: the multipart field name for the file
    /// - `file_bytes`: raw bytes of the file content
    /// - `file_name`: filename hint sent in the Content-Disposition header
    /// - `mime_type`: MIME type of the file (e.g. `"application/gzip"`)
    /// - `extra_fields`: additional text key/value fields to include in the form
    pub async fn multipart_post<T: DeserializeOwned>(
        &mut self,
        path: &str,
        file_field_name: &str,
        file_bytes: Vec<u8>,
        file_name: &str,
        mime_type: &str,
        extra_fields: Vec<(&str, String)>,
    ) -> Result<T> {
        // Closure-like helper to build the multipart request from scratch.
        // We need this because reqwest::multipart::Form is not Clone, so we
        // must rebuild it for the retry attempt.
        let build_multipart_request =
            |client: &ApiClient, bytes: &[u8]| -> Result<reqwest::RequestBuilder> {
                let url = format!("{}/api/v1{}", client.base_url, path);

                let file_part = multipart::Part::bytes(bytes.to_vec())
                    .file_name(file_name.to_string())
                    .mime_str(mime_type)
                    .context("Invalid MIME type")?;

                let mut form = multipart::Form::new().part(file_field_name.to_string(), file_part);

                for (key, value) in &extra_fields {
                    form = form.text(key.to_string(), value.clone());
                }

                let mut req = client.client.post(&url).multipart(form);
                if let Some(token) = &client.auth_token {
                    req = req.bearer_auth(token);
                }
                Ok(req)
            };

        // First attempt
        let req = build_multipart_request(self, &file_bytes)?;
        let response = req
            .send()
            .await
            .context("Failed to send multipart request to API")?;

        if response.status() == StatusCode::UNAUTHORIZED
            && self.refresh_token.is_some()
            && self.refresh_auth_token().await?
        {
            // Retry with new token
            let req = build_multipart_request(self, &file_bytes)?;
            let response = req
                .send()
                .await
                .context("Failed to send multipart request to API (retry)")?;
            return self.handle_response(response).await;
        }

        self.handle_response(response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        matchers::{body_json, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[test]
    fn test_client_creation() {
        let client = ApiClient::new("http://localhost:8080".to_string(), None);
        assert_eq!(client.base_url, "http://localhost:8080");
        assert!(client.auth_token.is_none());
    }

    #[test]
    fn test_set_auth_token() {
        let mut client = ApiClient::new("http://localhost:8080".to_string(), None);
        assert!(client.auth_token.is_none());

        client.set_auth_token("test_token".to_string());
        assert_eq!(client.auth_token, Some("test_token".to_string()));

        client.clear_auth_token();
        assert!(client.auth_token.is_none());
    }

    #[test]
    fn test_url_for_api_path() {
        let client = ApiClient::new("http://localhost:8080".to_string(), None);
        assert_eq!(
            client.url_for("/actions"),
            "http://localhost:8080/api/v1/actions"
        );
    }

    #[test]
    fn test_url_for_auth_path() {
        let client = ApiClient::new("http://localhost:8080".to_string(), None);
        assert_eq!(
            client.url_for("/auth/login"),
            "http://localhost:8080/auth/login"
        );
    }

    #[tokio::test]
    async fn cache_put_unwraps_a_cache_data_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(
                "/api/v1/cache/namespaces/users/generations/7/chunks/0",
            ))
            .and(body_json(json!({
                "owner_type": "pack",
                "owner_ref": "salesforce",
                "entries": [{"external_id": "005xx", "value": {"name": "Ada"}}],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"generation_id": 7, "chunk_index": 0, "replayed": false}
            })))
            .mount(&server)
            .await;

        let mut client = ApiClient::new(server.uri(), None);
        let response: serde_json::Value = client
            .cache_put(
                "/cache/namespaces/users/generations/7/chunks/0",
                &json!({
                    "owner_type": "pack",
                    "owner_ref": "salesforce",
                    "entries": [{"external_id": "005xx", "value": {"name": "Ada"}}],
                }),
            )
            .await
            .unwrap();

        assert_eq!(response["generation_id"], 7);
        assert_eq!(response["chunk_index"], 0);
    }
}
