//! Execution-only frontend. Hosts hold CP-issued dispatch/channel grants, never
//! the controller's CP bearer, Engine socket, private LaunchSpec or log keys.
use crate::workload_launch::*;
use async_trait::async_trait;
use bytes::Bytes;
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client, Response, StatusCode, Url,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt, time::Duration};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LaunchIoError {
    #[error("invalid fixed launch frontend configuration or grant")]
    Configuration,
    #[error("launch frontend transport failed")]
    Transport,
    #[error("launch frontend rejected request (HTTP {0})")]
    Rejected(u16),
    #[error("invalid or incomplete launch frontend response")]
    InvalidResponse,
}
pub type LaunchIoResult<T> = std::result::Result<T, LaunchIoError>;

/// Trusted CP/operator deployment policy, never deserialized from a pack or
/// frontend request. Plain HTTP does not establish an isolated engine boundary.
#[derive(Clone, Copy, Debug)]
pub enum LaunchHttpPolicy {
    TlsOrLoopback,
    DevelopmentPlainHttp,
}
impl LaunchHttpPolicy {
    pub fn allows_origin(self, url: &Url) -> bool {
        let loopback = url
            .host_str()
            .and_then(|host| {
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|address| address.is_loopback());
        url.scheme() == "https"
            || (url.scheme() == "http" && (matches!(self, Self::DevelopmentPlainHttp) || loopback))
    }
}

pub struct LaunchIoChunk {
    pub bytes: Bytes,
    pub next_offset: u64,
    pub truncated_bytes: u64,
    pub eof: bool,
}
impl fmt::Debug for LaunchIoChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchIoChunk")
            .field("length", &self.bytes.len())
            .field("next_offset", &self.next_offset)
            .field("truncated_bytes", &self.truncated_bytes)
            .field("eof", &self.eof)
            .finish()
    }
}
#[async_trait]
pub trait LaunchTransport: Send + Sync {
    async fn launch(&self, launch: &ReservedLaunch) -> LaunchIoResult<Box<dyn LaunchIoHandle>>;
}
#[async_trait]
pub trait LaunchIoHandle: Send + Sync {
    fn request(&self) -> &LaunchRequest;
    /// Exactly one bounded stdin body. A lost acknowledgement is not permission
    /// to replay parameters; the controller/CP one-shot reservation fences it.
    async fn write_stdin(&self, grant: &LaunchIoGrant, bytes: Bytes) -> LaunchIoResult<()>;
    async fn read(
        &self,
        grant: &LaunchIoGrant,
        offset: u64,
        max_bytes: u32,
    ) -> LaunchIoResult<LaunchIoChunk>;
    /// Finite authenticated readiness observation. Callers own readiness and
    /// assignment/grant refresh before the one-shot stdin delivery.
    async fn status(&self) -> LaunchIoResult<LaunchStatus>;
    async fn wait(&self) -> LaunchIoResult<LaunchStatus>;
    async fn cancel(&self, reason: LaunchStopReason) -> LaunchIoResult<LaunchStatus>;
    /// None means authenticated not-ready (Pending/Uploading/NotRequired), never
    /// provider absence. Denial and transport failures remain errors. A receipt
    /// exists only after CP durable publication, not merely local process exit.
    async fn stage_result(&self) -> LaunchIoResult<Option<LaunchStageResult>>;
}

/// The origin is selected by authenticated CP engine registration, not a pack
/// or caller's URL. It contains no authority. Credentials are per-call headers.
#[derive(Clone)]
pub struct HttpLaunchTransport {
    client: Client,
    base: Url,
}
impl HttpLaunchTransport {
    pub fn new(base: Url) -> LaunchIoResult<Self> {
        Self::with_policy(base, LaunchHttpPolicy::TlsOrLoopback)
    }
    pub fn with_policy(base: Url, policy: LaunchHttpPolicy) -> LaunchIoResult<Self> {
        if !policy.allows_origin(&base)
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.path() != "/"
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(LaunchIoError::Configuration);
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| LaunchIoError::Configuration)?;
        Ok(Self { client, base })
    }
    fn url(&self, id: i64, suffix: &[&str]) -> LaunchIoResult<Url> {
        if id <= 0 {
            return Err(LaunchIoError::Configuration);
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| LaunchIoError::Configuration)?
            .extend(["v1", "launches", &id.to_string()])
            .extend(suffix);
        Ok(url)
    }
    async fn dispatch<T: Serialize + ?Sized>(
        &self,
        launch: &ReservedLaunch,
        suffix: &[&str],
        body: &T,
    ) -> LaunchIoResult<Response> {
        self.client
            .post(self.url(launch.request.launch_id, suffix)?)
            .header(LAUNCH_DISPATCH_HEADER, secret(&launch.dispatch_token)?)
            .json(body)
            .send()
            .await
            .map_err(|_| LaunchIoError::Transport)
    }
}
#[async_trait]
impl LaunchTransport for HttpLaunchTransport {
    async fn launch(&self, launch: &ReservedLaunch) -> LaunchIoResult<Box<dyn LaunchIoHandle>> {
        let status: LaunchStatus =
            decode(self.dispatch(launch, &[], &launch.request).await?).await?;
        validate_status(&status, &launch.request)?;
        Ok(Box::new(HttpLaunchHandle {
            transport: self.clone(),
            launch: launch.clone(),
        }))
    }
}
struct HttpLaunchHandle {
    transport: HttpLaunchTransport,
    launch: ReservedLaunch,
}
impl HttpLaunchHandle {
    fn validate_grant(&self, grant: &LaunchIoGrant) -> LaunchIoResult<Duration> {
        if grant.request.launch_id != self.launch.request.launch_id
            || grant.request.admitted_digest != self.launch.request.admitted_digest
            || grant.max_bytes == 0
        {
            return Err(LaunchIoError::Configuration);
        }
        let remaining = (grant.expires_at - chrono::Utc::now())
            .to_std()
            .map_err(|_| LaunchIoError::Configuration)?;
        if remaining.is_zero() {
            return Err(LaunchIoError::Configuration);
        }
        Ok(remaining.min(Duration::from_secs(30)))
    }
}
#[async_trait]
impl LaunchIoHandle for HttpLaunchHandle {
    fn request(&self) -> &LaunchRequest {
        &self.launch.request
    }
    async fn write_stdin(&self, grant: &LaunchIoGrant, bytes: Bytes) -> LaunchIoResult<()> {
        let timeout = self.validate_grant(grant)?;
        if grant.channel != LaunchIoChannel::Stdin
            || bytes.len() > 1024 * 1024
            || bytes.len() as u64 > grant.max_bytes
        {
            return Err(LaunchIoError::Configuration);
        }
        let mut url = self.transport.url(self.request().launch_id, &["stdin"])?;
        url.query_pairs_mut()
            .append_pair("admitted_digest", self.request().admitted_digest.as_str());
        let response = self
            .transport
            .client
            .put(url)
            .header(LAUNCH_IO_HEADER, secret(&grant.token)?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .timeout(timeout)
            .body(bytes)
            .send()
            .await
            .map_err(|_| LaunchIoError::Transport)?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(LaunchIoError::Rejected(response.status().as_u16()));
        }
        Ok(())
    }
    async fn read(
        &self,
        grant: &LaunchIoGrant,
        offset: u64,
        max_bytes: u32,
    ) -> LaunchIoResult<LaunchIoChunk> {
        let timeout = self.validate_grant(grant)?;
        if max_bytes == 0 || max_bytes > 65536 || max_bytes as u64 > grant.max_bytes {
            return Err(LaunchIoError::Configuration);
        }
        let channel = match grant.channel {
            LaunchIoChannel::Stdout => "stdout",
            LaunchIoChannel::Stderr => "stderr",
            LaunchIoChannel::Stdin => return Err(LaunchIoError::Configuration),
        };
        let mut url = self.transport.url(self.request().launch_id, &[channel])?;
        url.query_pairs_mut()
            .append_pair("admitted_digest", self.request().admitted_digest.as_str())
            .append_pair("offset", &offset.to_string())
            .append_pair("max_bytes", &max_bytes.to_string());
        let response = self
            .transport
            .client
            .get(url)
            .header(LAUNCH_IO_HEADER, secret(&grant.token)?)
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| LaunchIoError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(LaunchIoError::Rejected(response.status().as_u16()));
        }
        let headers = response.headers();
        if single_header(headers, "content-type")? != "application/octet-stream" {
            return Err(LaunchIoError::InvalidResponse);
        }
        let next_offset = single_header(headers, LAUNCH_IO_NEXT_OFFSET_HEADER)?
            .parse::<u64>()
            .map_err(|_| LaunchIoError::InvalidResponse)?;
        let truncated_bytes = single_header(headers, LAUNCH_IO_TRUNCATED_HEADER)?
            .parse::<u64>()
            .map_err(|_| LaunchIoError::InvalidResponse)?;
        let eof = match single_header(headers, LAUNCH_IO_EOF_HEADER)? {
            "true" => true,
            "false" => false,
            _ => return Err(LaunchIoError::InvalidResponse),
        };
        let bytes = bounded_body(response, max_bytes as usize).await?;
        if offset
            .checked_add(truncated_bytes)
            .and_then(|value| value.checked_add(bytes.len() as u64))
            != Some(next_offset)
            || chrono::Utc::now() >= grant.expires_at
        {
            return Err(LaunchIoError::InvalidResponse);
        }
        Ok(LaunchIoChunk {
            bytes: bytes.into(),
            next_offset,
            truncated_bytes,
            eof,
        })
    }
    async fn status(&self) -> LaunchIoResult<LaunchStatus> {
        let status: LaunchStatus = decode(
            self.transport
                .dispatch(&self.launch, &["status"], self.request())
                .await?,
        )
        .await?;
        validate_status(&status, self.request())?;
        Ok(status)
    }
    async fn wait(&self) -> LaunchIoResult<LaunchStatus> {
        loop {
            let status = self.status().await?;
            if status.state == LaunchState::Stopped {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    async fn cancel(&self, reason: LaunchStopReason) -> LaunchIoResult<LaunchStatus> {
        let status = decode(
            self.transport
                .dispatch(
                    &self.launch,
                    &["cancel"],
                    &StopLaunchRequest {
                        request: self.request().clone(),
                        reason,
                    },
                )
                .await?,
        )
        .await?;
        validate_status(&status, self.request())?;
        if !matches!(
            status.state,
            LaunchState::StopRequired | LaunchState::Stopped
        ) {
            return Err(LaunchIoError::InvalidResponse);
        }
        Ok(status)
    }
    async fn stage_result(&self) -> LaunchIoResult<Option<LaunchStageResult>> {
        let response = self
            .transport
            .dispatch(&self.launch, &["stage-result"], self.request())
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let receipt: LaunchStageResult = decode(response).await?;
        if receipt.output_id <= 0
            || receipt.object_id <= 0
            || receipt.validation_receipt <= 0
            || receipt.object_version.is_empty()
            || receipt.size_bytes == 0
        {
            return Err(LaunchIoError::InvalidResponse);
        }
        Ok(Some(receipt))
    }
}
fn validate_status(status: &LaunchStatus, request: &LaunchRequest) -> LaunchIoResult<()> {
    status
        .validate_shape()
        .map_err(|_| LaunchIoError::InvalidResponse)?;
    if status.request.launch_id != request.launch_id
        || status.request.admitted_digest != request.admitted_digest
    {
        return Err(LaunchIoError::InvalidResponse);
    }
    Ok(())
}
fn secret(token: &LaunchToken) -> LaunchIoResult<HeaderValue> {
    let mut value =
        HeaderValue::from_str(token.as_str()).map_err(|_| LaunchIoError::Configuration)?;
    value.set_sensitive(true);
    Ok(value)
}
fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> LaunchIoResult<&'a str> {
    if headers.get_all(name).iter().count() != 1 {
        return Err(LaunchIoError::InvalidResponse);
    }
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or(LaunchIoError::InvalidResponse)
}
async fn decode<T: DeserializeOwned>(response: Response) -> LaunchIoResult<T> {
    if !response.status().is_success() {
        return Err(LaunchIoError::Rejected(response.status().as_u16()));
    }
    serde_json::from_slice(&bounded_body(response, 4 * 1024 * 1024).await?)
        .map_err(|_| LaunchIoError::InvalidResponse)
}
async fn bounded_body(mut response: Response, limit: usize) -> LaunchIoResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(LaunchIoError::InvalidResponse);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| LaunchIoError::Transport)?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(LaunchIoError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    fn reserved() -> ReservedLaunch {
        ReservedLaunch {
            request: LaunchRequest {
                launch_id: 71,
                admitted_digest: Sha256Digest::of_bytes(b"owned-manifest"),
            },
            dispatch_token: LaunchToken::new("owned-dispatch".into()).unwrap(),
        }
    }
    fn status(launch: &ReservedLaunch) -> LaunchStatus {
        LaunchStatus {
            request: launch.request.clone(),
            state: LaunchState::Reserved,
            runtime_state: LaunchRuntimeState::Unstarted,
            exit_outcome: None,
            resource: None,
            stop_reason: None,
            publication: LaunchPublicationState::NotRequired,
            capture: LaunchCaptureState::Pending,
            capture_deadline: None,
        }
    }
    fn grant(launch: &ReservedLaunch, channel: LaunchIoChannel) -> LaunchIoGrant {
        LaunchIoGrant {
            request: launch.request.clone(),
            channel,
            token: LaunchToken::new("owned-channel-token".into()).unwrap(),
            max_bytes: 1024,
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(1),
        }
    }
    async fn request(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        let headers = loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            assert!(bytes.len() <= 2 * 1024 * 1024);
            if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let head = String::from_utf8(bytes[..headers].to_vec()).unwrap();
        let length = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() < headers + length {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
        }
        (head, bytes[headers..headers + length].to_vec())
    }
    async fn json(socket: &mut tokio::net::TcpStream, value: &LaunchStatus) {
        let body = serde_json::to_vec(value).unwrap();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        socket.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn frontend_uses_only_target_dispatch_and_channel_grants() {
        let launch = reserved();
        let expected = launch.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, body) = request(&mut socket).await;
            assert!(headers.starts_with("POST /v1/launches/71 HTTP/1.1"));
            assert!(headers.contains("x-attune-launch-dispatch: owned-dispatch"));
            assert!(!headers.to_ascii_lowercase().contains("\nauthorization:"));
            assert_eq!(
                serde_json::from_slice::<LaunchRequest>(&body)
                    .unwrap()
                    .admitted_digest,
                expected.request.admitted_digest
            );
            json(&mut socket, &status(&expected)).await;
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, body) = request(&mut socket).await;
            assert!(headers.starts_with("POST /v1/launches/71/status HTTP/1.1"));
            assert!(headers.contains("x-attune-launch-dispatch: owned-dispatch"));
            assert!(!headers.to_ascii_lowercase().contains("\nauthorization:"));
            assert_eq!(
                serde_json::from_slice::<LaunchRequest>(&body).unwrap(),
                expected.request
            );
            json(&mut socket, &status(&expected)).await;
            let (mut socket, _) = listener.accept().await.unwrap();
            let (headers, body) = request(&mut socket).await;
            assert!(headers.starts_with("PUT /v1/launches/71/stdin?admitted_digest="));
            assert!(!headers
                .lines()
                .next()
                .unwrap()
                .contains("owned-channel-token"));
            assert!(headers.contains("x-attune-launch-io: owned-channel-token"));
            assert!(!headers.to_ascii_lowercase().contains("\nauthorization:"));
            assert!(!headers.contains("x-attune-launch-dispatch:"));
            assert_eq!(body, b"{\"owned_secret\":true}");
            socket
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        });
        let transport =
            HttpLaunchTransport::new(Url::parse(&format!("http://{address}/")).unwrap()).unwrap();
        let result = async {
            let handle = transport.launch(&launch).await?;
            assert_eq!(handle.status().await?.request, launch.request);
            handle
                .write_stdin(
                    &grant(&launch, LaunchIoChannel::Stdin),
                    Bytes::from_static(b"{\"owned_secret\":true}"),
                )
                .await
        }
        .await;
        server.await.unwrap();
        result.unwrap();
    }
    #[tokio::test]
    async fn finite_reads_reject_missing_bytes_and_ambiguous_headers() {
        let launch = reserved();
        let expected = launch.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            json(&mut socket, &status(&expected)).await;
            for headers in ["x-attune-launch-next-offset: 9\r\nx-attune-launch-truncated-bytes: 0\r\nx-attune-launch-eof: true", "x-attune-launch-next-offset: 4\r\nx-attune-launch-next-offset: 4\r\nx-attune-launch-truncated-bytes: 0\r\nx-attune-launch-eof: true"] {
                let (mut socket,_)=listener.accept().await.unwrap();let (head,_)=request(&mut socket).await;assert!(head.starts_with("GET /v1/launches/71/stdout?"));
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 4\r\n{headers}\r\nConnection: close\r\n\r\ndata").as_bytes()).await.unwrap();socket.shutdown().await.unwrap();
            }
        });
        let transport =
            HttpLaunchTransport::new(Url::parse(&format!("http://{address}/")).unwrap()).unwrap();
        let result = async {
            let handle = transport.launch(&launch).await?;
            for _ in 0..2 {
                assert!(handle
                    .read(&grant(&launch, LaunchIoChannel::Stdout), 0, 64)
                    .await
                    .is_err());
            }
            Ok::<_, LaunchIoError>(())
        }
        .await;
        server.await.unwrap();
        result.unwrap();
    }
    #[test]
    fn credentials_and_raw_bytes_are_not_debug_values_or_url_authority() {
        for origin in [
            "http://external-engine.example/",
            "http://localhost/",
            "http://loopback.example/",
            "http://user:password@localhost/",
            "http://localhost/?token=bad",
            "http://localhost/path",
            "file:///tmp/host",
        ] {
            assert!(HttpLaunchTransport::new(Url::parse(origin).unwrap()).is_err());
        }
        for origin in ["http://127.0.0.1/", "http://[::1]/"] {
            assert!(HttpLaunchTransport::new(Url::parse(origin).unwrap()).is_ok());
        }
        assert!(HttpLaunchTransport::with_policy(
            Url::parse("http://controller.dev/").unwrap(),
            LaunchHttpPolicy::DevelopmentPlainHttp
        )
        .is_ok());
        let token = LaunchToken::new("owned-secret".into()).unwrap();
        assert!(secret(&token).unwrap().is_sensitive());
        assert!(!format!("{:?}", secret(&token).unwrap()).contains("owned-secret"));
        let chunk = LaunchIoChunk {
            bytes: Bytes::from_static(b"private-output"),
            next_offset: 14,
            truncated_bytes: 0,
            eof: true,
        };
        assert!(!format!("{chunk:?}").contains("private-output"));
    }
    #[tokio::test]
    async fn stage_receipt_denial_or_transport_failure_is_not_not_ready() {
        let launch = reserved();
        let expected = launch.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            json(&mut socket, &status(&expected)).await;
            let (mut socket, _) = listener.accept().await.unwrap();
            let (head, _) = request(&mut socket).await;
            assert!(head.starts_with("POST /v1/launches/71/stage-result "));
            socket
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            socket.shutdown().await.unwrap();
        });
        let transport =
            HttpLaunchTransport::new(Url::parse(&format!("http://{address}/")).unwrap()).unwrap();
        let result = async {
            let handle = transport.launch(&launch).await?;
            assert!(matches!(
                handle.stage_result().await,
                Err(LaunchIoError::Rejected(403))
            ));
            assert!(handle.stage_result().await.is_err());
            Ok::<_, LaunchIoError>(())
        }
        .await;
        server.await.unwrap();
        result.unwrap();
    }
}
