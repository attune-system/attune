//! Typed client for the authoritative launch ledger. The controller transports
//! credentials in headers only and never logs response bodies/invocation data.
use attune_common::workload_launch::*;
use reqwest::{Client, Response, Url};
use serde::{de::DeserializeOwned, Serialize};
use std::{path::Path, time::Duration};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

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
    credential: LaunchToken,
}
impl ControlPlane {
    pub fn new(base: Url, credential: LaunchToken) -> Result<Self> {
        if !matches!(base.scheme(), "https" | "http")
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
            credential,
        })
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
            .bearer_auth(self.credential.as_str())
            .header(header, token.as_str())
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
                .bearer_auth(self.credential.as_str())
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
    pub async fn authorize_io(
        &self,
        lease: &LaunchLease,
        token: &LaunchToken,
        operation: &AuthorizeLaunchIoRequest,
    ) -> Result<LaunchIoAuthorization> {
        if operation.request.launch_id != lease.handle.launch_id
            || operation.request.admitted_digest != lease.handle.admitted_digest
        {
            return Err(ControlError::Response);
        }
        let authorization: LaunchIoAuthorization = decode(
            self.client
                .post(self.url(lease.handle.launch_id, &["authorize-io"])?)
                .bearer_auth(self.credential.as_str())
                .header(LAUNCH_LEASE_HEADER, lease.token.as_str())
                .header(LAUNCH_IO_HEADER, token.as_str())
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
            .bearer_auth(self.credential.as_str())
            .header(LAUNCH_LEASE_HEADER, lease.token.as_str())
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
