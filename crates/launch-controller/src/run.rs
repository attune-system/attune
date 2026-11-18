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
        if bytes.len() > 1024 * 1024 {
            return Err(ActuatorError::Authority);
        }
        // Holding this lock serializes the durable CP one-shot authorization
        // with local consumption. Lost ACK is consumed, never retried/reminted.
        let mut io = self.io.lock().await;
        if io.stdin_taken || io.stdin.is_none() {
            return Err(ActuatorError::Authority);
        }
        let lease = io.lease.as_ref().ok_or(ActuatorError::Authority)?;
        if lease.purpose != LaunchLeasePurpose::Run {
            return Err(ActuatorError::Authority);
        }
        let auth = cp
            .authorize_io(
                lease,
                token,
                &AuthorizeLaunchIoRequest {
                    request: self.request.clone(),
                    channel: LaunchIoChannel::Stdin,
                    offset: 0,
                    bytes: bytes.len() as u64,
                },
            )
            .await?;
        io.stdin_taken = true;
        let mut writer = io.stdin.take().ok_or(ActuatorError::Authority)?;
        let deadline = (auth.expires_at - Utc::now())
            .to_std()
            .map_err(|_| ActuatorError::Authority)?;
        tokio::time::timeout(deadline, async {
            writer.write_all(bytes).await?;
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
                return self.cleanup(&manifest, &status, &lease).await;
            }
            ClaimedLaunch::Publication {
                manifest,
                status,
                lease,
            } => {
                session.replace_lease(lease.clone()).await;
                self.publish(&manifest, &status, &lease).await?;
                return Ok(self.cp.status(request).await?);
            }
            ClaimedLaunch::Capture { status, lease, .. } => {
                session.replace_lease(lease).await;
                // Restart recovery cannot manufacture previously captured bytes.
                session.fail_capture();
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
            let _closed = self.cp.stop(request, reason).await;
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
            let tick = remaining.min(Duration::from_secs(5));
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
        let mut stopped = self.cleanup(&manifest, &status, &lease).await?;
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
