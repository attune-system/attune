//! API-based pack transport.
//!
//! Downloads pack archives from the API and extracts them to the local
//! `packs_base_dir`. Used by remote workers/sensors without a shared volume.

use async_trait::async_trait;
use reqwest::Client;
use std::sync::Arc;
use tracing::{debug, info};

use super::PackFileTransport;
use crate::auth::WorkerTokenProvider;
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
enum AuthTokenSource {
    Static(String),
    WorkerProvider(Arc<WorkerTokenProvider>),
}

impl AuthTokenSource {
    fn token(&self) -> Result<String> {
        match self {
            Self::Static(token) => Ok(token.clone()),
            Self::WorkerProvider(provider) => provider
                .token()
                .map_err(|e| Error::Internal(format!("Failed to get worker auth token: {e}"))),
        }
    }

    fn can_force_refresh(&self) -> bool {
        matches!(self, Self::WorkerProvider(_))
    }

    fn force_refresh(&self) -> Result<String> {
        match self {
            Self::Static(token) => Ok(token.clone()),
            Self::WorkerProvider(provider) => provider
                .force_refresh()
                .map_err(|e| Error::Internal(format!("Failed to refresh worker auth token: {e}"))),
        }
    }
}

/// HTTP-based pack transport that downloads pack archives from the API.
#[derive(Debug, Clone)]
pub struct ApiPackTransport {
    api_url: String,
    auth_token_source: AuthTokenSource,
    packs_base_dir: String,
    client: Client,
}

impl ApiPackTransport {
    pub fn new(api_url: &str, auth_token: &str, packs_base_dir: &str) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            auth_token_source: AuthTokenSource::Static(auth_token.to_string()),
            packs_base_dir: packs_base_dir.to_string(),
            client,
        }
    }

    pub fn new_with_worker_token_provider(
        api_url: &str,
        token_provider: Arc<WorkerTokenProvider>,
        packs_base_dir: &str,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            auth_token_source: AuthTokenSource::WorkerProvider(token_provider),
            packs_base_dir: packs_base_dir.to_string(),
            client,
        }
    }

    /// Update the auth token (e.g., after token refresh).
    pub fn set_auth_token(&mut self, token: &str) {
        self.auth_token_source = AuthTokenSource::Static(token.to_string());
    }

    fn archive_url(&self, pack_ref: &str) -> String {
        format!(
            "{}/api/v1/internal/packs/{}/archive",
            self.api_url, pack_ref
        )
    }
}

#[async_trait]
impl PackFileTransport for ApiPackTransport {
    async fn sync_pack(&self, pack_ref: &str) -> Result<()> {
        let url = self.archive_url(pack_ref);
        info!(
            "Downloading pack '{}' from {} to {}",
            pack_ref, url, self.packs_base_dir
        );

        let token = self.auth_token_source.token()?;
        let mut response = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to download pack '{}': {}", pack_ref, e))
            })?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && self.auth_token_source.can_force_refresh()
        {
            let refreshed_token = self.auth_token_source.force_refresh()?;
            response = self
                .client
                .get(&url)
                .bearer_auth(&refreshed_token)
                .send()
                .await
                .map_err(|e| {
                    Error::Internal(format!(
                        "Failed to retry pack download '{}': {}",
                        pack_ref, e
                    ))
                })?;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Internal(format!(
                "Pack download for '{}' returned {}: {}",
                pack_ref, status, body
            )));
        }

        let archive_bytes = response.bytes().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to read pack archive for '{}': {}",
                pack_ref, e
            ))
        })?;

        debug!(
            "Downloaded {} bytes for pack '{}', extracting...",
            archive_bytes.len(),
            pack_ref
        );

        // Extract tar.gz to packs_base_dir
        let packs_dir = self.packs_base_dir.clone();
        let pack_ref_owned = pack_ref.to_string();
        tokio::task::spawn_blocking(move || {
            use flate2::read::GzDecoder;
            use std::io::Cursor;

            let cursor = Cursor::new(archive_bytes);
            let decoder = GzDecoder::new(cursor);
            let mut archive = tar::Archive::new(decoder);

            // Safety: validate entries before extracting
            let dest = std::path::Path::new(&packs_dir);
            archive.set_overwrite(true);
            archive.set_unpack_xattrs(false);
            archive.set_preserve_permissions(false);

            // Validate each entry path
            for entry in archive.entries()? {
                let mut entry = entry?;
                let path = entry.path()?;

                // Reject path traversal
                if path
                    .components()
                    .any(|c| c == std::path::Component::ParentDir)
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Path traversal in archive entry: {:?}", path),
                    ));
                }

                // Reject absolute paths
                if path.is_absolute() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Absolute path in archive entry: {:?}", path),
                    ));
                }

                entry.unpack_in(dest)?;
            }

            Ok::<_, std::io::Error>(())
        })
        .await
        .map_err(|e| {
            Error::Internal(format!(
                "Pack extraction task panicked for '{}': {}",
                pack_ref_owned, e
            ))
        })?
        .map_err(|e| {
            Error::Internal(format!(
                "Failed to extract pack '{}': {}",
                pack_ref_owned, e
            ))
        })?;

        info!("Pack '{}' synced successfully", pack_ref_owned);
        Ok(())
    }

    async fn remove_pack(&self, pack_ref: &str) -> Result<()> {
        let pack_dir = std::path::Path::new(&self.packs_base_dir).join(pack_ref);
        if pack_dir.is_dir() {
            info!("Removing local pack directory for '{}'", pack_ref);
            tokio::fs::remove_dir_all(&pack_dir).await.map_err(|e| {
                Error::Internal(format!(
                    "Failed to remove pack directory {:?}: {}",
                    pack_dir, e
                ))
            })?;
        } else {
            debug!(
                "Pack '{}' directory not found locally, nothing to remove",
                pack_ref
            );
        }
        Ok(())
    }

    async fn is_pack_local(&self, pack_ref: &str) -> bool {
        let pack_dir = std::path::Path::new(&self.packs_base_dir).join(pack_ref);
        pack_dir.is_dir()
    }

    fn transport_mode(&self) -> &'static str {
        "api"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_api_transport_is_pack_local() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        );

        assert!(!transport.is_pack_local("mypack").await);

        std::fs::create_dir(tmp.path().join("mypack")).unwrap();
        assert!(transport.is_pack_local("mypack").await);
    }

    #[tokio::test]
    async fn test_api_transport_remove_pack() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        );

        // Create a pack dir with a file
        let pack_dir = tmp.path().join("mypack");
        std::fs::create_dir(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("pack.yaml"), "ref: mypack").unwrap();

        assert!(transport.is_pack_local("mypack").await);
        transport.remove_pack("mypack").await.unwrap();
        assert!(!transport.is_pack_local("mypack").await);
    }

    #[tokio::test]
    async fn test_api_transport_remove_nonexistent_pack() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        );

        // Should not error
        transport.remove_pack("nonexistent").await.unwrap();
    }
}
