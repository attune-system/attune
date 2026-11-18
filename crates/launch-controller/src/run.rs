//! One-shot Run lifecycle and finite scoped I/O. The caller supplies only a
//! ReservedLaunch; invocation and every input body come from authoritative CP.
use crate::{
    actuator::{Actuator, ActuatorError, Result},
    capture::{Capture, CapturedChunk},
    control_plane::ControlPlane,
    engine::{demultiplex, CreateOutcome, OwnedContainer},
};
use attune_common::workload_launch::*;
use chrono::Utc;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::Mutex,
    task::JoinHandle,
};

type InputWriter = Box<dyn AsyncWrite + Send + Unpin>;
struct LiveIo {
    lease: Option<LaunchLease>,
    stdin: Option<InputWriter>,
    stdin_taken: bool,
}

/// Only the owning controller retains raw output; this is not durable custody.
/// A new process cannot claim that a lost in-memory buffer reached EOF.
pub struct RunSession {
    request: LaunchRequest,
    io: Mutex<LiveIo>,
    pub stdout: Capture,
    pub stderr: Capture,
    capture_attached: std::sync::atomic::AtomicBool,
}
impl RunSession {
    pub fn new(request: LaunchRequest, capture_bytes: usize) -> std::io::Result<Self> {
        Ok(Self {
            request,
            io: Mutex::new(LiveIo {
                lease: None,
                stdin: None,
                stdin_taken: false,
            }),
            stdout: Capture::new(capture_bytes)?,
            stderr: Capture::new(capture_bytes)?,
            capture_attached: false.into(),
        })
    }
    pub fn request(&self) -> &LaunchRequest {
        &self.request
    }
    async fn lease(&self) -> Result<LaunchLease> {
        self.io
            .lock()
            .await
            .lease
            .clone()
            .ok_or(ActuatorError::Authority)
    }
    async fn renew(&self, cp: &ControlPlane) -> Result<LaunchLease> {
        let mut io = self.io.lock().await;
        let lease = io.lease.as_ref().ok_or(ActuatorError::Authority)?;
        if lease.handle.lease_deadline <= Utc::now() {
            return Err(ActuatorError::Authority);
        }
        let remaining = (lease.handle.lease_deadline - Utc::now())
            .to_std()
            .map_err(|_| ActuatorError::Authority)?;
        let lease = tokio::time::timeout(remaining, cp.renew(lease))
            .await
            .map_err(|_| ActuatorError::Authority)??;
        io.lease = Some(lease.clone());
        Ok(lease)
    }
    async fn replace_lease(&self, lease: LaunchLease) {
        self.io.lock().await.lease = Some(lease);
    }
    pub async fn write_stdin(
        &self,
        cp: &ControlPlane,
        token: &LaunchToken,
        bytes: &[u8],
    ) -> Result<()> {
        if bytes.len() > attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES {
            return Err(ActuatorError::Authority);
        }
        let payload = attune_common::workload_stdin::LaunchStdinPayload::from_bytes(bytes.to_vec())
            .map_err(|_| ActuatorError::Authority)?;
        // Holding this lock serializes the durable CP one-shot authorization
        // with local consumption. Lost ACK is consumed, never retried/reminted.
        let mut io = self.io.lock().await;
        if io.stdin_taken || io.stdin.is_none() {
            return Err(ActuatorError::Authority);
        }
        let lease = io.lease.clone().ok_or(ActuatorError::Authority)?;
        if lease.purpose != LaunchLeasePurpose::Run {
            return Err(ActuatorError::Authority);
        }
        // Consume before sending authorization. Its ACK can be lost after CP
        // commits the one-shot marker; this controller must not send it again.
        io.stdin_taken = true;
        let mut writer = io.stdin.take().ok_or(ActuatorError::Authority)?;
        let auth = cp.authorize_stdin(&lease, token, &payload).await?;
        // A slow container reader must not block lease renewal/cancellation.
        // Local consumption is already final and the exact payload is owned.
        drop(io);
        let deadline = (auth.expires_at - Utc::now())
            .to_std()
            .map_err(|_| ActuatorError::Authority)?;
        tokio::time::timeout(deadline, async {
            writer.write_all(payload.as_bytes()).await?;
            writer.shutdown().await
        })
        .await
        .map_err(|_| ActuatorError::Authority)?
        .map_err(crate::engine::EngineError::from)?;
        Ok(())
    }
    pub async fn read(
        &self,
        cp: &ControlPlane,
        token: &LaunchToken,
        channel: LaunchIoChannel,
        offset: u64,
        max_bytes: u32,
    ) -> Result<CapturedChunk> {
        if channel == LaunchIoChannel::Stdin || max_bytes == 0 || max_bytes > 65536 {
            return Err(ActuatorError::Authority);
        }
        let io = self.io.lock().await;
        let lease = io.lease.as_ref().ok_or(ActuatorError::Authority)?;
        let auth = cp
            .authorize_io(
                lease,
                token,
                &AuthorizeLaunchIoRequest {
                    request: self.request.clone(),
                    channel,
                    offset,
                    bytes: max_bytes as u64,
                },
            )
            .await?;
        let chunk = match channel {
            LaunchIoChannel::Stdout => self.stdout.read(offset, max_bytes),
            LaunchIoChannel::Stderr => self.stderr.read(offset, max_bytes),
            LaunchIoChannel::Stdin => unreachable!(),
        }
        .map_err(crate::engine::EngineError::from)?;
        if Utc::now() >= auth.expires_at {
            return Err(ActuatorError::Authority);
        }
        Ok(chunk)
    }
    pub fn fail_capture(&self) {
        self.stdout.fail();
        self.stderr.fail();
    }
    /// A finished lifecycle can retain output longer than one lease. Refresh
    /// only retained authority; a read never reopens Run or engine input.
    pub(crate) async fn maintain_retained_io(
        &self,
        cp: &ControlPlane,
        reserved: &ReservedLaunch,
    ) -> Result<()> {
        let mut io = self.io.lock().await;
        if let Some(lease) = &io.lease {
            if lease.handle.lease_deadline > Utc::now() + chrono::Duration::seconds(5) {
                return Ok(());
            }
            if matches!(
                lease.purpose,
                LaunchLeasePurpose::Capture | LaunchLeasePurpose::Publication
            ) && lease.handle.lease_deadline > Utc::now()
            {
                let remaining = (lease.handle.lease_deadline - Utc::now())
                    .to_std()
                    .map_err(|_| ActuatorError::Authority)?;
                let lease = tokio::time::timeout(remaining, cp.renew(lease))
                    .await
                    .map_err(|_| ActuatorError::Authority)??;
                io.lease = Some(lease);
                return Ok(());
            }
        }
        let lease = match cp.claim(reserved).await? {
            ClaimedLaunch::Capture { lease, .. } | ClaimedLaunch::Publication { lease, .. } => {
                lease
            }
            _ => return Err(ActuatorError::Authority),
        };
        io.lease = Some(lease);
        Ok(())
    }
}

impl Actuator {
    /// The service owns and joins this task. It must never abort a lifecycle
    /// future while a provider operation may still be in flight.
    pub async fn run(
        &self,
        request: &ReservedLaunch,
        session: &Arc<RunSession>,
        minimum_free_bytes: u64,
    ) -> Result<LaunchStatus> {
        if request.request != *session.request() {
            return Err(ActuatorError::Authority);
        }
        let claim = self.cp.claim(request).await?;
        let (spec, lease) = match claim {
            ClaimedLaunch::Active { spec, lease } => (spec, lease),
            ClaimedLaunch::Cleanup {
                manifest,
                status,
                lease,
            } => {
                session.replace_lease(lease.clone()).await;
                let stopped = self.cleanup(&manifest, &status, &lease).await?;
                return self.finish_removed(request, session, stopped).await;
            }
            ClaimedLaunch::Publication {
                manifest,
                status,
                lease,
            } => {
                session.replace_lease(lease.clone()).await;
                self.publish(&manifest, &status, &lease).await?;
                let published = self.cp.status(request).await?;
                self.cleanup_private(&manifest, &published).await?;
                return self.finish_removed(request, session, published).await;
            }
            ClaimedLaunch::Capture { status, lease, .. } => {
                session.replace_lease(lease).await;
                // Restart recovery cannot manufacture previously captured bytes.
                if !session
                    .capture_attached
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    session.fail_capture();
                }
                return Ok(status);
            }
            ClaimedLaunch::Stopped { status } => return Ok(status),
        };
        if spec.manifest.engine_id != self.binding.engine_id {
            return Err(ActuatorError::Authority);
        }
        session.replace_lease(lease).await;
        let mut reader = None;
        let mut resource = None;
        let result = self
            .execute_run(
                &spec,
                session,
                minimum_free_bytes,
                &mut resource,
                &mut reader,
            )
            .await;
        // Drop decrypted argv/env before retained cleanup/publication claims.
        drop(spec);
        if result.is_err() {
            // A failed close does not acknowledge cleanup. Nonetheless stop an
            // owned running container after its local authority ends; CP recovery
            // retains its create and Wait+Remove obligations.
            let reason = if matches!(result, Err(ActuatorError::Timeout)) {
                LaunchStopReason::Timeout
            } else {
                LaunchStopReason::LaunchFailure
            };
            let _closed =
                tokio::time::timeout(Duration::from_secs(2), self.cp.stop(request, reason)).await;
            if let Some(container) = &resource {
                let _settled = self.engine.stop_and_wait(container, 10).await;
            }
        }
        // Normal Exited and explicit stop both rotate into a public-only cleanup
        // lease before provider removal or ACK. No Run lease can publish/ACK.
        let retained = self.settle_after_run(request, session).await;
        if let Some(mut reader) = reader {
            if !matches!(
                tokio::time::timeout(Duration::from_secs(10), &mut reader).await,
                Ok(Ok(Ok(())))
            ) {
                reader.abort();
                let _joined = reader.await;
                session.fail_capture();
            }
        } else {
            session.fail_capture();
        }
        // Closing stdin is independent of output capture/retention.
        session.io.lock().await.stdin = None;
        retained
    }
    async fn execute_run(
        &self,
        spec: &LaunchSpec,
        session: &RunSession,
        minimum_free_bytes: u64,
        resource: &mut Option<OwnedContainer>,
        reader: &mut Option<JoinHandle<crate::engine::Result<()>>>,
    ) -> Result<()> {
        let manifest = &spec.manifest;
        self.engine
            .verify_engine_binding(&self.binding, &self.policy)
            .await?;
        let cache = self.cache.clone();
        let saved = manifest.clone();
        tokio::task::spawn_blocking(move || cache.ensure_capacity(&saved, minimum_free_bytes))
            .await
            .map_err(|_| ActuatorError::FilesystemTask)??;
        for pin in &manifest.inputs {
            let lease = session.renew(&self.cp).await?;
            let body = tempfile::NamedTempFile::new().map_err(crate::engine::EngineError::from)?;
            self.cp
                .download_input(&lease, pin, body.path(), self.compressed_ceiling)
                .await?;
            let cache = self.cache.clone();
            let pin = pin.clone();
            tokio::task::spawn_blocking(move || {
                let mut body = body;
                cache.install_input(&pin, body.as_file_mut())
            })
            .await
            .map_err(|_| ActuatorError::FilesystemTask)??;
        }
        let cache = self.cache.clone();
        let saved = manifest.clone();
        tokio::task::spawn_blocking(move || cache.prepare_private(&saved))
            .await
            .map_err(|_| ActuatorError::FilesystemTask)??;
        let image_env = self
            .engine
            .image_environment_for_binding(&self.binding, &self.policy, &manifest.image)
            .await?;
        let prepared = self.policy.prepare(spec, &image_env)?;
        let lease = session.renew(&self.cp).await?;
        let create = self.cp.create_intent(&lease).await?;
        if create.disposition != CreateDisposition::CreateOnce {
            return Err(ActuatorError::Unsettled);
        }
        let container = match self.engine.create_once(&prepared).await? {
            CreateOutcome::Created(container) => container,
            CreateOutcome::ReconcileRequired | CreateOutcome::DefinitivelyRejected { .. } => {
                // Conservatively retain settlement if process interruption loses
                // a rejection. Never infer absence from a subsequent lookup.
                self.cp
                    .observe(
                        &lease,
                        LaunchObservation::CreateUnknown {
                            create_nonce: create.create_nonce,
                        },
                    )
                    .await?;
                return Err(ActuatorError::Unsettled);
            }
        };
        *resource = Some(container);
        let container = resource.as_ref().expect("owned create");
        self.cp
            .observe(
                &lease,
                LaunchObservation::Created {
                    create_nonce: create.create_nonce,
                    resource: container.evidence().clone(),
                },
            )
            .await?;
        let attached = self.engine.attach(container).await?;
        let (source, sink) = tokio::io::split(attached);
        session.io.lock().await.stdin = Some(Box::new(sink));
        let stdout = session.stdout.clone();
        let stderr = session.stderr.clone();
        session
            .capture_attached
            .store(true, std::sync::atomic::Ordering::Release);
        *reader = Some(tokio::spawn(demultiplex(source, stdout, stderr)));
        let lease = session.renew(&self.cp).await?;
        let start = self.cp.start_intent(&lease).await?;
        if start.disposition != StartDisposition::StartOnce {
            return Err(ActuatorError::Unsettled);
        }
        if self.engine.start_once(container).await.is_err() {
            self.cp
                .observe(
                    &lease,
                    LaunchObservation::StartUnknown {
                        start_nonce: start.start_nonce,
                        resource: container.evidence().clone(),
                    },
                )
                .await?;
            return Err(ActuatorError::Unsettled);
        }
        self.cp
            .observe(
                &lease,
                LaunchObservation::Started {
                    start_nonce: start.start_nonce,
                    resource: container.evidence().clone(),
                },
            )
            .await?;
        let timeout = manifest.timeout_ms.map(Duration::from_millis);
        let started = tokio::time::Instant::now();
        let wait = self.engine.wait(container);
        tokio::pin!(wait);
        loop {
            let lease = session.lease().await?;
            let remaining = (lease.handle.lease_deadline - Utc::now())
                .to_std()
                .map_err(|_| ActuatorError::Authority)?;
            if remaining.is_zero() {
                return Err(ActuatorError::Authority);
            }
            let mut tick = remaining.min(Duration::from_secs(5));
            if let Some(limit) = timeout {
                let left = limit.saturating_sub(started.elapsed());
                if left.is_zero() {
                    return Err(ActuatorError::Timeout);
                }
                tick = tick.min(left);
            }
            let exit = tokio::select! {
                result = &mut wait => Some(result?),
                _ = tokio::time::sleep(tick) => None,
            };
            if let Some(exit) = exit {
                let lease = session.renew(&self.cp).await?;
                self.cp
                    .observe(
                        &lease,
                        LaunchObservation::Exited {
                            start_nonce: start.start_nonce,
                            resource: container.evidence().clone(),
                            exit_code: exit.exit_code,
                            oom_killed: exit.oom_killed,
                        },
                    )
                    .await?;
                return Ok(());
            }
            if timeout.is_some_and(|limit| started.elapsed() >= limit) {
                return Err(ActuatorError::Timeout);
            }
            // Fresh renewal rechecks source/owner/attempt and detects CP stop.
            // A denial enters stop/cleanup; no continuing on cached authority.
            session.renew(&self.cp).await?;
        }
    }
    async fn settle_after_run(
        &self,
        request: &ReservedLaunch,
        session: &RunSession,
    ) -> Result<LaunchStatus> {
        let (manifest, status, lease) = match self.cp.claim(request).await? {
            ClaimedLaunch::Cleanup {
                manifest,
                status,
                lease,
            } => (manifest, status, lease),
            _ => return Err(ActuatorError::Authority),
        };
        session.replace_lease(lease.clone()).await;
        let stopped = self.cleanup(&manifest, &status, &lease).await?;
        self.finish_removed(request, session, stopped).await
    }
    async fn finish_removed(
        &self,
        request: &ReservedLaunch,
        session: &RunSession,
        mut stopped: LaunchStatus,
    ) -> Result<LaunchStatus> {
        if stopped.permits_stage_publication()
            && matches!(
                stopped.publication,
                LaunchPublicationState::Pending | LaunchPublicationState::Uploading
            )
        {
            if let ClaimedLaunch::Publication {
                manifest,
                status,
                lease,
            } = self.cp.claim(request).await?
            {
                session.replace_lease(lease.clone()).await;
                self.publish(&manifest, &status, &lease).await?;
                stopped = self.cp.status(request).await?;
                self.cleanup_private(&manifest, &stopped).await?;
            } else {
                return Err(ActuatorError::Authority);
            }
        }
        if stopped.capture == LaunchCaptureState::Pending {
            if let ClaimedLaunch::Capture { lease, status, .. } = self.cp.claim(request).await? {
                session.replace_lease(lease).await;
                stopped = status;
            } else {
                return Err(ActuatorError::Authority);
            }
        }
        Ok(stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        workload_launch_io::LaunchHttpPolicy,
        workload_stdin::{AuthorizeLaunchStdinRequest, LaunchStdinPayload},
    };
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn exact_private_stdin_and_local_one_shot_survive_lost_or_invalid_ack() {
        // Transport/writer proof only. Actual CP frozen context/origin and
        // transaction fences require owning API integration, not this mock.
        for outcome in [
            "authorized",
            "backpressure",
            "lost-ack",
            "wrong-count",
            "denied",
        ] {
            let bytes =
                b"{\"selected\":\"owned-private-input\",\"escaped\":\"\\u0000\"}\n".to_vec();
            let request = LaunchRequest {
                launch_id: 1,
                admitted_digest: Sha256Digest::of_bytes(b"manifest"),
            };
            let lease = LaunchLease {
                purpose: LaunchLeasePurpose::Run,
                handle: LaunchHandle {
                    launch_id: 1,
                    admitted_digest: request.admitted_digest.clone(),
                    lease_id: uuid::Uuid::new_v4(),
                    lease_generation: 1,
                    lease_deadline: Utc::now() + chrono::Duration::seconds(60),
                    engine_id: 5,
                    engine_name_nonce: uuid::Uuid::new_v4(),
                },
                token: LaunchToken::new("owned-lease".into()).unwrap(),
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let expected_handle = lease.handle.clone();
            let expected_bytes = bytes.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();
                let mut buffer = [0; 4096];
                let end = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    received.extend_from_slice(&buffer[..count]);
                    assert!(received.len() <= 16384);
                    if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8(received[..end].to_vec())
                    .unwrap()
                    .to_ascii_lowercase();
                assert!(headers
                    .starts_with("post /api/v1/workload-launches/1/authorize-stdin http/1.1\r\n"));
                for expected in [
                    "authorization: bearer owned-controller",
                    "x-attune-launch-lease: owned-lease",
                    "x-attune-launch-io: owned-stdin",
                ] {
                    assert!(headers.contains(expected));
                }
                assert!(!headers.contains("x-attune-launch-dispatch:"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!(length < 8192);
                while received.len() - end < length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    received.extend_from_slice(&buffer[..count]);
                    assert!(received.len() <= 16384);
                }
                let input: AuthorizeLaunchStdinRequest =
                    serde_json::from_slice(&received[end..end + length]).unwrap();
                assert_eq!(input.handle, expected_handle);
                assert_eq!(input.payload.as_bytes(), expected_bytes);
                if outcome != "lost-ack" {
                    let response = if outcome == "denied" {
                        serde_json::json!({})
                    } else {
                        serde_json::to_value(LaunchIoAuthorization {
                            handle: expected_handle,
                            channel: LaunchIoChannel::Stdin,
                            max_bytes: expected_bytes.len() as u64
                                + u64::from(outcome == "wrong-count"),
                            expires_at: Utc::now() + chrono::Duration::seconds(30),
                        })
                        .unwrap()
                    };
                    let body = serde_json::to_vec(&response).unwrap();
                    let status = if outcome == "denied" {
                        "403 Forbidden"
                    } else {
                        "200 OK"
                    };
                    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
                    socket.write_all(&body).await.unwrap();
                }
                socket.shutdown().await.unwrap();
            });
            let token = LaunchToken::new("owned-stdin".into()).unwrap();
            // Literal-loopback allowance for public/status tests must not
            // silently permit private selected input over plaintext.
            let strict = ControlPlane::new(
                format!("http://{address}/").parse().unwrap(),
                LaunchToken::new("owned-controller".into()).unwrap(),
            )
            .unwrap();
            let payload = LaunchStdinPayload::from_bytes(bytes.clone()).unwrap();
            assert!(matches!(
                strict.authorize_stdin(&lease, &token, &payload).await,
                Err(crate::control_plane::ControlError::Configuration)
            ));
            let cp = ControlPlane::with_policy(
                format!("http://{address}/").parse().unwrap(),
                LaunchToken::new("owned-controller".into()).unwrap(),
                LaunchHttpPolicy::DevelopmentPlainHttp,
            )
            .unwrap();
            let session = Arc::new(RunSession::new(request, 4096).unwrap());
            session.replace_lease(lease).await;
            let (writer, mut reader) =
                tokio::io::duplex(if outcome == "backpressure" { 1 } else { 4096 });
            session.io.lock().await.stdin = Some(Box::new(writer));
            let writing = tokio::spawn({
                let session = session.clone();
                let cp = cp.clone();
                let token = token.clone();
                let bytes = bytes.clone();
                async move { session.write_stdin(&cp, &token, &bytes).await }
            });
            server.await.unwrap();
            let lease_available = if outcome == "backpressure" {
                tokio::time::timeout(Duration::from_secs(2), session.lease())
                    .await
                    .is_ok()
            } else {
                true
            };
            let mut written = Vec::new();
            reader.read_to_end(&mut written).await.unwrap();
            let result = writing.await.unwrap();
            let authorized = matches!(outcome, "authorized" | "backpressure");
            assert!(
                lease_available,
                "stdin backpressure blocked lease/cancel owner"
            );
            assert_eq!(result.is_ok(), authorized);
            assert_eq!(
                written,
                if authorized {
                    bytes.clone()
                } else {
                    Vec::new()
                }
            );
            assert!(matches!(
                session.write_stdin(&cp, &token, &bytes).await,
                Err(ActuatorError::Authority)
            ));
        }
    }

    #[tokio::test]
    async fn stalled_output_authorization_releases_session_lock_by_lease_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let cp = ControlPlane::new(
            origin.parse().unwrap(),
            LaunchToken::new("owned-controller".into()).unwrap(),
        )
        .unwrap();
        let request = LaunchRequest {
            launch_id: 1,
            admitted_digest: Sha256Digest::of_bytes(b"manifest"),
        };
        let lease = LaunchLease {
            purpose: LaunchLeasePurpose::Run,
            handle: LaunchHandle {
                launch_id: 1,
                admitted_digest: request.admitted_digest.clone(),
                lease_id: uuid::Uuid::new_v4(),
                lease_generation: 1,
                lease_deadline: Utc::now() + chrono::Duration::seconds(1),
                engine_id: 5,
                engine_name_nonce: uuid::Uuid::new_v4(),
            },
            token: LaunchToken::new("owned-lease".into()).unwrap(),
        };
        let session = Arc::new(RunSession::new(request, 4096).unwrap());
        session.replace_lease(lease).await;
        let (entered, received) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(socket.read(&mut bytes).await.unwrap() > 0);
            entered.send(()).unwrap();
            let _ = released.await;
            socket.shutdown().await.unwrap();
        });
        let reading = tokio::spawn({
            let cp = cp.clone();
            let session = session.clone();
            async move {
                session
                    .read(
                        &cp,
                        &LaunchToken::new("owned-output".into()).unwrap(),
                        LaunchIoChannel::Stdout,
                        0,
                        64,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), received)
            .await
            .unwrap()
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), reading)
            .await
            .unwrap()
            .unwrap();
        let available = tokio::time::timeout(Duration::from_secs(1), session.lease())
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        assert!(available.handle.lease_deadline <= Utc::now());
        assert!(matches!(
            session.renew(&cp).await,
            Err(ActuatorError::Authority)
        ));
        release.send(()).unwrap();
        server.await.unwrap();
    }
}
