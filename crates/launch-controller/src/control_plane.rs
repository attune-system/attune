//! Typed client for the authoritative launch ledger. The controller transports
//! credentials in headers only and never logs response bodies/invocation data.
use attune_common::workload_launch::*;
use reqwest::{Client, Response, Url};
use serde::{de::DeserializeOwned, Serialize};
use std::{path::Path, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

#[path = "artifact_client.rs"]
mod artifact_client;

const JSON_LIMIT: usize = 4 * 1024 * 1024;
#[derive(Debug, Error)]
pub enum ControlError {
    #[error("invalid fixed control-plane configuration")]
    Configuration,
    #[error("control-plane request failed; launch authority is not established")]
    Transport,
    #[error("control-plane request rejected (HTTP {0})")]
    Rejected(u16),
    #[error("invalid or oversized control-plane response")]
    Response,
    #[error("input staging I/O failed")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, ControlError>;

#[derive(Clone)]
pub struct ControlPlane {
    client: Client,
    base: Url,
    credential: Arc<tokio::sync::RwLock<LaunchToken>>,
    private_plain_http: bool,
}
impl ControlPlane {
    pub fn new(base: Url, credential: LaunchToken) -> Result<Self> {
        Self::with_policy(
            base,
            credential,
            attune_common::workload_launch_io::LaunchHttpPolicy::TlsOrLoopback,
        )
    }
    pub fn with_policy(
        base: Url,
        credential: LaunchToken,
        policy: attune_common::workload_launch_io::LaunchHttpPolicy,
    ) -> Result<Self> {
        if !policy.allows_origin(&base)
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            return Err(ControlError::Configuration);
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| ControlError::Configuration)?;
        Ok(Self {
            client,
            base,
            credential: Arc::new(tokio::sync::RwLock::new(credential)),
            private_plain_http: matches!(
                policy,
                attune_common::workload_launch_io::LaunchHttpPolicy::DevelopmentPlainHttp
            ),
        })
    }
    /// Replace only with a CP-issued, engine/pool-scoped session credential.
    /// Clones observe the replacement; this client contains no signing key and
    /// never invents a refresh or unlimited lifetime when renewal is unavailable.
    pub async fn replace_credential(&self, credential: LaunchToken) {
        *self.credential.write().await = credential;
    }
    async fn credential(&self) -> LaunchToken {
        self.credential.read().await.clone()
    }
    /// Renewal is CP-issued and target-bound. A lost response does not extend
    /// the old credential locally; callers must stop using it at its deadline.
    pub async fn renew_session(
        &self,
        current_expires_at: chrono::DateTime<chrono::Utc>,
        persist: impl FnOnce(&attune_common::workload_session::WorkloadSession) -> std::io::Result<()>,
    ) -> Result<attune_common::workload_session::WorkloadSession> {
        use attune_common::workload_session::{RenewWorkloadSession, WorkloadSession};
        let now = chrono::Utc::now();
        let remaining = (current_expires_at - now)
            .to_std()
            .map_err(|_| ControlError::Response)?;
        if remaining.is_zero() {
            return Err(ControlError::Response);
        }
        let previous = self.credential().await;
        let mut url = self.base.clone();
        url.set_path("/api/v1/workload-launches/session");
        let response = self
            .client
            .post(url)
            .bearer_auth(previous.as_str())
            .json(&RenewWorkloadSession {})
            .timeout(remaining.min(Duration::from_secs(30)))
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        if !response.status().is_success() {
            return Err(ControlError::Rejected(response.status().as_u16()));
        }
        if response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            != Some("no-store")
        {
            return Err(ControlError::Response);
        }
        let session: WorkloadSession = decode(response).await?;
        let now = chrono::Utc::now();
        if now >= current_expires_at
            || session.expires_at <= now
            || session.expires_at > now + chrono::Duration::seconds(300)
        {
            return Err(ControlError::Response);
        }
        let mut slot = self.credential.write().await;
        if slot.as_str() != previous.as_str() {
            return Err(ControlError::Response);
        }
        // The exclusive provider must publish+fsync before RAM replacement.
        // Any persistence error is fatal to admission, never RAM-only renewal.
        persist(&session)?;
        if chrono::Utc::now() >= current_expires_at || chrono::Utc::now() >= session.expires_at {
            return Err(ControlError::Response);
        }
        *slot = session.token.clone();
        Ok(session)
    }
    fn url(&self, id: i64, suffix: &[&str]) -> Result<Url> {
        if id <= 0 {
            return Err(ControlError::Configuration);
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| ControlError::Configuration)?
            .extend(["api", "v1", "workload-launches", &id.to_string()])
            .extend(suffix);
        Ok(url)
    }
    pub async fn engine_binding(&self) -> Result<LaunchEngineBinding> {
        let mut url = self.base.clone();
        url.set_path("/api/v1/workload-launches/engine");
        let binding: LaunchEngineBinding = decode(
            self.client
                .get(url)
                .bearer_auth(self.credential().await.as_str())
                .send()
                .await
                .map_err(|_| ControlError::Transport)?,
        )
        .await?;
        binding
            .validate_shape()
            .map_err(|_| ControlError::Response)?;
        Ok(binding)
    }
    async fn post<T: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        id: i64,
        suffix: &[&str],
        header: &'static str,
        token: &LaunchToken,
        body: &T,
    ) -> Result<O> {
        let response = self
            .client
            .post(self.url(id, suffix)?)
            .bearer_auth(self.credential().await.as_str())
            .header(header, secret(token)?)
            .json(body)
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        decode(response).await
    }
    pub async fn resolve(&self, launch: &ReservedLaunch) -> Result<LaunchManifest> {
        let manifest: LaunchManifest = self
            .post(
                launch.request.launch_id,
                &["resolve"],
                LAUNCH_DISPATCH_HEADER,
                &launch.dispatch_token,
                &launch.request,
            )
            .await?;
        if manifest.launch_id != launch.request.launch_id
            || manifest
                .admitted_digest()
                .map_err(|_| ControlError::Response)?
                != launch.request.admitted_digest
        {
            return Err(ControlError::Response);
        }
        Ok(manifest)
    }
    pub async fn reconcile_page(
        &self,
        query: &ReconcileLaunchesRequest,
    ) -> Result<ReconcileLaunchesPage> {
        if !(1..=100).contains(&query.limit) || query.after.is_some_and(|id| id <= 0) {
            return Err(ControlError::Configuration);
        }
        let mut url = self.base.clone();
        url.set_path("/api/v1/workload-launches/reconcile");
        let page: ReconcileLaunchesPage = decode(
            self.client
                .post(url)
                .bearer_auth(self.credential().await.as_str())
                .json(query)
                .send()
                .await
                .map_err(|_| ControlError::Transport)?,
        )
        .await?;
        if page.items.len() > query.limit as usize
            || page
                .items
                .windows(2)
                .any(|pair| pair[0].request.launch_id >= pair[1].request.launch_id)
            || page
                .items
                .iter()
                .any(|item| item.request.launch_id <= query.after.unwrap_or(0))
            || page.next_cursor.is_some_and(|cursor| {
                page.items
                    .last()
                    .is_none_or(|item| item.request.launch_id != cursor)
            })
        {
            return Err(ControlError::Response);
        }
        Ok(page)
    }
    /// Compare the exact private bytes with CP's authenticated frozen input.
    /// The caller retains this same immutable payload through the one write.
    /// No generic count authorization, public hash or insecure implicit loopback
    /// transport may stand in for this request. A lost ACK is not retried here.
    pub async fn authorize_stdin(
        &self,
        lease: &LaunchLease,
        token: &LaunchToken,
        payload: &attune_common::workload_stdin::LaunchStdinPayload,
    ) -> Result<LaunchIoAuthorization> {
        if (self.base.scheme() != "https" && !self.private_plain_http)
            || lease.purpose != LaunchLeasePurpose::Run
            || lease.handle.lease_deadline <= chrono::Utc::now()
        {
            return Err(ControlError::Configuration);
        }
        let remaining = (lease.handle.lease_deadline - chrono::Utc::now())
            .to_std()
            .map_err(|_| ControlError::Response)?;
        let response = self
            .client
            .post(self.url(lease.handle.launch_id, &["authorize-stdin"])?)
            .bearer_auth(self.credential().await.as_str())
            .header(LAUNCH_LEASE_HEADER, secret(&lease.token)?)
            .header(LAUNCH_IO_HEADER, secret(token)?)
            .json(
                &attune_common::workload_stdin::AuthorizeLaunchStdinRequest {
                    handle: lease.handle.clone(),
                    payload: payload.clone(),
                },
            )
            .timeout(remaining.min(Duration::from_secs(30)))
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        let authorization: LaunchIoAuthorization = decode(response).await?;
        if authorization.handle != lease.handle
            || authorization.channel != LaunchIoChannel::Stdin
            || authorization.max_bytes != payload.as_bytes().len() as u64
            || authorization.expires_at <= chrono::Utc::now()
            || authorization.expires_at > lease.handle.lease_deadline
        {
            return Err(ControlError::Response);
        }
        Ok(authorization)
    }

    pub async fn authorize_io(
        &self,
        lease: &LaunchLease,
        token: &LaunchToken,
        operation: &AuthorizeLaunchIoRequest,
    ) -> Result<LaunchIoAuthorization> {
        // Stdin requires CP comparison with the privately frozen exact bytes.
        // A count-only grant must never activate pack-controlled input, even
        // against an older CP that still accepts this generic channel request.
        if operation.channel == LaunchIoChannel::Stdin
            || operation.request.launch_id != lease.handle.launch_id
            || operation.request.admitted_digest != lease.handle.admitted_digest
        {
            return Err(ControlError::Response);
        }
        let authorization: LaunchIoAuthorization = decode(
            self.client
                .post(self.url(lease.handle.launch_id, &["authorize-io"])?)
                .bearer_auth(self.credential().await.as_str())
                .header(LAUNCH_LEASE_HEADER, secret(&lease.token)?)
                .header(LAUNCH_IO_HEADER, secret(token)?)
                .json(operation)
                .send()
                .await
                .map_err(|_| ControlError::Transport)?,
        )
        .await?;
        if authorization.handle != lease.handle
            || authorization.channel != operation.channel
            || authorization.max_bytes < operation.bytes
            || authorization.expires_at <= chrono::Utc::now()
            || authorization.expires_at > lease.handle.lease_deadline
        {
            return Err(ControlError::Response);
        }
        Ok(authorization)
    }
    pub async fn claim(&self, launch: &ReservedLaunch) -> Result<ClaimedLaunch> {
        let claim: ClaimedLaunch = self
            .post(
                launch.request.launch_id,
                &["claim"],
                LAUNCH_DISPATCH_HEADER,
                &launch.dispatch_token,
                &launch.request,
            )
            .await?;
        match &claim {
            ClaimedLaunch::Active { spec, lease } => {
                spec.validate_shape().map_err(|_| ControlError::Response)?;
                validate_lease(lease, &launch.request)?;
                if spec
                    .manifest
                    .admitted_digest()
                    .map_err(|_| ControlError::Response)?
                    != launch.request.admitted_digest
                    || spec.manifest.launch_id != launch.request.launch_id
                    || spec.manifest.engine_id != lease.handle.engine_id
                    || spec.manifest.engine_name_nonce != lease.handle.engine_name_nonce
                    || lease.purpose != LaunchLeasePurpose::Run
                {
                    return Err(ControlError::Response);
                }
            }
            ClaimedLaunch::Cleanup {
                manifest,
                status,
                lease,
            } => {
                validate_lease(lease, &launch.request)?;
                validate_status(status, &launch.request)?;
                if manifest
                    .admitted_digest()
                    .map_err(|_| ControlError::Response)?
                    != launch.request.admitted_digest
                    || manifest.launch_id != launch.request.launch_id
                    || manifest.engine_id != lease.handle.engine_id
                    || manifest.engine_name_nonce != lease.handle.engine_name_nonce
                    || lease.purpose != LaunchLeasePurpose::Cleanup
                    || !matches!(
                        status.state,
                        LaunchState::CleanupRequired | LaunchState::StopRequired
                    )
                {
                    return Err(ControlError::Response);
                }
            }
            ClaimedLaunch::Stopped { status } => {
                validate_status(status, &launch.request)?;
                if status.state != LaunchState::Stopped {
                    return Err(ControlError::Response);
                }
            }
            ClaimedLaunch::Publication {
                manifest,
                status,
                lease,
            } => {
                validate_lease(lease, &launch.request)?;
                validate_status(status, &launch.request)?;
                if manifest.purpose != LaunchPurpose::Setup
                    || manifest.launch_id != launch.request.launch_id
                    || manifest
                        .admitted_digest()
                        .map_err(|_| ControlError::Response)?
                        != launch.request.admitted_digest
                    || manifest.engine_id != lease.handle.engine_id
                    || manifest.engine_name_nonce != lease.handle.engine_name_nonce
                    || lease.purpose != LaunchLeasePurpose::Publication
                    || !status.permits_stage_publication()
                {
                    return Err(ControlError::Response);
                }
            }
            ClaimedLaunch::Capture {
                manifest,
                status,
                lease,
            } => {
                validate_lease(lease, &launch.request)?;
                validate_status(status, &launch.request)?;
                if manifest.launch_id != launch.request.launch_id
                    || manifest
                        .admitted_digest()
                        .map_err(|_| ControlError::Response)?
                        != launch.request.admitted_digest
                    || manifest.engine_id != lease.handle.engine_id
                    || manifest.engine_name_nonce != lease.handle.engine_name_nonce
                    || lease.purpose != LaunchLeasePurpose::Capture
                    || status.state != LaunchState::Stopped
                    || status.capture != LaunchCaptureState::Pending
                    || status.capture_deadline.is_none()
                {
                    return Err(ControlError::Response);
                }
            }
        }
        Ok(claim)
    }
    pub async fn status(&self, launch: &ReservedLaunch) -> Result<LaunchStatus> {
        let status = self
            .post(
                launch.request.launch_id,
                &["status"],
                LAUNCH_DISPATCH_HEADER,
                &launch.dispatch_token,
                &launch.request,
            )
            .await?;
        validate_status(&status, &launch.request)?;
        Ok(status)
    }
    pub async fn create_intent(&self, lease: &LaunchLease) -> Result<LaunchCreateIntent> {
        let intent: LaunchCreateIntent = self
            .with_lease(
                lease,
                "create-intent",
                &LaunchLeaseRequest {
                    handle: lease.handle.clone(),
                },
            )
            .await?;
        if intent.handle != lease.handle {
            return Err(ControlError::Response);
        }
        Ok(intent)
    }
    pub async fn start_intent(&self, lease: &LaunchLease) -> Result<LaunchStartIntent> {
        let intent: LaunchStartIntent = self
            .with_lease(
                lease,
                "start",
                &LaunchLeaseRequest {
                    handle: lease.handle.clone(),
                },
            )
            .await?;
        if intent.handle != lease.handle {
            return Err(ControlError::Response);
        }
        Ok(intent)
    }
    pub async fn renew(&self, lease: &LaunchLease) -> Result<LaunchLease> {
        let renewed: LaunchLease = self
            .with_lease(
                lease,
                "renew",
                &LaunchLeaseRequest {
                    handle: lease.handle.clone(),
                },
            )
            .await?;
        validate_lease(&renewed, &request(&lease.handle))?;
        if renewed.handle.engine_id != lease.handle.engine_id
            || renewed.purpose != lease.purpose
            || renewed.handle.engine_name_nonce != lease.handle.engine_name_nonce
            || renewed.handle.lease_id != lease.handle.lease_id
            || renewed.handle.lease_generation != lease.handle.lease_generation
        {
            return Err(ControlError::Response);
        }
        Ok(renewed)
    }
    pub async fn observe(
        &self,
        lease: &LaunchLease,
        observation: LaunchObservation,
    ) -> Result<LaunchStatus> {
        let status = self
            .with_lease(
                lease,
                "observe",
                &ObserveLaunchRequest {
                    handle: lease.handle.clone(),
                    observation,
                },
            )
            .await?;
        validate_status(&status, &request(&lease.handle))?;
        Ok(status)
    }
    /// Closing CP authority must precede engine stop/cancel acknowledgement.
    pub async fn stop(
        &self,
        launch: &ReservedLaunch,
        reason: LaunchStopReason,
    ) -> Result<LaunchStatus> {
        let status = self
            .post(
                launch.request.launch_id,
                &["stop"],
                LAUNCH_DISPATCH_HEADER,
                &launch.dispatch_token,
                &StopLaunchRequest {
                    request: launch.request.clone(),
                    reason,
                },
            )
            .await?;
        validate_status(&status, &launch.request)?;
        if !matches!(
            status.state,
            LaunchState::StopRequired | LaunchState::Stopped
        ) {
            return Err(ControlError::Response);
        }
        Ok(status)
    }
    pub async fn stop_ack(
        &self,
        lease: &LaunchLease,
        create: ProviderCreateSettlement,
        removed_resource: Option<EngineResourceEvidence>,
    ) -> Result<LaunchStatus> {
        let status = self
            .with_lease(
                lease,
                "stop-ack",
                &StopLaunchAcknowledgement {
                    handle: lease.handle.clone(),
                    create,
                    removed_resource,
                },
            )
            .await?;
        validate_status(&status, &request(&lease.handle))?;
        if status.state != LaunchState::Stopped {
            return Err(ControlError::Response);
        }
        Ok(status)
    }
    pub async fn reserve_stage_output(
        &self,
        lease: &LaunchLease,
        output: &ReserveLaunchOutput,
    ) -> Result<LaunchOutputGrant> {
        if lease.purpose != LaunchLeasePurpose::Publication
            || output.handle != lease.handle
            || output.kind != LaunchOutputKind::StageArchive
            || output.size_bytes == 0
        {
            return Err(ControlError::Response);
        }
        let grant: LaunchOutputGrant = self.with_lease(lease, "outputs", output).await?;
        if grant.output_id <= 0
            || grant.expires_at <= chrono::Utc::now()
            || grant.expires_at > lease.handle.lease_deadline
        {
            return Err(ControlError::Response);
        }
        Ok(grant)
    }
    /// File ownership is consumed; no pack process can modify this controller
    /// spool body. CP validates the exact payload and repeats authority checks
    /// after native provider I/O before returning its durable receipt.
    pub async fn upload_stage_output(
        &self,
        lease: &LaunchLease,
        output: &ReserveLaunchOutput,
        grant: &LaunchOutputGrant,
        body: std::fs::File,
        compressed_ceiling: u64,
    ) -> Result<LaunchStageResult> {
        if lease.purpose != LaunchLeasePurpose::Publication
            || output.handle != lease.handle
            || output.kind != LaunchOutputKind::StageArchive
            || grant.output_id <= 0
            || output.size_bytes == 0
            || output.size_bytes > compressed_ceiling
            || body.metadata()?.len() != output.size_bytes
        {
            return Err(ControlError::Response);
        }
        let remaining = (grant.expires_at - chrono::Utc::now())
            .to_std()
            .map_err(|_| ControlError::Response)?;
        if remaining.is_zero() || grant.expires_at > lease.handle.lease_deadline {
            return Err(ControlError::Response);
        }
        let mut file = tokio::fs::File::from_std(body);
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let deadline = grant.expires_at;
        let stream = futures::stream::try_unfold(
            (file, output.size_bytes),
            move |(mut file, remaining)| async move {
                if remaining == 0 {
                    return Ok(None);
                }
                let mut bytes = vec![0u8; remaining.min(65536) as usize];
                let count = file.read(&mut bytes).await?;
                if chrono::Utc::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "publication grant expired",
                    ));
                }
                if count == 0 {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "publication body shortened",
                    ))
                } else {
                    bytes.truncate(count);
                    Ok(Some((bytes, (file, remaining - count as u64))))
                }
            },
        );
        let mut url = self.url(
            lease.handle.launch_id,
            &["outputs", &grant.output_id.to_string(), "body"],
        )?;
        url.query_pairs_mut()
            .append_pair("admitted_digest", lease.handle.admitted_digest.as_str());
        let response = self
            .client
            .post(url)
            .bearer_auth(self.credential().await.as_str())
            .header(LAUNCH_LEASE_HEADER, secret(&lease.token)?)
            .header(LAUNCH_OUTPUT_HEADER, secret(&grant.token)?)
            .header(reqwest::header::CONTENT_TYPE, LAUNCH_ARCHIVE_CONTENT_TYPE)
            .header(reqwest::header::CONTENT_LENGTH, output.size_bytes)
            .timeout(remaining.min(Duration::from_secs(30)))
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        let receipt: LaunchStageResult = decode(response).await?;
        if receipt.output_id != grant.output_id
            || receipt.object_id <= 0
            || receipt.validation_receipt <= 0
            || receipt.object_version.is_empty()
            || receipt.size_bytes != output.size_bytes
            || receipt.sha256 != output.sha256
        {
            return Err(ControlError::Response);
        }
        Ok(receipt)
    }
    /// Historical receipt only. A CP denial/timeout is never absence/not-ready.
    pub async fn stage_result(&self, launch: &ReservedLaunch) -> Result<LaunchStageResult> {
        let receipt: LaunchStageResult = self
            .post(
                launch.request.launch_id,
                &["stage-result"],
                LAUNCH_DISPATCH_HEADER,
                &launch.dispatch_token,
                &launch.request,
            )
            .await?;
        if receipt.output_id <= 0
            || receipt.object_id <= 0
            || receipt.validation_receipt <= 0
            || receipt.object_version.is_empty()
            || receipt.size_bytes == 0
        {
            return Err(ControlError::Response);
        }
        Ok(receipt)
    }
    async fn with_lease<T: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        lease: &LaunchLease,
        suffix: &str,
        body: &T,
    ) -> Result<O> {
        self.post(
            lease.handle.launch_id,
            &[suffix],
            LAUNCH_LEASE_HEADER,
            &lease.token,
            body,
        )
        .await
    }
    /// Destination is an already-created, controller-owned temporary file.
    /// No caller filename or redirect can select another body/host path.
    pub async fn download_input(
        &self,
        lease: &LaunchLease,
        input: &PinnedLaunchInput,
        destination: &Path,
        compressed_ceiling: u64,
    ) -> Result<()> {
        if input.pin_id <= 0
            || input.size_bytes == 0
            || input.size_bytes > compressed_ceiling
            || lease.purpose != LaunchLeasePurpose::Run
        {
            return Err(ControlError::Response);
        }
        let mut response = self
            .client
            .post(self.url(
                lease.handle.launch_id,
                &["inputs", &input.pin_id.to_string(), "body"],
            )?)
            .bearer_auth(self.credential().await.as_str())
            .header(LAUNCH_LEASE_HEADER, secret(&lease.token)?)
            .json(&LaunchLeaseRequest {
                handle: lease.handle.clone(),
            })
            .send()
            .await
            .map_err(|_| ControlError::Transport)?;
        if !response.status().is_success() {
            return Err(ControlError::Rejected(response.status().as_u16()));
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(LAUNCH_ARCHIVE_CONTENT_TYPE)
            || response
                .content_length()
                .is_some_and(|size| size != input.size_bytes)
        {
            return Err(ControlError::Response);
        }
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(destination)
            .await?;
        let mut size = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ControlError::Transport)?
        {
            size = size
                .checked_add(chunk.len() as u64)
                .ok_or(ControlError::Response)?;
            if size > input.size_bytes {
                return Err(ControlError::Response);
            }
            output.write_all(&chunk).await?;
        }
        if size != input.size_bytes {
            return Err(ControlError::Response);
        }
        output.sync_all().await?;
        Ok(())
    }
}
pub fn request(handle: &LaunchHandle) -> LaunchRequest {
    LaunchRequest {
        launch_id: handle.launch_id,
        admitted_digest: handle.admitted_digest.clone(),
    }
}
fn validate_lease(lease: &LaunchLease, expected: &LaunchRequest) -> Result<()> {
    let handle = &lease.handle;
    if handle.launch_id != expected.launch_id
        || handle.admitted_digest != expected.admitted_digest
        || handle.lease_id.is_nil()
        || handle.lease_generation <= 0
        || handle.engine_id <= 0
        || handle.engine_name_nonce.is_nil()
    {
        return Err(ControlError::Response);
    }
    Ok(())
}
fn validate_status(status: &LaunchStatus, expected: &LaunchRequest) -> Result<()> {
    status
        .validate_shape()
        .map_err(|_| ControlError::Response)?;
    if status.request.launch_id != expected.launch_id
        || status.request.admitted_digest != expected.admitted_digest
    {
        return Err(ControlError::Response);
    }
    Ok(())
}
fn secret(token: &LaunchToken) -> Result<reqwest::header::HeaderValue> {
    let mut value = reqwest::header::HeaderValue::from_str(token.as_str())
        .map_err(|_| ControlError::Configuration)?;
    value.set_sensitive(true);
    Ok(value)
}
async fn decode<T: DeserializeOwned>(mut response: Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(ControlError::Rejected(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > JSON_LIMIT as u64)
    {
        return Err(ControlError::Response);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ControlError::Transport)?
    {
        if bytes.len().saturating_add(chunk.len()) > JSON_LIMIT {
            return Err(ControlError::Response);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ControlError::Response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn generic_count_only_io_cannot_authorize_stdin() {
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
                lease_deadline: chrono::Utc::now() + chrono::Duration::seconds(60),
                engine_id: 5,
                engine_name_nonce: uuid::Uuid::new_v4(),
            },
            token: LaunchToken::new("owned-lease".into()).unwrap(),
        };
        let cp = ControlPlane::new(
            "http://127.0.0.1:1/".parse().unwrap(),
            LaunchToken::new("owned-controller".into()).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            cp.authorize_io(
                &lease,
                &LaunchToken::new("owned-stdin".into()).unwrap(),
                &AuthorizeLaunchIoRequest {
                    request,
                    channel: LaunchIoChannel::Stdin,
                    offset: 0,
                    bytes: 1
                }
            )
            .await,
            Err(ControlError::Response)
        ));
    }
    #[tokio::test]
    async fn cloned_clients_use_replaced_cp_credential_without_local_signing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for credential in ["owned-controller-first", "owned-controller-renewed"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    assert!(bytes.len() < 16384);
                }
                let headers = String::from_utf8(bytes).unwrap();
                assert!(headers.starts_with("GET /api/v1/workload-launches/engine HTTP/1.1"));
                assert!(headers
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {credential}")));
                assert!(!headers.contains("x-attune-launch-dispatch"));
                assert!(!headers.contains("x-attune-launch-lease"));
                let body = serde_json::to_vec(&LaunchEngineBinding {
                    engine_id: 5,
                    daemon_id: "owned-daemon".into(),
                    platform: LaunchPlatform::LinuxAmd64,
                })
                .unwrap();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        let client = ControlPlane::new(
            Url::parse(&format!("http://{address}/")).unwrap(),
            LaunchToken::new("owned-controller-first".into()).unwrap(),
        )
        .unwrap();
        let result = async {
            assert_eq!(client.engine_binding().await?.engine_id, 5);
            let other = client.clone();
            other
                .replace_credential(LaunchToken::new("owned-controller-renewed".into()).unwrap())
                .await;
            assert_eq!(client.engine_binding().await?.daemon_id, "owned-daemon");
            Ok::<_, ControlError>(())
        }
        .await;
        server.await.unwrap();
        result.unwrap();
    }
    #[tokio::test]
    async fn renewal_requires_durable_publication_before_credential_replacement() {
        use attune_common::workload_session::{RenewWorkloadSession, WorkloadSession};
        use axum::{
            extract::State,
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            routing::post,
            Json, Router,
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        async fn renew(
            State(deny): State<Arc<AtomicBool>>,
            headers: HeaderMap,
            Json(_): Json<RenewWorkloadSession>,
        ) -> axum::response::Response {
            if deny.load(Ordering::SeqCst)
                || headers.get("authorization").and_then(|v| v.to_str().ok())
                    != Some("Bearer cp-original")
                || headers.contains_key(LAUNCH_LEASE_HEADER)
                || headers.contains_key(LAUNCH_DISPATCH_HEADER)
            {
                return StatusCode::FORBIDDEN.into_response();
            }
            (
                [("cache-control", "no-store")],
                Json(WorkloadSession {
                    token: LaunchToken::new("cp-renewed".into()).unwrap(),
                    expires_at: chrono::Utc::now() + chrono::Duration::seconds(120),
                }),
            )
                .into_response()
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let deny = Arc::new(AtomicBool::new(false));
        let router = Router::new()
            .route("/api/v1/workload-launches/session", post(renew))
            .with_state(deny.clone());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let client = ControlPlane::new(
            Url::parse(&format!("http://{address}/")).unwrap(),
            LaunchToken::new("cp-original".into()).unwrap(),
        )
        .unwrap();
        let expiry = chrono::Utc::now() + chrono::Duration::seconds(90);
        let failed = client
            .renew_session(expiry, |_| {
                Err(std::io::Error::other("owned persistence failure"))
            })
            .await;
        let unchanged = client.credential().await;
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = crate::session_store::SessionStore::open(root.path().into()).unwrap();
        let renewed = client
            .renew_session(expiry, |session| store.persist(session))
            .await;
        let installed = client.credential().await;
        let retained = store.retained();
        deny.store(true, Ordering::SeqCst);
        let rejected = client.renew_session(expiry, |_| Ok(())).await;
        stop.send(()).unwrap();
        server.await.unwrap();
        assert!(matches!(failed, Err(ControlError::Io(_))));
        assert_eq!(unchanged.as_str(), "cp-original");
        assert_eq!(renewed.unwrap().token.as_str(), "cp-renewed");
        assert_eq!(installed.as_str(), "cp-renewed");
        assert_eq!(retained.unwrap().unwrap().token.as_str(), "cp-renewed");
        assert!(matches!(rejected, Err(ControlError::Rejected(403))));
    }
}
