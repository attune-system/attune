//! Controller frontend. Dispatch and channel grants are forwarded to CP for
//! every authority decision. There is no raw-spec or provider-selector route.
use crate::{
    actuator::{Actuator, ActuatorError},
    control_plane::ControlError,
    run::RunSession,
};
use attune_common::workload_launch::*;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::{sync::Mutex, task::JoinHandle};

pub struct ServiceLimits {
    pub max_active: usize,
    pub max_retained: usize,
    pub capture_bytes_per_stream: usize,
    pub minimum_free_bytes: u64,
}
struct Entry {
    session: Arc<RunSession>,
    reserved: ReservedLaunch,
    task: JoinHandle<()>,
    completed: Arc<Mutex<Option<DateTime<Utc>>>>,
    retryable: Arc<std::sync::atomic::AtomicBool>,
}
pub struct LaunchService {
    actuator: Arc<Actuator>,
    limits: ServiceLimits,
    entries: Mutex<BTreeMap<i64, Entry>>,
    closing: std::sync::atomic::AtomicBool,
}
impl LaunchService {
    pub fn new(actuator: Arc<Actuator>, limits: ServiceLimits) -> Result<Arc<Self>, &'static str> {
        if limits.max_active == 0
            || limits.max_active > 64
            || limits.max_retained < limits.max_active
            || limits.max_retained > 256
            || limits.capture_bytes_per_stream == 0
            || limits.capture_bytes_per_stream > 64 * 1024 * 1024
        {
            return Err("invalid controller admission/capture ceiling");
        }
        Ok(Arc::new(Self {
            actuator,
            limits,
            entries: Mutex::new(BTreeMap::new()),
            closing: false.into(),
        }))
    }
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/v1/launches/{id}", post(launch))
            .route("/v1/launches/{id}/status", post(status))
            .route("/v1/launches/{id}/cancel", post(cancel))
            .route("/v1/launches/{id}/stage-result", post(stage_result))
            .route(
                "/v1/launches/{id}/stdin",
                put(stdin).layer(DefaultBodyLimit::max(
                    attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES,
                )),
            )
            .route("/v1/launches/{id}/{channel}", get(read))
            .layer(DefaultBodyLimit::max(1024 * 1024))
            .with_state(self)
    }
    /// Discovery is controller-authenticated and independent of assignments.
    /// Every round restarts after=None; unfinished launches are never consumed.
    pub async fn reconcile_round(&self) -> Result<(), ActuatorError> {
        let mut after = None;
        loop {
            let page = self
                .actuator
                .cp
                .reconcile_page(&ReconcileLaunchesRequest { after, limit: 100 })
                .await?;
            for launch in page.items {
                // Claim performs the fresh authority/lease checks. Admission
                // saturation is retried next round, never marked stopped.
                let _scheduled = self.schedule(launch).await;
            }
            after = page.next_cursor;
            if after.is_none() {
                return Ok(());
            }
        }
    }
    async fn schedule(&self, launch: ReservedLaunch) -> Result<(), FrontError> {
        use std::sync::atomic::Ordering;
        if self.closing.load(Ordering::Acquire) {
            return Err(FrontError(StatusCode::SERVICE_UNAVAILABLE));
        }
        let mut entries = self.entries.lock().await;
        // Closing may have begun while this admission waited for the owner
        // lock. Shutdown must not drain the map before a late insertion.
        if self.closing.load(Ordering::Acquire) {
            return Err(FrontError(StatusCode::SERVICE_UNAVAILABLE));
        }
        // Remove only completed tasks after the bounded raw-capture lifetime.
        // This never removes an Engine resource or CP ledger obligation.
        let mut retired = Vec::new();
        for (&id, entry) in entries.iter() {
            if entry.task.is_finished()
                && entry
                    .completed
                    .lock()
                    .await
                    .is_none_or(|at| Utc::now() >= at)
            {
                retired.push(id);
            }
        }
        for id in retired {
            if let Some(entry) = entries.remove(&id) {
                entry.session.fail_capture();
                let _joined = entry.task.await;
            }
        }
        if let Some(entry) = entries.get(&launch.request.launch_id) {
            if entry.session.request() != &launch.request {
                return Err(FrontError(StatusCode::FORBIDDEN));
            }
            if !entry.task.is_finished() || !entry.retryable.load(Ordering::Acquire) {
                return Ok(());
            }
        }
        if (!entries.contains_key(&launch.request.launch_id)
            && entries.len() >= self.limits.max_retained)
            || entries
                .values()
                .filter(|entry| !entry.task.is_finished())
                .count()
                >= self.limits.max_active
        {
            return Err(FrontError(StatusCode::SERVICE_UNAVAILABLE));
        }
        let (session, completed) = if let Some(previous) = entries.remove(&launch.request.launch_id)
        {
            let _joined = previous.task.await;
            (previous.session, previous.completed)
        } else {
            (
                Arc::new(
                    RunSession::new(launch.request.clone(), self.limits.capture_bytes_per_stream)
                        .map_err(|_| FrontError(StatusCode::INTERNAL_SERVER_ERROR))?,
                ),
                Arc::new(Mutex::new(None)),
            )
        };
        let running = session.clone();
        let reserved = launch.clone();
        let actuator = self.actuator.clone();
        let minimum_free = self.limits.minimum_free_bytes;
        let deadline = completed.clone();
        let retryable = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let failed = retryable.clone();
        // Retiring completed entries above can await their owner. Recheck at
        // the final insertion boundary; no await separates this from spawn and
        // insertion under the same entries lock drained by shutdown.
        if self.closing.load(Ordering::Acquire) {
            return Err(FrontError(StatusCode::SERVICE_UNAVAILABLE));
        }
        let task = tokio::spawn(async move {
            let result = actuator.run(&reserved, &running, minimum_free).await;
            let until = match result {
                Ok(status) => status.capture_deadline.unwrap_or_else(Utc::now),
                Err(_) => {
                    failed.store(true, std::sync::atomic::Ordering::Release);
                    // No private invocation/error bodies in service diagnostics.
                    eprintln!("launch reconciliation remains incomplete; durable CP discovery retains cleanup");
                    Utc::now() + chrono::Duration::seconds(60)
                }
            };
            let mut deadline = deadline.lock().await;
            let until = until.min(Utc::now() + chrono::Duration::seconds(60));
            *deadline = Some(deadline.map_or(until, |previous| previous.min(until)));
        });
        entries.insert(
            launch.request.launch_id,
            Entry {
                session,
                reserved: launch,
                task,
                completed,
                retryable,
            },
        );
        Ok(())
    }
    async fn session(&self, request: &LaunchRequest) -> Result<Arc<RunSession>, FrontError> {
        let entries = self.entries.lock().await;
        let entry = entries
            .get(&request.launch_id)
            .ok_or(FrontError(StatusCode::CONFLICT))?;
        if entry.session.request() != request {
            return Err(FrontError(StatusCode::FORBIDDEN));
        }
        let session = entry.session.clone();
        let reserved = entry.reserved.clone();
        let finished = entry.task.is_finished();
        drop(entries);
        if finished {
            session
                .maintain_retained_io(&self.actuator.cp, &reserved)
                .await?;
        }
        Ok(session)
    }
    /// Stop admission, close authority, then join every lifecycle owner. No
    /// detached task is aborted while an Engine create/start may be uncertain.
    pub async fn shutdown(&self) {
        use std::sync::atomic::Ordering;
        self.closing.store(true, Ordering::Release);
        let entries = std::mem::take(&mut *self.entries.lock().await);
        for entry in entries.values() {
            let _closed = self
                .actuator
                .cp
                .stop(&entry.reserved, LaunchStopReason::Shutdown)
                .await;
        }
        for (_, entry) in entries {
            let _joined = entry.task.await;
            entry.session.fail_capture();
        }
    }
}

pub struct FrontError(StatusCode);
impl From<ActuatorError> for FrontError {
    fn from(error: ActuatorError) -> Self {
        Self(match error {
            ActuatorError::Control(ControlError::Rejected(401)) => StatusCode::UNAUTHORIZED,
            ActuatorError::Control(ControlError::Rejected(403 | 404))
            | ActuatorError::Authority => StatusCode::FORBIDDEN,
            ActuatorError::Control(ControlError::Rejected(409)) => StatusCode::CONFLICT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        })
    }
}
impl From<ControlError> for FrontError {
    fn from(error: ControlError) -> Self {
        ActuatorError::from(error).into()
    }
}
impl IntoResponse for FrontError {
    fn into_response(self) -> Response {
        (self.0, "launch operation unavailable").into_response()
    }
}
fn token(headers: &HeaderMap, name: &'static str) -> Result<LaunchToken, FrontError> {
    if headers.contains_key(header::AUTHORIZATION) || headers.get_all(name).iter().count() != 1 {
        return Err(FrontError(StatusCode::UNAUTHORIZED));
    }
    LaunchToken::new(
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or(FrontError(StatusCode::UNAUTHORIZED))?
            .to_string(),
    )
    .map_err(|_| FrontError(StatusCode::UNAUTHORIZED))
}
fn reserved(
    id: i64,
    headers: &HeaderMap,
    request: LaunchRequest,
) -> Result<ReservedLaunch, FrontError> {
    if id <= 0 || id != request.launch_id {
        return Err(FrontError(StatusCode::BAD_REQUEST));
    }
    Ok(ReservedLaunch {
        request,
        dispatch_token: token(headers, LAUNCH_DISPATCH_HEADER)?,
    })
}
async fn launch(
    State(service): State<Arc<LaunchService>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> Result<Json<LaunchStatus>, FrontError> {
    let reserved = reserved(id, &headers, request)?;
    // Authenticate before allocating any per-launch local state.
    let manifest = service.actuator.cp.resolve(&reserved).await?;
    if manifest.engine_id != service.actuator.binding.engine_id {
        return Err(FrontError(StatusCode::FORBIDDEN));
    }
    service.schedule(reserved.clone()).await?;
    Ok(Json(service.actuator.cp.status(&reserved).await?))
}
async fn status(
    State(service): State<Arc<LaunchService>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> Result<Json<LaunchStatus>, FrontError> {
    let reserved = reserved(id, &headers, request)?;
    Ok(Json(service.actuator.cp.status(&reserved).await?))
}
async fn cancel(
    State(service): State<Arc<LaunchService>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(body): Json<StopLaunchRequest>,
) -> Result<Json<LaunchStatus>, FrontError> {
    let reserved = reserved(id, &headers, body.request)?;
    let status = service.actuator.cp.stop(&reserved, body.reason).await?;
    // Retained recovery does not depend on an existing host/frontend task.
    let _scheduled = service.schedule(reserved).await;
    Ok(Json(status))
}
async fn stage_result(
    State(service): State<Arc<LaunchService>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> Result<Json<LaunchStageResult>, FrontError> {
    let reserved = reserved(id, &headers, request)?;
    let status = service.actuator.cp.status(&reserved).await?;
    match status.publication {
        LaunchPublicationState::Pending
        | LaunchPublicationState::Uploading
        | LaunchPublicationState::NotRequired => Err(FrontError(StatusCode::NOT_FOUND)),
        LaunchPublicationState::Rejected => Err(FrontError(StatusCode::CONFLICT)),
        LaunchPublicationState::Published => {
            Ok(Json(service.actuator.cp.stage_result(&reserved).await?))
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StdinQuery {
    admitted_digest: Sha256Digest,
}
async fn stdin(
    State(service): State<Arc<LaunchService>>,
    Path(id): Path<i64>,
    Query(query): Query<StdinQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, FrontError> {
    let token = token(&headers, LAUNCH_IO_HEADER)?;
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some("application/octet-stream")
    {
        return Err(FrontError(StatusCode::UNSUPPORTED_MEDIA_TYPE));
    }
    let session = service
        .session(&LaunchRequest {
            launch_id: id,
            admitted_digest: query.admitted_digest,
        })
        .await?;
    session
        .write_stdin(&service.actuator.cp, &token, &body)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn read(
    State(service): State<Arc<LaunchService>>,
    Path((id, channel)): Path<(i64, String)>,
    Query(query): Query<LaunchIoReadQuery>,
    headers: HeaderMap,
) -> Result<Response, FrontError> {
    let token = token(&headers, LAUNCH_IO_HEADER)?;
    let channel = match channel.as_str() {
        "stdout" => LaunchIoChannel::Stdout,
        "stderr" => LaunchIoChannel::Stderr,
        _ => return Err(FrontError(StatusCode::BAD_REQUEST)),
    };
    let session = service
        .session(&LaunchRequest {
            launch_id: id,
            admitted_digest: query.admitted_digest,
        })
        .await?;
    let chunk = session
        .read(
            &service.actuator.cp,
            &token,
            channel,
            query.offset,
            query.max_bytes,
        )
        .await?;
    let mut response = chunk.bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    for (name, value) in [
        (LAUNCH_IO_NEXT_OFFSET_HEADER, chunk.next_offset.to_string()),
        (
            LAUNCH_IO_TRUNCATED_HEADER,
            chunk.truncated_bytes.to_string(),
        ),
        (LAUNCH_IO_EOF_HEADER, chunk.eof.to_string()),
    ] {
        response.headers_mut().insert(
            name,
            HeaderValue::from_str(&value)
                .map_err(|_| FrontError(StatusCode::INTERNAL_SERVER_ERROR))?,
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cache::{CacheLimits, CacheStore},
        control_plane::ControlPlane,
        engine::DockerEngine,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakeCp {
        denied: AtomicBool,
        status: LaunchStatus,
    }
    async fn cp_status(
        State(cp): State<Arc<FakeCp>>,
        headers: HeaderMap,
        Json(request): Json<LaunchRequest>,
    ) -> Response {
        if cp.denied.load(Ordering::SeqCst)
            || headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                != Some("Bearer dedicated-controller")
            || headers
                .get(LAUNCH_DISPATCH_HEADER)
                .and_then(|v| v.to_str().ok())
                != Some("target-dispatch")
            || request != cp.status.request
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        Json(cp.status.clone()).into_response()
    }
    async fn serve(
        router: Router,
    ) -> (
        reqwest::Url,
        tokio::sync::oneshot::Sender<()>,
        JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url =
            reqwest::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        (url, stop, task)
    }
    #[tokio::test]
    async fn authenticated_pending_is_not_ready_but_denial_is_never_none() {
        let request = LaunchRequest {
            launch_id: 1,
            admitted_digest: Sha256Digest::of_bytes(b"public-manifest"),
        };
        let cp = Arc::new(FakeCp {
            denied: AtomicBool::new(false),
            status: LaunchStatus {
                request: request.clone(),
                state: LaunchState::Reserved,
                runtime_state: LaunchRuntimeState::Unstarted,
                exit_outcome: None,
                publication: LaunchPublicationState::Pending,
                capture: LaunchCaptureState::Pending,
                capture_deadline: None,
                resource: None,
                stop_reason: None,
            },
        });
        let (cp_url, cp_stop, cp_task) = serve(
            Router::new()
                .route("/api/v1/workload-launches/1/status", post(cp_status))
                .with_state(cp.clone()),
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let policy = crate::test_support::policy(
            format!("sha256:{}", "a".repeat(64)),
            "owned-network".into(),
        );
        let actuator = Arc::new(Actuator {
            cp: ControlPlane::new(
                cp_url,
                LaunchToken::new("dedicated-controller".into()).unwrap(),
            )
            .unwrap(),
            engine: DockerEngine::connect(&root.path().join("nonexistent-engine.sock")).unwrap(),
            binding: LaunchEngineBinding {
                engine_id: policy.engine_id,
                daemon_id: "owned-daemon".into(),
                platform: LaunchPlatform::LinuxAmd64,
            },
            policy,
            cache: Arc::new(
                CacheStore::open(
                    root.path().join("cache"),
                    CacheLimits {
                        compressed_bytes: 1024,
                        expanded_bytes: 4096,
                        files: 10,
                    },
                )
                .unwrap(),
            ),
            compressed_ceiling: 1024,
        });
        let service = LaunchService::new(
            actuator,
            ServiceLimits {
                max_active: 1,
                max_retained: 2,
                capture_bytes_per_stream: 4096,
                minimum_free_bytes: 0,
            },
        )
        .unwrap();
        let (front_url, front_stop, front_task) = serve(service.clone().router()).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = front_url.join("v1/launches/1/stage-result").unwrap();
        // Capture results before assertions, then await both owned servers'
        // shutdown even if a contract assertion will fail.
        let pending = client
            .post(url.clone())
            .header(LAUNCH_DISPATCH_HEADER, "target-dispatch")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status();
        cp.denied.store(true, Ordering::SeqCst);
        let denied = client
            .post(url.clone())
            .header(LAUNCH_DISPATCH_HEADER, "target-dispatch")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status();
        cp.denied.store(false, Ordering::SeqCst);
        let broad_bearer = client
            .post(url)
            .header(header::AUTHORIZATION, "Bearer inappropriate-manager")
            .header(LAUNCH_DISPATCH_HEADER, "target-dispatch")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status();
        let observed = client
            .post(front_url.join("v1/launches/1/status").unwrap())
            .header(LAUNCH_DISPATCH_HEADER, "target-dispatch")
            .json(&request)
            .send()
            .await
            .unwrap();
        let observed_status = observed.status();
        let body = observed.json::<LaunchStatus>().await;
        // Exact packaged Python probe wire, through the real frontend and CP
        // HTTP client. The fixture intentionally has no admitted launch here.
        let public_probe = serde_json::json!({"launch_id":1,"admitted_digest":"a".repeat(64)});
        let mut caller_denials = Vec::new();
        for (grant, bearer) in [(false, false), (true, false), (true, true)] {
            let mut attempt = client
                .post(front_url.join("v1/launches/1/status").unwrap())
                .json(&public_probe);
            if grant {
                attempt = attempt.header(LAUNCH_DISPATCH_HEADER, "owned-wrong-grant");
            }
            if bearer {
                attempt = attempt.header(header::AUTHORIZATION, "Bearer inappropriate-manager");
            }
            caller_denials.push(attempt.send().await.unwrap().status());
        }
        let mut stdin_url = front_url.join("v1/launches/1/stdin").unwrap();
        stdin_url
            .query_pairs_mut()
            .append_pair("admitted_digest", request.admitted_digest.as_str());
        let input_above_old_limit = client
            .put(stdin_url.clone())
            .header(LAUNCH_IO_HEADER, "target-stdin")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(vec![0; 1024 * 1024 + 1])
            .send()
            .await
            .unwrap()
            .status();
        let oversized_input = client
            .put(stdin_url)
            .header(LAUNCH_IO_HEADER, "target-stdin")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(vec![
                0;
                attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES
                    + 1
            ])
            .send()
            .await
            .unwrap()
            .status();
        let oversized_dispatch = client
            .post(front_url.join("v1/launches/1/status").unwrap())
            .header(LAUNCH_DISPATCH_HEADER, "target-dispatch")
            .header(header::CONTENT_TYPE, "application/json")
            .body(vec![b' '; 1024 * 1024 + 1])
            .send()
            .await
            .unwrap()
            .status();
        // Deliberately park admission at the entries lock, then close local
        // authority before letting it resume. No sleep or scheduler race.
        let entries = service.entries.lock().await;
        let admission = service.schedule(ReservedLaunch {
            request: request.clone(),
            dispatch_token: LaunchToken::new("target-dispatch".into()).unwrap(),
        });
        tokio::pin!(admission);
        assert!(futures::poll!(&mut admission).is_pending());
        let shutdown = service.shutdown();
        tokio::pin!(shutdown);
        assert!(futures::poll!(&mut shutdown).is_pending());
        drop(entries);
        let closed_admission = admission.await;
        shutdown.await;
        front_stop.send(()).unwrap();
        front_task.await.unwrap();
        cp_stop.send(()).unwrap();
        cp_task.await.unwrap();
        assert!(matches!(
            closed_admission,
            Err(FrontError(StatusCode::SERVICE_UNAVAILABLE))
        ));
        assert!(service.entries.lock().await.is_empty());
        assert_eq!(pending, StatusCode::NOT_FOUND);
        assert_eq!(denied, StatusCode::FORBIDDEN);
        assert_eq!(broad_bearer, StatusCode::UNAUTHORIZED);
        assert_eq!(observed_status, StatusCode::OK);
        assert_eq!(body.unwrap().request, request);
        assert_eq!(
            caller_denials,
            vec![
                StatusCode::UNAUTHORIZED,
                StatusCode::FORBIDDEN,
                StatusCode::UNAUTHORIZED
            ]
        );
        // Body bounds alone grant no input authority: the valid-size stdin
        // request still has no admitted running session and writes no bytes.
        assert_eq!(input_above_old_limit, StatusCode::CONFLICT);
        assert_eq!(oversized_input, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(oversized_dispatch, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(service.entries.lock().await.is_empty());
    }
}
