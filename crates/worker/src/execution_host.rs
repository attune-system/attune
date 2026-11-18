//! Execution-only host consumer. No Config, database, MQ, signer or master key
//! is accepted here. The reusable enrollment bearer is dropped after startup;
//! every later control effect uses the bounded CP-issued host session.
use crate::{custody_control::HostSessionSlot, log_delivery_spool::LogDeliverySpool};
use attune_common::{
    execution_host_config::ExecutionHostConfig,
    workload_host::{
        AssignedAction, NextActionRequest, NextActionResponse, RegisterWorkloadHost,
        RegisteredWorkloadHost, WorkloadHostRole,
    },
    workload_launch::{
        LaunchIoChannel, LaunchIoGrant, LaunchIoGrantRequest, LaunchRuntimeState, LaunchState,
        LaunchStopReason, LaunchToken,
    },
    workload_launch_io::{HttpLaunchTransport, LaunchHttpPolicy, LaunchIoHandle, LaunchTransport},
    workload_session::{RenewWorkloadSession, WorkloadSession},
    Error, Result,
};
use chrono::Utc;
use serde::{de::DeserializeOwned, Serialize};
use tokio_util::sync::CancellationToken;

const MAX_ASSIGNMENT_RESPONSE: usize = 20 * 1024 * 1024;
fn unavailable() -> Error {
    Error::execution("Execution-only host control unavailable")
}
fn denied() -> Error {
    Error::PermissionDenied("Execution-only host authority unavailable".into())
}
fn origin(url: &reqwest::Url, policy: LaunchHttpPolicy) -> Result<()> {
    if !policy.allows_origin(url)
        || !(url.scheme() == "https" || matches!(policy, LaunchHttpPolicy::DevelopmentPlainHttp))
        || url.host_str().is_none()
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::configuration(
            "Execution host requires an admitted HTTPS origin",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct HostControlClient {
    origin: reqwest::Url,
    client: reqwest::Client,
    sessions: HostSessionSlot,
    policy: LaunchHttpPolicy,
}
async fn exchange<T: Serialize, R: DeserializeOwned>(
    client: &reqwest::Client,
    url: reqwest::Url,
    token: &LaunchToken,
    request: &T,
    limit: usize,
) -> Result<R> {
    let mut response = client
        .post(url)
        .bearer_auth(token.as_str())
        .json(request)
        .send()
        .await
        .map_err(|_| unavailable())?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(denied());
    }
    if response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        != Some("no-store")
        || response
            .content_length()
            .is_some_and(|size| size > limit as u64)
    {
        return Err(unavailable());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|n| n > limit)
        {
            return Err(unavailable());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| unavailable())
}
fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| unavailable())
}
impl HostControlClient {
    pub fn new(
        origin_url: reqwest::Url,
        sessions: HostSessionSlot,
        policy: LaunchHttpPolicy,
    ) -> Result<Self> {
        origin(&origin_url, policy)?;
        Ok(Self {
            origin: origin_url,
            client: client()?,
            sessions,
            policy,
        })
    }
    async fn post<T: Serialize, R: DeserializeOwned>(
        &self,
        suffix: &str,
        request: &T,
        limit: usize,
    ) -> Result<R> {
        let previous = self.sessions.current().await?;
        let value = exchange(
            &self.client,
            self.origin
                .join(&format!("api/v1/workload-hosts/{suffix}"))
                .map_err(|_| unavailable())?,
            &previous.token,
            request,
            limit,
        )
        .await?;
        if previous.expires_at <= Utc::now() {
            return Err(denied());
        }
        self.sessions.current().await?;
        Ok(value)
    }
    pub async fn renew(&self) -> Result<()> {
        let previous = self.sessions.current().await?;
        // Bind the HTTP request to the same previous token used by conditional
        // replacement. Concurrent renewals cannot overwrite newer credentials.
        let next: WorkloadSession = exchange(
            &self.client,
            self.origin
                .join("api/v1/workload-hosts/session")
                .map_err(|_| unavailable())?,
            &previous.token,
            &RenewWorkloadSession {},
            16384,
        )
        .await?;
        self.sessions.replace(&previous, next).await?;
        Ok(())
    }
    pub async fn next_action(&self, known_executions: Vec<i64>) -> Result<NextActionResponse> {
        if known_executions.len() > 1024 || known_executions.iter().any(|id| *id <= 0) {
            return Err(denied());
        }
        let result: NextActionResponse = self
            .post(
                "actions/next",
                &NextActionRequest { known_executions },
                MAX_ASSIGNMENT_RESPONSE,
            )
            .await?;
        if let NextActionResponse::Assigned { assignment } = &result {
            if assignment.execution <= 0
                || assignment.launch.request.launch_id <= 0
                || assignment.stdin_json.len()
                    > attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES
                || assignment.max_stdout_bytes == 0
                || assignment.max_stderr_bytes == 0
            {
                return Err(unavailable());
            }
            let selected =
                reqwest::Url::parse(&assignment.controller_origin).map_err(|_| unavailable())?;
            origin(&selected, self.policy)?;
            for (grant, channel) in [
                (&assignment.stdin_grant, LaunchIoChannel::Stdin),
                (&assignment.stdout_grant, LaunchIoChannel::Stdout),
                (&assignment.stderr_grant, LaunchIoChannel::Stderr),
            ] {
                if grant.request != assignment.launch.request
                    || grant.channel != channel
                    || grant.expires_at <= Utc::now()
                    || grant.max_bytes == 0
                    || grant.expires_at > Utc::now() + chrono::Duration::seconds(30)
                {
                    return Err(unavailable());
                }
            }
        }
        Ok(result)
    }
    pub async fn channel_grant(&self, request: LaunchIoGrantRequest) -> Result<LaunchIoGrant> {
        let grant: LaunchIoGrant = self.post("actions/io-grants", &request, 16384).await?;
        if grant.request != request.request
            || grant.channel != request.channel
            || grant.max_bytes == 0
            || grant.expires_at <= Utc::now()
            || grant.expires_at > Utc::now() + chrono::Duration::seconds(30)
        {
            return Err(unavailable());
        }
        Ok(grant)
    }
    /// Returns only after the actual controller is ready and one exact stdin
    /// request is acknowledged. Failure/unknown ACK triggers cancellation; this
    /// function never retries a possibly consumed write or infers absence.
    pub async fn start_action(
        &self,
        mut assignment: AssignedAction,
        stop: &CancellationToken,
    ) -> Result<StartedHostAction> {
        let selected =
            reqwest::Url::parse(&assignment.controller_origin).map_err(|_| unavailable())?;
        origin(&selected, self.policy)?;
        let transport =
            HttpLaunchTransport::with_policy(selected, self.policy).map_err(|_| unavailable())?;
        let handle = transport
            .launch(&assignment.launch)
            .await
            .map_err(|_| unavailable())?;
        let ready = async {
            loop {
                self.sessions.current().await?;
                let status = handle.status().await.map_err(|_| unavailable())?;
                if status.request != assignment.launch.request {
                    return Err(unavailable());
                }
                if status.state == LaunchState::Created
                    && status.runtime_state == LaunchRuntimeState::Running
                {
                    break;
                }
                if matches!(
                    status.state,
                    LaunchState::Stopped | LaunchState::StopRequired | LaunchState::CleanupRequired
                ) {
                    return Err(denied());
                }
                tokio::select! {_=stop.cancelled()=>return Err(denied()),_=tokio::time::sleep(std::time::Duration::from_millis(100))=>{}}
            }
            let grant = self
                .channel_grant(LaunchIoGrantRequest {
                    request: assignment.launch.request.clone(),
                    channel: LaunchIoChannel::Stdin,
                })
                .await?;
            if assignment.stdin_json.len() as u64 > grant.max_bytes {
                return Err(denied());
            }
            self.sessions.current().await?;
            // Move this exact immutable String into the HTTP body once. No
            // parameter/env/file re-read can splice different bytes after auth.
            handle
                .write_stdin(&grant, std::mem::take(&mut assignment.stdin_json).into())
                .await
                .map_err(|_| unavailable())?;
            Ok(())
        };
        let outcome = tokio::select! {_=stop.cancelled()=>Err(denied()),result=ready=>result};
        if let Err(error) = outcome {
            let _ = handle
                .cancel(if stop.is_cancelled() {
                    LaunchStopReason::Shutdown
                } else {
                    LaunchStopReason::LaunchFailure
                })
                .await;
            return Err(error);
        }
        Ok(StartedHostAction { assignment, handle })
    }
}
/// This is intentionally not an execution-completion receipt. Raw capture,
/// durable custody and fenced CP completion are separate required consumers.
pub struct StartedHostAction {
    pub assignment: AssignedAction,
    pub handle: Box<dyn LaunchIoHandle>,
}

pub struct EnrolledExecutionHost {
    pub registration: RegisteredWorkloadHost,
    pub control: HostControlClient,
    pub custody: LogDeliverySpool,
}
impl EnrolledExecutionHost {
    /// Hold exclusive custody ownership before even the registration request.
    /// No general Config is loaded and no local signing key is constructed.
    pub async fn enroll(config: ExecutionHostConfig) -> Result<Self> {
        origin(&config.api_origin, config.http_policy)?;
        let owner = LogDeliverySpool::acquire_owner(
            config.custody_root,
            512 * 1024 * 1024,
            64 * 1024 * 1024,
            32 * 1024 * 1024,
        )
        .await?;
        let instance = uuid::Uuid::new_v4();
        let request = RegisterWorkloadHost {
            instance,
            custody_instance: uuid::Uuid::new_v4(),
        };
        let registered: RegisteredWorkloadHost = exchange(
            &client()?,
            config
                .api_origin
                .join("api/v1/workload-hosts/register")
                .map_err(|_| unavailable())?,
            &config.enrollment,
            &request,
            16384,
        )
        .await?;
        if registered.worker <= 0
            || registered.instance != instance
            || registered.registration_generation <= 0
            || !matches!(registered.profile.role, WorkloadHostRole::Action)
        {
            return Err(denied());
        }
        registered.profile.engine.validate_shape()?;
        let controller = reqwest::Url::parse(&registered.profile.controller_origin)
            .map_err(|_| unavailable())?;
        origin(&controller, config.http_policy)?;
        let sessions = HostSessionSlot::new(registered.session.clone())?;
        let custody = owner.bind_worker(registered.worker)?;
        let control = HostControlClient::new(config.api_origin, sessions, config.http_policy)?;
        // config.enrollment is not retained in the manager/control objects.
        Ok(Self {
            registration: registered,
            control,
            custody,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_host::HostExecutionProfile;
    use attune_common::workload_launch::{
        LaunchEngineBinding, LaunchPlatform, PinnedImage, Sha256Digest,
    };
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    #[derive(Clone)]
    struct Wire {
        root: std::path::PathBuf,
        locked: Arc<AtomicBool>,
        refreshed: Arc<AtomicBool>,
        host_token: Arc<tokio::sync::Mutex<String>>,
    }
    async fn register(
        State(wire): State<Wire>,
        headers: HeaderMap,
        Json(request): Json<RegisterWorkloadHost>,
    ) -> std::result::Result<(HeaderMap, Json<RegisteredWorkloadHost>), StatusCode> {
        if headers.get("authorization").and_then(|v| v.to_str().ok())
            != Some("Bearer owned-enrollment")
        {
            return Err(StatusCode::FORBIDDEN);
        }
        wire.locked.store(
            LogDeliverySpool::acquire_owner(
                wire.root,
                512 * 1024 * 1024,
                64 * 1024 * 1024,
                32 * 1024 * 1024,
            )
            .await
            .is_err(),
            Ordering::SeqCst,
        );
        let result = RegisteredWorkloadHost {
            worker: 41,
            registration_generation: 1,
            instance: request.instance,
            session: WorkloadSession {
                token: LaunchToken::new(wire.host_token.lock().await.clone())
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                expires_at: Utc::now() + chrono::Duration::seconds(299),
            },
            profile: HostExecutionProfile {
                role: WorkloadHostRole::Action,
                engine: LaunchEngineBinding {
                    engine_id: 2,
                    daemon_id: "owned-daemon".into(),
                    platform: LaunchPlatform::LinuxAmd64,
                },
                image: PinnedImage {
                    reference: format!(
                        "sha256:{}",
                        Sha256Digest::of_bytes(b"owned-image").as_str()
                    ),
                    sha256: Sha256Digest::of_bytes(b"owned-image"),
                },
                controller_origin: "http://127.0.0.1:1/".into(),
                max_concurrent: 1,
            },
        };
        Ok((
            HeaderMap::from_iter([(reqwest::header::CACHE_CONTROL, "no-store".parse().unwrap())]),
            Json(result),
        ))
    }
    async fn next(
        State(wire): State<Wire>,
        headers: HeaderMap,
        Json(request): Json<NextActionRequest>,
    ) -> std::result::Result<(HeaderMap, Json<NextActionResponse>), StatusCode> {
        let expected = format!("Bearer {}", wire.host_token.lock().await.as_str());
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str())
            || !request.known_executions.is_empty()
        {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok((
            HeaderMap::from_iter([(reqwest::header::CACHE_CONTROL, "no-store".parse().unwrap())]),
            Json(NextActionResponse::Idle),
        ))
    }
    async fn renew(
        State(wire): State<Wire>,
        headers: HeaderMap,
        Json(_): Json<RenewWorkloadSession>,
    ) -> std::result::Result<(HeaderMap, Json<WorkloadSession>), StatusCode> {
        let mut current = wire.host_token.lock().await;
        let expected = format!("Bearer {}", current.as_str());
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
            return Err(StatusCode::FORBIDDEN);
        }
        *current = "owned-renewed-host-session".into();
        wire.refreshed.store(true, Ordering::SeqCst);
        let value = WorkloadSession {
            token: LaunchToken::new(current.clone())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
            expires_at: Utc::now() + chrono::Duration::seconds(299),
        };
        Ok((
            HeaderMap::from_iter([(reqwest::header::CACHE_CONTROL, "no-store".parse().unwrap())]),
            Json(value),
        ))
    }
    #[tokio::test]
    async fn enrollment_owns_custody_before_http_and_later_calls_only_use_refreshable_host_sessions(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        attune_common::auth::install_crypto_provider();
        let root = tempfile::tempdir()?;
        let wire = Wire {
            root: root.path().join("custody"),
            locked: Arc::new(AtomicBool::new(false)),
            refreshed: Arc::new(AtomicBool::new(false)),
            host_token: Arc::new(tokio::sync::Mutex::new("owned-host-session".into())),
        };
        let app = Router::new()
            .route("/api/v1/workload-hosts/register", post(register))
            .route("/api/v1/workload-hosts/session", post(renew))
            .route("/api/v1/workload-hosts/actions/next", post(next))
            .with_state(wire.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin_url = reqwest::Url::parse(&format!("http://{}/", listener.local_addr()?))?;
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopping.cancelled_owned())
                .await
        });
        let result = async {
            let enrolled = EnrolledExecutionHost::enroll(ExecutionHostConfig {
                api_origin: origin_url.clone(),
                enrollment: LaunchToken::new("owned-enrollment".into())?,
                custody_root: wire.root.clone(),
                http_policy: LaunchHttpPolicy::DevelopmentPlainHttp,
            })
            .await?;
            let clone = enrolled.control.clone();
            let first = enrolled.control.next_action(Vec::new()).await?;
            enrolled.control.renew().await?;
            let second = clone.next_action(Vec::new()).await?;
            let lock = LogDeliverySpool::acquire_owner(
                wire.root.clone(),
                512 * 1024 * 1024,
                64 * 1024 * 1024,
                32 * 1024 * 1024,
            )
            .await;
            let strict = HostControlClient::new(
                origin_url,
                enrolled.control.sessions.clone(),
                LaunchHttpPolicy::TlsOrLoopback,
            );
            let facts = (
                matches!(first, NextActionResponse::Idle),
                matches!(second, NextActionResponse::Idle),
                lock.is_err(),
                strict.is_err(),
            );
            drop(enrolled);
            drop(clone);
            let owner = LogDeliverySpool::acquire_owner(
                wire.root.clone(),
                512 * 1024 * 1024,
                64 * 1024 * 1024,
                32 * 1024 * 1024,
            )
            .await?;
            drop(owner);
            Ok::<_, Box<dyn std::error::Error>>(facts)
        }
        .await;
        stop.cancel();
        server.await??;
        let facts = result?;
        assert_eq!(facts, (true, true, true, true));
        assert!(wire.locked.load(Ordering::SeqCst));
        assert!(wire.refreshed.load(Ordering::SeqCst));
        Ok(())
    }
    #[derive(Clone)]
    struct StartWire {
        request: attune_common::workload_launch::LaunchRequest,
        observed: Arc<AtomicBool>,
        granted: Arc<AtomicBool>,
        writes: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
        cancellations: Arc<std::sync::atomic::AtomicUsize>,
        fail_after_write: bool,
    }
    fn wire_status(
        wire: &StartWire,
        running: bool,
        cancelled: bool,
    ) -> attune_common::workload_launch::LaunchStatus {
        use attune_common::workload_launch::*;
        LaunchStatus {
            request: wire.request.clone(),
            state: if cancelled {
                LaunchState::StopRequired
            } else if running {
                LaunchState::Created
            } else {
                LaunchState::Reserved
            },
            runtime_state: if running {
                LaunchRuntimeState::Running
            } else {
                LaunchRuntimeState::Unstarted
            },
            exit_outcome: None,
            capture: LaunchCaptureState::Pending,
            capture_deadline: None,
            publication: LaunchPublicationState::NotRequired,
            resource: running.then(|| EngineResourceEvidence {
                engine_id: 2,
                engine_name_nonce: uuid::Uuid::new_v4(),
                resource_id: "owned-container".into(),
                admitted_digest: wire.request.admitted_digest.clone(),
                public_manifest_digest: Sha256Digest::of_bytes(b"owned-policy"),
            }),
            stop_reason: cancelled.then_some(LaunchStopReason::LaunchFailure),
        }
    }
    fn dispatch(headers: &HeaderMap) -> bool {
        headers.get("authorization").is_none()
            && headers
                .get(attune_common::workload_launch::LAUNCH_DISPATCH_HEADER)
                .and_then(|v| v.to_str().ok())
                == Some("owned-dispatch")
    }
    async fn launch(
        State(wire): State<StartWire>,
        headers: HeaderMap,
        Json(request): Json<attune_common::workload_launch::LaunchRequest>,
    ) -> std::result::Result<Json<attune_common::workload_launch::LaunchStatus>, StatusCode> {
        if !dispatch(&headers) || request != wire.request {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(Json(wire_status(&wire, false, false)))
    }
    async fn status(
        State(wire): State<StartWire>,
        headers: HeaderMap,
        Json(request): Json<attune_common::workload_launch::LaunchRequest>,
    ) -> std::result::Result<Json<attune_common::workload_launch::LaunchStatus>, StatusCode> {
        if !dispatch(&headers) || request != wire.request {
            return Err(StatusCode::FORBIDDEN);
        }
        wire.observed.store(true, Ordering::SeqCst);
        Ok(Json(wire_status(&wire, true, false)))
    }
    async fn grant(
        State(wire): State<StartWire>,
        headers: HeaderMap,
        Json(request): Json<LaunchIoGrantRequest>,
    ) -> std::result::Result<(HeaderMap, Json<LaunchIoGrant>), StatusCode> {
        if headers.get("authorization").and_then(|v| v.to_str().ok())
            != Some("Bearer owned-host-session")
            || !wire.observed.load(Ordering::SeqCst)
            || request.request != wire.request
            || request.channel != LaunchIoChannel::Stdin
        {
            return Err(StatusCode::FORBIDDEN);
        }
        wire.granted.store(true, Ordering::SeqCst);
        Ok((
            HeaderMap::from_iter([(reqwest::header::CACHE_CONTROL, "no-store".parse().unwrap())]),
            Json(LaunchIoGrant {
                request: wire.request,
                channel: request.channel,
                token: LaunchToken::new("owned-fresh-stdin".into()).unwrap(),
                max_bytes: 8 * 1024 * 1024,
                expires_at: Utc::now() + chrono::Duration::seconds(29),
            }),
        ))
    }
    async fn stdin(
        State(wire): State<StartWire>,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> StatusCode {
        if headers.get("authorization").is_some()
            || !wire.granted.load(Ordering::SeqCst)
            || headers
                .get(attune_common::workload_launch::LAUNCH_IO_HEADER)
                .and_then(|v| v.to_str().ok())
                != Some("owned-fresh-stdin")
        {
            return StatusCode::FORBIDDEN;
        }
        wire.writes.lock().await.push(body.to_vec());
        if wire.fail_after_write {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::NO_CONTENT
        }
    }
    async fn cancel(
        State(wire): State<StartWire>,
        headers: HeaderMap,
        Json(request): Json<attune_common::workload_launch::StopLaunchRequest>,
    ) -> std::result::Result<Json<attune_common::workload_launch::LaunchStatus>, StatusCode> {
        if !dispatch(&headers) || request.request != wire.request {
            return Err(StatusCode::FORBIDDEN);
        }
        wire.cancellations.fetch_add(1, Ordering::SeqCst);
        Ok(Json(wire_status(&wire, true, true)))
    }
    #[tokio::test]
    async fn action_waits_for_running_refreshes_stdin_and_never_retries_an_uncertain_write(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        use attune_common::workload_launch::{LaunchRequest, ReservedLaunch};
        attune_common::auth::install_crypto_provider();
        for fail_after_write in [false, true] {
            let request = LaunchRequest {
                launch_id: 9,
                admitted_digest: Sha256Digest::of_bytes(b"owned-launch"),
            };
            let wire = StartWire {
                request: request.clone(),
                observed: Arc::new(AtomicBool::new(false)),
                granted: Arc::new(AtomicBool::new(false)),
                writes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                cancellations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                fail_after_write,
            };
            let app = Router::new()
                .route("/v1/launches/{id}", post(launch))
                .route("/v1/launches/{id}/status", post(status))
                .route("/v1/launches/{id}/stdin", axum::routing::put(stdin))
                .route("/v1/launches/{id}/cancel", post(cancel))
                .route("/api/v1/workload-hosts/actions/io-grants", post(grant))
                .with_state(wire.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let url = reqwest::Url::parse(&format!("http://{}/", listener.local_addr()?))?;
            let stop = CancellationToken::new();
            let stopping = stop.clone();
            let server = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(stopping.cancelled_owned())
                    .await
            });
            let result = async {
                let client = HostControlClient::new(
                    url.clone(),
                    HostSessionSlot::new(WorkloadSession {
                        token: LaunchToken::new("owned-host-session".into())?,
                        expires_at: Utc::now() + chrono::Duration::seconds(299),
                    })?,
                    LaunchHttpPolicy::DevelopmentPlainHttp,
                )?;
                let original = br#"{"message":"owned-parameter"}"#.to_vec();
                let stale = |channel| LaunchIoGrant {
                    request: request.clone(),
                    channel,
                    token: LaunchToken::new("owned-expired-grant".into()).unwrap(),
                    expires_at: Utc::now() - chrono::Duration::seconds(1),
                    max_bytes: 8 * 1024 * 1024,
                };
                let assignment = AssignedAction {
                    execution: 1,
                    launch: ReservedLaunch {
                        request: request.clone(),
                        dispatch_token: LaunchToken::new("owned-dispatch".into())?,
                    },
                    controller_origin: url.to_string(),
                    stdin_json: String::from_utf8(original.clone())?,
                    stdin_grant: stale(LaunchIoChannel::Stdin),
                    stdout_grant: stale(LaunchIoChannel::Stdout),
                    stderr_grant: stale(LaunchIoChannel::Stderr),
                    max_stdout_bytes: 1024,
                    max_stderr_bytes: 1024,
                };
                let started = client
                    .start_action(assignment, &CancellationToken::new())
                    .await;
                let accepted = started.is_ok();
                if let Ok(started) = started {
                    if !started.assignment.stdin_json.is_empty() {
                        return Err("stdin retained after owned write".into());
                    }
                }
                Ok::<_, Box<dyn std::error::Error>>((accepted, original))
            }
            .await;
            stop.cancel();
            server.await??;
            let (accepted, original) = result?;
            assert_eq!(accepted, !fail_after_write);
            assert_eq!(*wire.writes.lock().await, vec![original]);
            assert_eq!(
                wire.cancellations.load(Ordering::SeqCst),
                usize::from(fail_after_write)
            );
        }
        Ok(())
    }
}
