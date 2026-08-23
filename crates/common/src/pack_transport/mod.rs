//! Pack file transport abstraction
//!
//! Provides a transport layer for distributing pack file contents to workers
//! and sensors. When services share a mounted `packs_data` volume, the
//! [`VolumePackTransport`] is a no-op (files are already present). When
//! services do NOT share a volume, [`ApiPackTransport`] downloads pack
//! archives from the API and extracts them locally.
//!
//! Detection follows the same sentinel pattern as `ArtifactFileTransport`:
//! if `.attune-packs-sentinel` exists in `packs_base_dir`, the volume is
//! shared and no downloads are needed.

mod api;
mod volume;

pub use api::ApiPackTransport;
pub use volume::VolumePackTransport;

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::WorkerTokenProvider;
use crate::error::Result;

/// Sentinel file written by the API at startup to indicate a shared pack volume.
pub const PACKS_SENTINEL_FILE: &str = ".attune-packs-sentinel";

/// Abstraction over pack file distribution.
///
/// Workers and sensors call these methods at startup and when handling
/// `pack.registered` / `pack.deleted` MQ events to ensure they have
/// the pack files they need locally.
#[async_trait]
pub trait PackFileTransport: Send + Sync + std::fmt::Debug {
    /// Download and extract the pack's file tree to the local `packs_base_dir`.
    ///
    /// For volume transport this is a no-op (files are already present).
    /// For API transport this downloads a tarball and extracts it.
    async fn sync_pack(&self, pack_ref: &str) -> Result<()>;

    /// Download a staged pack candidate for a test run without replacing the
    /// active local pack. Returns the extracted candidate directory.
    async fn sync_pack_test_candidate(
        &self,
        pack_ref: &str,
        pack_install_id: i64,
        candidate_access_token: Option<&str>,
    ) -> Result<PathBuf>;

    /// Remove the local copy of a pack's file tree.
    ///
    /// For volume transport this is a no-op (volume is managed externally).
    /// For API transport this deletes the local directory.
    async fn remove_pack(&self, pack_ref: &str) -> Result<()>;

    /// Check whether a pack's files exist locally.
    async fn is_pack_local(&self, pack_ref: &str) -> bool;

    /// Returns the transport mode name for diagnostics / logging.
    fn transport_mode(&self) -> &'static str;
}

/// Detect whether the packs volume is shared and build transport backed by a
/// refreshable worker token provider.
pub fn build_pack_transport_with_worker_token_provider(
    packs_base_dir: &str,
    api_url: Option<&str>,
    token_provider: Option<Arc<WorkerTokenProvider>>,
) -> Box<dyn PackFileTransport> {
    let sentinel_path = std::path::Path::new(packs_base_dir).join(PACKS_SENTINEL_FILE);

    if sentinel_path.exists() {
        tracing::info!(
            "Packs sentinel found at {:?} — using volume transport",
            sentinel_path
        );
        Box::new(VolumePackTransport::new(packs_base_dir))
    } else if let (Some(url), Some(provider)) = (api_url, token_provider) {
        tracing::info!(
            "No packs sentinel at {:?} — using API transport ({})",
            sentinel_path,
            url
        );
        Box::new(ApiPackTransport::new_with_worker_token_provider(
            url,
            provider,
            packs_base_dir,
        ))
    } else {
        tracing::warn!(
            "No packs sentinel and no API credentials — falling back to volume transport"
        );
        Box::new(VolumePackTransport::new(packs_base_dir))
    }
}

/// Write the packs sentinel file (called by the API at startup).
pub fn write_packs_sentinel(packs_base_dir: &str, api_url: &str) -> std::io::Result<()> {
    let sentinel_path = std::path::Path::new(packs_base_dir).join(PACKS_SENTINEL_FILE); // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- packs_base_dir is a trusted deployment root and the sentinel filename is constant.
    let content = serde_json::json!({
        "api_url": api_url,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(&sentinel_path, serde_json::to_string_pretty(&content)?)?;
    tracing::debug!("Wrote packs sentinel to {:?}", sentinel_path);
    Ok(())
}
