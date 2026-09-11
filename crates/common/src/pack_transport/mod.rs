//! Pack file transport abstraction
//!
//! Provides a transport layer for distributing pack file contents to workers
//! and sensors. When services share a mounted `packs_data` volume, the
//! [`VolumePackTransport`] is a no-op (files are already present). When
//! services do NOT share a volume, [`ApiPackTransport`] downloads pack
//! archives from the API and extracts them locally.
//!
//! Explicit `volume` and `api` modes ignore sentinel files. Legacy `auto` mode
//! checks `.attune-packs-sentinel` in `packs_base_dir`.

mod api;
mod volume;

pub use api::ApiPackTransport;
pub use volume::VolumePackTransport;

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

use crate::artifact_transport::TransportMode;
use crate::auth::WorkerTokenProvider;
use crate::error::{Error, Result};

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
    async fn sync_release(
        &self,
        pack_ref: &str,
        release_id: i64,
        release_digest: &str,
    ) -> Result<PathBuf>;

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
    async fn is_release_local(&self, pack_ref: &str, release_digest: &str) -> bool;

    /// Returns the transport mode name for diagnostics / logging.
    fn transport_mode(&self) -> &'static str;
}

pub fn release_cache_path(
    packs_base_dir: impl AsRef<std::path::Path>,
    pack_ref: &str,
    release_digest: &str,
) -> Result<PathBuf> {
    crate::schema::RefValidator::validate_pack_ref(pack_ref)?;
    if release_digest.len() != 64
        || !release_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::validation(
            "Pack release digest must be a lowercase SHA-256 digest",
        ));
    }
    Ok(packs_base_dir
        .as_ref()
        .join(".releases")
        .join("sha256")
        .join(release_digest)
        .join("pack"))
}

/// Validate the inputs required before starting an API-backed pack consumer.
pub fn validate_api_pack_transport(api_url: Option<&str>, has_worker_token: bool) -> Result<()> {
    api_url
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| {
            crate::Error::configuration(
                "packs.transport is 'api' but ATTUNE_API_URL is missing or empty",
            )
        })?;
    if !has_worker_token {
        return Err(crate::Error::configuration(
            "packs.transport is 'api' but worker token configuration is missing",
        ));
    }

    Ok(())
}

/// Build the configured pack transport with a refreshable worker token provider.
pub fn build_pack_transport_with_worker_token_provider(
    packs_base_dir: &str,
    api_url: Option<&str>,
    token_provider: Option<Arc<WorkerTokenProvider>>,
    configured_mode: &TransportMode,
) -> Result<Box<dyn PackFileTransport>> {
    let sentinel_path = std::path::Path::new(packs_base_dir).join(PACKS_SENTINEL_FILE);

    let mode = match configured_mode {
        TransportMode::Auto if sentinel_path.exists() => {
            tracing::info!(path = %sentinel_path.display(), "Packs sentinel found; using volume transport");
            TransportMode::Volume
        }
        TransportMode::Auto => {
            tracing::info!(path = %sentinel_path.display(), "No packs sentinel found; using API transport");
            TransportMode::Api
        }
        mode => mode.clone(),
    };

    match mode {
        TransportMode::Volume => Ok(Box::new(VolumePackTransport::new(packs_base_dir))),
        TransportMode::Api => {
            validate_api_pack_transport(api_url, token_provider.is_some())?;
            let url = api_url.expect("API URL was validated");
            let provider = token_provider.ok_or_else(|| {
                crate::Error::configuration(
                    "packs.transport is 'api' but worker token configuration is missing",
                )
            })?;
            Ok(Box::new(ApiPackTransport::new_with_worker_token_provider(
                url,
                provider,
                packs_base_dir,
            )))
        }
        TransportMode::Auto => unreachable!("auto mode is resolved before transport construction"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::JwtConfig;

    fn token_provider() -> Arc<WorkerTokenProvider> {
        Arc::new(WorkerTokenProvider::new(
            1,
            "worker-test",
            JwtConfig {
                secret: "test-secret".to_string(),
                access_token_expiration: 3600,
                refresh_token_expiration: 7200,
            },
        ))
    }

    #[test]
    fn api_mode_requires_url_even_when_volume_sentinel_exists() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(PACKS_SENTINEL_FILE), "sentinel").unwrap();
        let error = build_pack_transport_with_worker_token_provider(
            temp.path().to_str().unwrap(),
            None,
            Some(token_provider()),
            &TransportMode::Api,
        )
        .unwrap_err();

        assert!(error.to_string().contains("ATTUNE_API_URL"));
    }

    #[test]
    fn api_mode_requires_worker_token_provider_without_falling_back() {
        let temp = tempfile::tempdir().unwrap();
        let error = build_pack_transport_with_worker_token_provider(
            temp.path().to_str().unwrap(),
            Some("http://api:8080"),
            None,
            &TransportMode::Api,
        )
        .unwrap_err();

        assert!(error.to_string().contains("worker token configuration"));
    }

    #[test]
    fn auto_mode_retains_sentinel_compatibility() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(PACKS_SENTINEL_FILE), "sentinel").unwrap();

        let transport = build_pack_transport_with_worker_token_provider(
            temp.path().to_str().unwrap(),
            None,
            None,
            &TransportMode::Auto,
        )
        .unwrap();

        assert_eq!(transport.transport_mode(), "volume");
    }
}
