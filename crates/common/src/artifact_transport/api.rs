//! API-based artifact file transport.
//!
//! Transfers file content over HTTP to/from the API service's internal
//! file endpoints. Used by remote workers and sensors that do not share
//! a mounted volume with the API.

use async_trait::async_trait;
use futures::TryStreamExt;
use reqwest::Client;
use std::path::Path;
use std::sync::Arc;
use tokio_util::io::{ReaderStream, StreamReader};

use super::{ArtifactFileTransport, BoxAsyncReader, ValidatedRelativePath};
use crate::auth::WorkerTokenProvider;
use crate::blob_store::hash_file;
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

    fn completion_url(&self, file_path: &str) -> Result<String> {
        let file_url = self.file_url(file_path)?;
        Ok(file_url.replacen("/internal/files/", "/internal/artifacts/complete/", 1))
    }

    fn log_segment_url(&self, artifact_version: i64, sequence: i64) -> String {
        format!(
            "{}/api/v1/internal/logs/{artifact_version}/segments/{sequence}",
            self.base_url
        )
    }

    fn log_seal_url(&self, artifact_version: i64, truncated: bool) -> String {
        format!(
            "{}/api/v1/internal/logs/{artifact_version}/seal?truncated={truncated}",
            self.base_url
        )
    }

    async fn send_file(
        &self,
        url: &str,
        token: &str,
        source_path: &Path,
        size: u64,
        sha256: &str,
        content_type: &str,
    ) -> Result<reqwest::Response> {
        let file = tokio::fs::File::open(source_path).await.map_err(|error| {
            Error::Io(format!(
                "Failed to open local artifact '{}': {error}",
                source_path.display()
            ))
        })?;
        self.client
            .put(url)
            .bearer_auth(token)
            .header("Content-Type", content_type)
            .header(reqwest::header::CONTENT_LENGTH, size)
            .header("x-attune-sha256", sha256)
            .body(reqwest::Body::wrap_stream(ReaderStream::with_capacity(
                file,
                64 * 1024,
            )))
            .send()
            .await
            .map_err(|error| Error::Io(format!("API streamed upload failed: {error}")))
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

fn is_retryable_log_segment_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn expected_log_sequence(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    headers
        .get("x-attune-log-next-sequence")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
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
                "API write_file failed for {file_path}: HTTP {status} - {body}"
            )));
        }
        Ok(())
    }

    async fn write_file_from_path(
        &self,
        file_path: &str,
        source_path: &Path,
        content_type: Option<&str>,
    ) -> Result<u64> {
        let url = self.file_url(file_path)?;
        let (size, digest) = hash_file(source_path)
            .await
            .map_err(|error| Error::Io(error.to_string()))?;
        let digest = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let content_type = content_type.unwrap_or("application/octet-stream");
        let token = self.auth_token_source.token()?;
        let mut response = self
            .send_file(&url, &token, source_path, size, &digest, content_type)
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && self.auth_token_source.can_force_refresh()
        {
            let token = self.auth_token_source.force_refresh()?;
            response = self
                .send_file(&url, &token, source_path, size, &digest, content_type)
                .await?;
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API streamed upload failed for {file_path}: HTTP {status} - {body}"
            )));
        }
        Ok(size)
    }

    async fn commit_log_segment(
        &self,
        artifact_version: i64,
        sequence: i64,
        content: &[u8],
    ) -> Result<()> {
        let url = self.log_segment_url(artifact_version, sequence);
        let request_error = format!(
            "API log segment request failed for version {artifact_version} sequence {sequence}"
        );
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| {
                client
                    .put(&url)
                    .bearer_auth(token)
                    .header("Content-Type", "application/octet-stream")
                    .body(content.to_vec())
            },
            &request_error,
        )
        .await
        .map_err(|error| match error {
            Error::Io(message) => Error::retryable_transport(message),
            error => error,
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let expected_sequence = expected_log_sequence(resp.headers());
            let body = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::CONFLICT {
                if let Some(expected_sequence) = expected_sequence {
                    return Err(Error::log_sequence_conflict(expected_sequence));
                }
            }
            let message = format!(
                "API log segment failed for version {artifact_version} sequence {sequence}: HTTP {status} - {body}"
            );
            if is_retryable_log_segment_status(status) {
                return Err(Error::retryable_transport(message));
            }
            return Err(Error::Io(message));
        }
        Ok(())
    }

    async fn seal_log_stream(&self, artifact_version: i64, truncated: bool) -> Result<()> {
        let url = self.log_seal_url(artifact_version, truncated);
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| client.post(&url).bearer_auth(token),
            "API log seal request failed",
        )
        .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API log seal failed for version {artifact_version}: HTTP {status} - {body}"
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

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API file_exists failed for {file_path}: HTTP {status} - {body}"
            )));
        }

        Ok(true)
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
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API file_size failed for {file_path}: HTTP {status} - {body}"
            )));
        }

        let size = resp
            .headers()
            .get("Content-Length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        Ok(size)
    }

    async fn complete_file(&self, file_path: &str) -> Result<Option<u64>> {
        let url = self.completion_url(file_path)?;
        let request_error = format!("API complete_file request failed for {file_path}");
        let resp = send_with_auth_retry(
            &self.client,
            &self.auth_token_source,
            |client, token| client.post(&url).bearer_auth(token),
            &request_error,
        )
        .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API complete_file failed for {file_path}: HTTP {status} - {body}"
            )));
        }
        resp.headers()
            .get("x-attune-size")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Some)
            .ok_or_else(|| {
                Error::Io(format!(
                    "API complete_file returned no size for {file_path}"
                ))
            })
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

        // 404 is OK; the file is already gone.
        if resp.status() == reqwest::StatusCode::NOT_FOUND || resp.status().is_success() {
            return Ok(());
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(Error::Io(format!(
            "API delete_file failed for {file_path}: HTTP {status} - {body}"
        )))
    }

    async fn open_reader(&self, file_path: &str, offset: u64) -> Result<BoxAsyncReader> {
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

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound {
                entity: "file".to_string(),
                field: "path".to_string(),
                value: file_path.to_string(),
            });
        }
        if offset > 0 && resp.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(Box::pin(std::io::Cursor::new(Vec::new())));
        }
        if offset > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API open_reader ignored byte offset {offset} for {file_path}: HTTP {status} - {body}"
            )));
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Io(format!(
                "API open_reader failed for {file_path}: HTTP {status} - {body}"
            )));
        }

        let stream = resp.bytes_stream().map_err(std::io::Error::other);
        Ok(Box::pin(StreamReader::new(stream)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{crypto_provider, JwtConfig};
    use std::collections::VecDeque;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex;
    use tokio::time::{sleep, timeout, Duration};

    #[test]
    fn segment_retry_statuses_exclude_conflict_auth_and_validation() {
        assert!(is_retryable_log_segment_status(
            reqwest::StatusCode::REQUEST_TIMEOUT
        ));
        assert!(is_retryable_log_segment_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(is_retryable_log_segment_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(!is_retryable_log_segment_status(
            reqwest::StatusCode::CONFLICT
        ));
        assert!(!is_retryable_log_segment_status(
            reqwest::StatusCode::UNAUTHORIZED
        ));
        assert!(!is_retryable_log_segment_status(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY
        ));
    }

    #[test]
    fn log_sequence_conflict_header_is_typed() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-attune-log-next-sequence", "42".parse().unwrap());
        assert_eq!(expected_log_sequence(&headers), Some(42));
    }

    struct MockResponse {
        status: u16,
        body: &'static str,
        extra_headers: &'static str,
        delay: Duration,
    }

    impl MockResponse {
        fn new(status: u16, body: &'static str) -> Self {
            Self {
                status,
                body,
                extra_headers: "",
                delay: Duration::from_millis(0),
            }
        }

        fn with_delay(status: u16, body: &'static str, delay: Duration) -> Self {
            Self {
                status,
                body,
                extra_headers: "",
                delay,
            }
        }

        fn with_headers(mut self, headers: &'static str) -> Self {
            self.extra_headers = headers;
            self
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
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                    response.status,
                    status_text(response.status),
                    body.len(),
                    response.extra_headers,
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

    #[tokio::test]
    async fn streamed_file_upload_sends_size_digest_and_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind server");
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            let header_end = loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if let Some(position) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            while request.len() - header_end < content_length {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
            }
            stream
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            (
                headers,
                request[header_end..header_end + content_length].to_vec(),
            )
        });

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact.bin");
        tokio::fs::write(&path, b"streamed artifact").await.unwrap();
        let transport = ApiTransport::new(
            &format!("http://{address}"),
            "token",
            "/opt/attune/artifacts",
        );
        let size = transport
            .write_file_from_path("artifacts/1/v1", &path, None)
            .await
            .unwrap();
        let (headers, body) = server.await.unwrap();

        assert_eq!(size, body.len() as u64);
        assert_eq!(body, b"streamed artifact");
        assert!(headers.contains(
            "x-attune-sha256: f969919c655bc131af68e786fe54b20de79e12c4194284684af0e9e3de0bd394"
        ));
    }

    #[tokio::test]
    async fn metadata_requests_only_map_404_to_missing() {
        let (base_url, _, server_task) = spawn_mock_server(vec![
            MockResponse::new(404, "missing"),
            MockResponse::new(404, "missing"),
            MockResponse::new(500, "head failed"),
            MockResponse::new(403, "forbidden"),
        ])
        .await;
        let transport = ApiTransport::new(&base_url, "token", "/artifacts");

        assert!(!transport.file_exists("logs/missing.log").await.unwrap());
        assert_eq!(transport.file_size("logs/missing.log").await.unwrap(), None);
        assert!(transport.file_exists("logs/test.log").await.is_err());
        assert!(transport.file_size("logs/test.log").await.is_err());
        server_task.await.expect("mock server task");
    }

    #[tokio::test]
    async fn complete_file_calls_completion_endpoint_and_requires_verified_size() {
        let (base_url, _, server_task) = spawn_mock_server(vec![
            MockResponse::new(200, "").with_headers("x-attune-size: 0\r\n")
        ])
        .await;
        let transport = ApiTransport::new(&base_url, "token", "/artifacts");

        assert_eq!(
            transport.complete_file("logs/final.log").await.unwrap(),
            Some(0)
        );
        server_task.await.expect("mock server task");

        assert_eq!(
            transport.completion_url("logs/final.log").unwrap(),
            format!("{base_url}/api/v1/internal/artifacts/complete/logs/final.log")
        );
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
