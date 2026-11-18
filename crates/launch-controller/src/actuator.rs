//! Trusted retained-launch reconciliation. No caller can supply a LaunchSpec,
//! engine selector or host path. Public methods begin with authenticated CP
//! claim/status and keep cleanup separate from Run authority.
use crate::{
    cache::{CacheError, CacheStore},
    control_plane::{ControlError, ControlPlane},
    engine::{DockerEngine, EngineError, EnginePolicy},
};
use attune_common::workload_launch::*;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ActuatorError {
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("provider creation/removal remains unsettled; no absence acknowledgement sent")]
    Unsettled,
    #[error("launch authority or retained phase does not match operation")]
    Authority,
    #[error("owned filesystem task failed")]
    FilesystemTask,
    #[error("launch reached its admitted timeout")]
    Timeout,
}
pub type Result<T> = std::result::Result<T, ActuatorError>;

pub struct Actuator {
    pub(crate) cp: ControlPlane,
    pub(crate) engine: DockerEngine,
    pub(crate) policy: EnginePolicy,
    pub(crate) binding: LaunchEngineBinding,
    pub(crate) cache: Arc<CacheStore>,
    pub(crate) compressed_ceiling: u64,
}
impl Actuator {
    /// Fetches original immutable binding even for a disabled engine, verifies
    /// actual daemon/platform, then proves exact named-volume/subpath binding.
    /// Probe is fixed controller-owned code, never a pack-supplied command.
    pub async fn initialize(
        cp: ControlPlane,
        engine: DockerEngine,
        policy: EnginePolicy,
        cache: Arc<CacheStore>,
        probe_image: PinnedImage,
        compressed_ceiling: u64,
    ) -> Result<Self> {
        policy.validate()?;
        if compressed_ceiling == 0 {
            return Err(ActuatorError::Authority);
        }
        let binding = cp.engine_binding().await?;
        engine.verify_engine_binding(&binding, &policy).await?;
        engine
            .image_environment_for_binding(&binding, &policy, &probe_image)
            .await?;
        engine
            .verify_cache_binding(&policy, &cache, &probe_image.reference)
            .await?;
        Ok(Self {
            cp,
            engine,
            policy,
            binding,
            cache,
            compressed_ceiling,
        })
    }
    /// Mutating Run behavior is implemented separately; retained claims never
    /// return private context and never call Engine start/attach/input APIs.
    pub async fn reconcile_retained(&self, request: &ReservedLaunch) -> Result<LaunchStatus> {
        match self.cp.claim(request).await? {
            ClaimedLaunch::Cleanup {
                manifest,
                status,
                lease,
            } => self.cleanup(&manifest, &status, &lease).await,
            ClaimedLaunch::Publication {
                manifest,
                status,
                lease,
            } => {
                self.publish(&manifest, &status, &lease).await?;
                Ok(self.cp.status(request).await?)
            }
            ClaimedLaunch::Capture { status, .. } | ClaimedLaunch::Stopped { status } => Ok(status),
            ClaimedLaunch::Active { .. } => Err(ActuatorError::Authority),
        }
    }
    pub(crate) async fn cleanup(
        &self,
        manifest: &LaunchManifest,
        status: &LaunchStatus,
        lease: &LaunchLease,
    ) -> Result<LaunchStatus> {
        if lease.purpose != LaunchLeasePurpose::Cleanup
            || manifest.engine_id != self.binding.engine_id
            || !matches!(
                status.state,
                LaunchState::CleanupRequired | LaunchState::StopRequired
            )
        {
            return Err(ActuatorError::Authority);
        }
        self.engine
            .verify_engine_binding(&self.binding, &self.policy)
            .await?;
        let intent = self.cp.create_intent(lease).await?;
        if intent.disposition != CreateDisposition::StopOnly {
            return Err(ActuatorError::Authority);
        }
        let stopped = if intent.create_nonce.is_nil() {
            self.cp
                .stop_ack(lease, ProviderCreateSettlement::NeverSubmitted, None)
                .await?
        } else {
            let cache = self.cache.clone();
            let saved_manifest = manifest.clone();
            let receipt =
                tokio::task::spawn_blocking(move || cache.removal_receipt(&saved_manifest))
                    .await
                    .map_err(|_| ActuatorError::FilesystemTask)??;
            let resource = if let Some(receipt) = receipt {
                if status
                    .resource
                    .as_ref()
                    .is_some_and(|expected| expected != &receipt)
                {
                    return Err(ActuatorError::Authority);
                }
                receipt
            } else {
                let resource = self
                    .engine
                    .remove_admitted_resource(manifest, status.resource.as_ref())
                    .await?
                    .ok_or(ActuatorError::Unsettled)?;
                let cache = self.cache.clone();
                let saved_manifest = manifest.clone();
                let saved_resource = resource.clone();
                tokio::task::spawn_blocking(move || {
                    cache.record_removal(&saved_manifest, &saved_resource)
                })
                .await
                .map_err(|_| ActuatorError::FilesystemTask)??;
                resource
            };
            self.cp
                .stop_ack(
                    lease,
                    ProviderCreateSettlement::Created {
                        create_nonce: intent.create_nonce,
                        resource: resource.clone(),
                    },
                    Some(resource),
                )
                .await?
        };
        if !matches!(
            stopped.publication,
            LaunchPublicationState::Pending | LaunchPublicationState::Uploading
        ) {
            let cache = self.cache.clone();
            let saved_manifest = manifest.clone();
            let saved_status = stopped.clone();
            tokio::task::spawn_blocking(move || {
                cache.cleanup_settled_private(&saved_manifest, &saved_status)
            })
            .await
            .map_err(|_| ActuatorError::FilesystemTask)??;
        }
        Ok(stopped)
    }
    pub(crate) async fn publish(
        &self,
        manifest: &LaunchManifest,
        status: &LaunchStatus,
        lease: &LaunchLease,
    ) -> Result<LaunchStageResult> {
        if lease.purpose != LaunchLeasePurpose::Publication
            || manifest.purpose != LaunchPurpose::Setup
            || manifest.engine_id != self.binding.engine_id
            || !status.permits_stage_publication()
        {
            return Err(ActuatorError::Authority);
        }
        self.engine
            .verify_engine_binding(&self.binding, &self.policy)
            .await?;
        let cache = self.cache.clone();
        let saved_manifest = manifest.clone();
        let saved_status = status.clone();
        let (body, size_bytes, sha256) = tokio::task::spawn_blocking(move || {
            let mut body = cache.archive_settled_stage(&saved_manifest, &saved_status)?;
            let size_bytes = body.metadata().map_err(CacheError::from)?.len();
            let mut hash = Sha256::new();
            let mut bytes = [0; 65536];
            loop {
                let count = body.read(&mut bytes).map_err(CacheError::from)?;
                if count == 0 {
                    break;
                }
                hash.update(&bytes[..count]);
            }
            body.seek(SeekFrom::Start(0)).map_err(CacheError::from)?;
            let sha256 = Sha256Digest::parse(
                hash.finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            )
            .map_err(|_| CacheError::PinMismatch)?;
            Ok::<_, CacheError>((body, size_bytes, sha256))
        })
        .await
        .map_err(|_| ActuatorError::FilesystemTask)??;
        let output = ReserveLaunchOutput {
            handle: lease.handle.clone(),
            kind: LaunchOutputKind::StageArchive,
            size_bytes,
            sha256,
        };
        let grant = self.cp.reserve_stage_output(lease, &output).await?;
        self.cp
            .upload_stage_output(lease, &output, &grant, body, self.compressed_ceiling)
            .await
            .map_err(ActuatorError::from)
    }
}
