//! API-based artifact file transport.
//!
//! Transfers file content over HTTP to/from the API service's internal
//! file endpoints. Used by remote workers and sensors that do not share
//! a mounted volume with the API.

use async_trait::async_trait;
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::{ArtifactFileTransport, BoxAsyncReader, BoxAsyncWriter, ValidatedRelativePath};
use crate::auth::WorkerTokenProvider;
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
enum AuthTokenSource {
    Static(String),
    WorkerProvider(Arc<WorkerTokenProvider>),
}

impl AuthTokenSource {
    fn token(&self) -> Result<String> {
        match self {
            Self::Static(token) => Ok(token.clone()),
            Self::WorkerProvider(provider) => provider
                .token()
                .map_err(|e| Error::Internal(format!("Failed to get worker auth token: {e}"))),
        }
    }

    fn can_force_refresh(&self) -> bool {
        matches!(self, Self::WorkerProvider(_))
    }

    fn force_refresh(&self) -> Result<String> {
        match self {
            Self::Static(token) => Ok(token.clone()),
            Self::WorkerProvider(provider) => provider
                .force_refresh()
                .map_err(|e| Error::Internal(format!("Failed to refresh worker auth token: {e}"))),
        }
    }
}

/// HTTP-based transport that calls internal file endpoints on the API.
#[derive(Debug, Clone)]
pub struct ApiTransport {
    base_url: String,
    auth_token_source: AuthTokenSource,
    artifacts_dir: String,
    client: Client,
}

impl ApiTransport {
    pub fn new(api_url: &str, auth_token: &str, artifacts_dir: &str) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            base_url: api_url.trim_end_matches('/').to_string(),
            auth_token_source: AuthTokenSource::Static(auth_token.to_string()),
            artifacts_dir: artifacts_dir.to_string(),
            client,
        }
    }

    pub fn new_with_worker_token_provider(
        api_url: &str,
        token_provider: Arc<WorkerTokenProvider>,
        artifacts_dir: &str,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            base_url: api_url.trim_end_matches('/').to_string(),
            auth_token_source: AuthTokenSource::WorkerProvider(token_provider),
            artifacts_dir: artifacts_dir.to_string(),
            client,
        }
    }

    /// Update the auth token (e.g., after token refresh).
    pub fn set_auth_token(&mut self, token: &str) {
        self.auth_token_source = AuthTokenSource::Static(token.to_string());
    }

    fn file_url(&self, file_path: &str) -> Result<String> {
        let file_path = ValidatedRelativePath::new(file_path)?;
        // Percent-encode each path segment individually
        use url::form_urlencoded;
        let encoded_path: String = file_path
            .as_str()
            .split('/')
            .map(|segment| form_urlencoded::byte_serialize(segment.as_bytes()).collect::<String>())
            .collect::<Vec<_>>()
            .join("/");
        Ok(format!(
            "{}/api/v1/internal/files/{}",
            self.base_url, encoded_path
        ))
    }
}

async fn send_with_auth_retry<F>(
    client: &Client,
    auth_token_source: &AuthTokenSource,
    build_request: F,
    request_error_context: &str,
) -> Result<reqwest::Response>
where
    F: Fn(&Client, &str) -> reqwest::RequestBuilder,
{
    let token = auth_token_source.token()?;
    let mut response = build_request(client, &token)
        .send()
        .await
        .map_err(|e| Error::Io(format!("{request_error_context}: {e}")))?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        && auth_token_source.can_force_refresh()
    {
        let refreshed_token = auth_token_source.force_refresh()?;
        response = build_request(client, &refreshed_token)
            .send()
            .await
            .map_err(|e| Error::Io(format!("{request_error_context}: {e}")))?;
    }

    Ok(response)
}

#[async_trait]
impl ArtifactFileTransport for ApiTransport {
    async fn write_file(
        &self,
        file_path: &str,
        content: &[u8],
        content_type: Option<&str>,
    ) -> Result<()> {
        let url = self.file_url(file_path)?;
        let ct = content_type.unwrap_or("application/octet-stream");
        let request_error = format!("API write_file request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| {
                client
                    .put(&url)
                    .bearer_auth(token)
                    .header("Content-Type", ct)
                    .body(content.to_vec())
            },
            &request_error,
        )
        .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API write_file failed for {file_path}: HTTP {status} — {body}"
            )));
        }
        Ok(())
    }

    async fn read_file(&self, file_path: &str) -> Result<Vec<u8>> {
        let url = self.file_url(file_path)?;
        let request_error = format!("API read_file request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| client.get(&url).bearer_auth(token),
            &request_error,
        )
        .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound {
                entity: "file".to_string(),
                field: "path".to_string(),
                value: file_path.to_string(),
            });
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API read_file failed for {file_path}: HTTP {status} — {body}"
            )));
        }

        resp.bytes().await.map(|b| b.to_vec()).map_err(|e| {
            Error::Io(format!(
                "API read_file body read failed for {file_path}: {e}"
            ))
        })
    }

    async fn append_file(&self, file_path: &str, content: &[u8]) -> Result<()> {
        let url = self.file_url(file_path)?;
        let request_error = format!("API append_file request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| {
                client
                    .patch(&url)
                    .bearer_auth(token)
                    .header("Content-Type", "application/octet-stream")
                    .body(content.to_vec())
            },
            &request_error,
        )
        .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API append_file failed for {file_path}: HTTP {status} — {body}"
            )));
        }
        Ok(())
    }

    async fn file_exists(&self, file_path: &str) -> Result<bool> {
        let url = self.file_url(file_path)?;
        let request_error = format!("API file_exists request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| client.head(&url).bearer_auth(token),
            &request_error,
        )
        .await?;

        Ok(resp.status().is_success())
    }

    async fn file_size(&self, file_path: &str) -> Result<Option<u64>> {
        let url = self.file_url(file_path)?;
        let request_error = format!("API file_size request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| client.head(&url).bearer_auth(token),
            &request_error,
        )
        .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Ok(None);
        }

        let size = resp
            .headers()
            .get("Content-Length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        Ok(size)
    }

    async fn delete_file(&self, file_path: &str) -> Result<()> {
        let url = self.file_url(file_path)?;
        let request_error = format!("API delete_file request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| client.delete(&url).bearer_auth(token),
            &request_error,
        )
        .await?;

        // 404 is OK — file already gone
        if resp.status() == reqwest::StatusCode::NOT_FOUND || resp.status().is_success() {
            return Ok(());
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(Error::Io(format!(
            "API delete_file failed for {file_path}: HTTP {status} — {body}"
        )))
    }

    async fn rename_file(&self, from: &str, to: &str) -> Result<()> {
        // API transport implements rename as read + write + delete
        // (no server-side rename endpoint to keep the API simple)
        let content = self.read_file(from).await?;
        self.write_file(to, &content, None).await?;
        self.delete_file(from).await?;
        Ok(())
    }

    async fn create_writer(&self, file_path: &str) -> Result<BoxAsyncWriter> {
        // Return a buffered writer that flushes to API via append calls.
        let writer = ApiBufferedWriter::new(
            self.client.clone(),
            self.file_url(file_path)?,
            self.auth_token_source.clone(),
            file_path.to_string(),
        );
        // Ensure file starts empty
        let _ = self.delete_file(file_path).await;
        Ok(Box::pin(writer))
    }

    async fn open_reader(&self, file_path: &str, offset: u64) -> Result<BoxAsyncReader> {
        // Download the full content starting from offset and wrap in a cursor
        let url = self.file_url(file_path)?;
        let request_error = format!("API open_reader request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| {
                let req = client.get(&url).bearer_auth(token);
                if offset > 0 {
                    req.header("Range", format!("bytes={offset}-"))
                } else {
                    req
                }
            },
            &request_error,
        )
        .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(Error::Io(format!(
                "API open_reader failed for {file_path}: HTTP {status}"
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| Error::Io(format!("API open_reader body read failed: {e}")))?;

        let cursor = std::io::Cursor::new(bytes.to_vec());
        Ok(Box::pin(cursor))
    }

    fn transport_mode(&self) -> &'static str {
        "api"
    }

    fn base_dir(&self) -> &str {
        &self.artifacts_dir
    }

    async fn ensure_parent_dirs(&self, file_path: &str) -> Result<()> {
        ValidatedRelativePath::new(file_path).map(|_| ())
    }
}

/// Buffered async writer that batches writes and flushes to API via PATCH/append.
///
/// Accumulates bytes in an internal buffer and flushes when the buffer exceeds
/// a threshold or when `shutdown` is called.
struct ApiBufferedWriter {
    client: Client,
    url: String,
    auth_token_source: AuthTokenSource,
    file_path: String,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl ApiBufferedWriter {
    fn new(
        client: Client,
        url: String,
        auth_token_source: AuthTokenSource,
        file_path: String,
    ) -> Self {
        Self {
            client,
            url,
            auth_token_source,
            file_path,
            buffer: Arc::new(Mutex::new(Vec::with_capacity(8192))),
        }
    }
}

impl std::fmt::Debug for ApiBufferedWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiBufferedWriter")
            .field("url", &self.url)
            .field("file_path", &self.file_path)
            .finish()
    }
}

const FLUSH_THRESHOLD: usize = 4096;

impl tokio::io::AsyncWrite for ApiBufferedWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let buffer = this.buffer.clone();

        let result = buffer.try_lock();
        match result {
            Ok(mut guard) => {
                guard.extend_from_slice(buf);
                let should_flush = guard.len() >= FLUSH_THRESHOLD;
                if should_flush {
                    let data = std::mem::take(&mut *guard);
                    drop(guard);
                    let client = this.client.clone();
                    let url = this.url.clone();
                    let auth_token_source = this.auth_token_source.clone();
                    let file_path = this.file_path.clone();
                    tokio::spawn(async move {
                        if let Err(e) = flush_to_api(&client, &url, &auth_token_source, &data).await
                        {
                            warn!("Failed to flush buffer to API for {file_path}: {e}");
                        }
                    });
                }
                std::task::Poll::Ready(Ok(buf.len()))
            }
            Err(_) => {
                // Lock contention — rare, just report as would-block
                std::task::Poll::Pending
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let buffer = this.buffer.clone();
        let result = buffer.try_lock();
        if let Ok(mut guard) = result {
            if !guard.is_empty() {
                let data = std::mem::take(&mut *guard);
                drop(guard);
                let client = this.client.clone();
                let url = this.url.clone();
                let auth_token_source = this.auth_token_source.clone();
                let file_path = this.file_path.clone();
                tokio::spawn(async move {
                    if let Err(e) = flush_to_api(&client, &url, &auth_token_source, &data).await {
                        warn!("Failed to flush final buffer to API for {file_path}: {e}");
                    }
                });
            }
        }
        std::task::Poll::Ready(Ok(()))
    }
}

async fn flush_to_api(
    client: &Client,
    url: &str,
    auth_token_source: &AuthTokenSource,
    data: &[u8],
) -> Result<()> {
    let resp = send_with_auth_retry(
        client,
        auth_token_source,
        |client, token| {
            client
                .patch(url)
                .bearer_auth(token)
                .header("Content-Type", "application/octet-stream")
                .body(data.to_vec())
        },
        "API flush request failed",
    )
    .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Io(format!(
            "API flush failed: HTTP {status} — {body}"
        )));
    }
    debug!("Flushed {} bytes to API", data.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{crypto_provider, JwtConfig};
    use std::collections::VecDeque;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{sleep, timeout, Duration};

    struct MockResponse {
        status: u16,
        body: &'static str,
        delay: Duration,
    }

    impl MockResponse {
        fn new(status: u16, body: &'static str) -> Self {
            Self {
                status,
                body,
                delay: Duration::from_millis(0),
            }
        }

        fn with_delay(status: u16, body: &'static str, delay: Duration) -> Self {
            Self {
                status,
                body,
                delay,
            }
        }
    }

    fn status_text(status: u16) -> &'static str {
        match status {
            200 => "OK",
            401 => "Unauthorized",
            500 => "Internal Server Error",
            _ => "Unknown",
        }
    }

    async fn spawn_mock_server(
        responses: Vec<MockResponse>,
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        let auth_headers = Arc::new(Mutex::new(Vec::new()));
        let captured_headers = auth_headers.clone();

        let handle = tokio::spawn(async move {
            let mut planned = VecDeque::from(responses);
            loop {
                if planned.is_empty() {
                    break;
                }

                let accept_result = timeout(Duration::from_millis(500), listener.accept()).await;
                let Ok(Ok((mut stream, _))) = accept_result else {
                    break;
                };

                let mut request = Vec::new();
                let mut buffer = vec![0_u8; 8192];
                loop {
                    match stream.read(&mut buffer).await {
                        Ok(0) => break,
                        Ok(read) => {
                            request.extend_from_slice(&buffer[..read]);
                            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => return,
                    }
                }

                let request_text = String::from_utf8_lossy(&request);
                if let Some(auth_line) = request_text.lines().find(|line| {
                    line.get(..14)
                        .map(|prefix| prefix.eq_ignore_ascii_case("authorization:"))
                        .unwrap_or(false)
                }) {
                    if let Some((_, value)) = auth_line.split_once(':') {
                        captured_headers.lock().await.push(value.trim().to_string());
                    }
                }

                let response = planned.pop_front().expect("planned response");
                if !response.delay.is_zero() {
                    sleep(response.delay).await;
                }

                let body = response.body.as_bytes();
                let header = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.status,
                    status_text(response.status),
                    body.len()
                );
                if stream.write_all(header.as_bytes()).await.is_err() {
                    return;
                }
                if !body.is_empty() && stream.write_all(body).await.is_err() {
                    return;
                }
                let _ = stream.shutdown().await;
            }
        });

        (format!("http://{}", addr), auth_headers, handle)
    }

    fn test_worker_provider() -> Arc<WorkerTokenProvider> {
        crypto_provider::install();
        Arc::new(WorkerTokenProvider::new_with_options(
            1,
            "artifact-transport-test",
            JwtConfig {
                secret: "artifact-transport-test-secret".to_string(),
                access_token_expiration: 3600,
                refresh_token_expiration: 604_800,
            },
            3600,
            0,
        ))
    }

    #[tokio::test]
    async fn write_file_retries_once_on_401_with_worker_token_provider() {
        let (base_url, auth_headers, server_task) = spawn_mock_server(vec![
            MockResponse::with_delay(401, "unauthorized", Duration::from_millis(1200)),
            MockResponse::new(200, "ok"),
        ])
        .await;

        let transport = ApiTransport::new_with_worker_token_provider(
            &base_url,
            test_worker_provider(),
            "/opt/attune/artifacts",
        );

        transport
            .write_file("logs/test.log", b"hello", Some("text/plain"))
            .await
            .expect("write_file should retry and succeed");

        let headers = auth_headers.lock().await.clone();
        assert_eq!(headers.len(), 2, "expected initial request and one retry");
        assert_ne!(
            headers[0], headers[1],
            "retry should use a force-refreshed worker token"
        );

        server_task.await.expect("mock server task");
    }

    #[tokio::test]
    async fn write_file_does_not_retry_on_401_with_static_token() {
        let (base_url, auth_headers, server_task) = spawn_mock_server(vec![
            MockResponse::new(401, "unauthorized"),
            MockResponse::new(200, "ok"),
        ])
        .await;

        let transport = ApiTransport::new(&base_url, "static-token", "/opt/attune/artifacts");
        let result = transport
            .write_file("logs/test.log", b"hello", Some("text/plain"))
            .await;

        assert!(result.is_err(), "static token transport should fail on 401");
        let headers = auth_headers.lock().await.clone();
        assert_eq!(headers.len(), 1, "static token mode should not retry");

        server_task.await.expect("mock server task");
    }

    #[tokio::test]
    async fn write_file_does_not_retry_non_auth_failures() {
        let (base_url, auth_headers, server_task) = spawn_mock_server(vec![
            MockResponse::new(500, "boom"),
            MockResponse::new(200, "ok"),
        ])
        .await;

        let transport = ApiTransport::new_with_worker_token_provider(
            &base_url,
            test_worker_provider(),
            "/opt/attune/artifacts",
        );
        let result = transport
            .write_file("logs/test.log", b"hello", Some("text/plain"))
            .await;

        assert!(
            result.is_err(),
            "500 responses should still fail immediately"
        );
        let headers = auth_headers.lock().await.clone();
        assert_eq!(
            headers.len(),
            1,
            "non-auth failures must preserve no-retry behavior"
        );

        server_task.await.expect("mock server task");
    }

    #[test]
    fn file_url_rejects_ambiguous_or_escaping_paths() {
        let transport = ApiTransport::new("http://localhost", "token", "/artifacts");
        assert!(transport.file_url("safe/path.txt").is_ok());
        for path in ["../escape", "/absolute", "a\\b", "a//b", "C:/escape"] {
            assert!(transport.file_url(path).is_err(), "{path}");
        }
    }
}
