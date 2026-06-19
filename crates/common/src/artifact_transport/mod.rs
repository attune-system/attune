//! Artifact file transport abstraction
//!
//! This module provides a transport layer for artifact file content,
//! decoupling metadata operations (always via DB) from file content
//! transfer (via shared volume or API).
//!
//! Two implementations:
//! - [`VolumeTransport`]: Direct filesystem I/O on a shared volume (fast path)
//! - [`ApiTransport`]: HTTP-based upload/download via API internal endpoints (remote workers)
//!
//! Workers and sensors auto-detect which transport to use at startup
//! by checking for a sentinel file written by the API.

mod api;
pub mod detection;
mod volume;

pub use api::ApiTransport;
pub use detection::{detect_transport_mode, TransportMode};
pub use volume::VolumeTransport;

use async_trait::async_trait;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::auth::WorkerTokenProvider;
use crate::error::{Error, Result};

/// Async writer returned by `create_writer`.
pub type BoxAsyncWriter = Pin<Box<dyn AsyncWrite + Send + Sync>>;
/// Async reader returned by `open_reader`.
pub type BoxAsyncReader = Pin<Box<dyn AsyncRead + Send + Sync>>;

/// Abstraction over artifact file content storage.
///
/// Metadata (artifact/version rows) continues to flow through direct DB access.
/// This trait handles only the *file bytes* — writing, reading, appending,
/// existence checks, and cleanup.
#[async_trait]
pub trait ArtifactFileTransport: Send + Sync + std::fmt::Debug {
    /// Write complete file content, creating parent directories as needed.
    async fn write_file(
        &self,
        file_path: &str,
        content: &[u8],
        content_type: Option<&str>,
    ) -> Result<()>;

    /// Read complete file content.
    async fn read_file(&self, file_path: &str) -> Result<Vec<u8>>;

    /// Append bytes to an existing file (creates if missing).
    async fn append_file(&self, file_path: &str, content: &[u8]) -> Result<()>;

    /// Check whether a file exists.
    async fn file_exists(&self, file_path: &str) -> Result<bool>;

    /// Return the file size in bytes, or `None` if the file does not exist.
    async fn file_size(&self, file_path: &str) -> Result<Option<u64>>;

    /// Delete a file. No error if it does not exist.
    async fn delete_file(&self, file_path: &str) -> Result<()>;

    /// Rename / move a file within the transport.
    async fn rename_file(&self, from: &str, to: &str) -> Result<()>;

    /// Create a streaming writer for live output capture.
    async fn create_writer(&self, file_path: &str) -> Result<BoxAsyncWriter>;

    /// Open a streaming reader, optionally starting at `offset` bytes.
    async fn open_reader(&self, file_path: &str, offset: u64) -> Result<BoxAsyncReader>;

    /// Returns the transport mode name for diagnostics / logging.
    fn transport_mode(&self) -> &'static str;

    /// Returns the base directory for resolving absolute paths (if applicable).
    fn base_dir(&self) -> &str;

    /// Ensure parent directories exist for a given file path.
    /// Default implementation is a no-op (API transport handles this server-side).
    async fn ensure_parent_dirs(&self, _file_path: &str) -> Result<()> {
        Ok(())
    }
}

/// Build the appropriate transport from config + detection.
pub fn build_transport(
    artifacts_dir: &str,
    api_url: Option<&str>,
    auth_token: Option<&str>,
    config_transport: &TransportMode,
) -> Box<dyn ArtifactFileTransport> {
    match config_transport {
        TransportMode::Volume => Box::new(VolumeTransport::new(artifacts_dir)),
        TransportMode::Api => {
            let url = api_url.unwrap_or("http://localhost:8080");
            let token = auth_token.unwrap_or("");
            Box::new(ApiTransport::new(url, token, artifacts_dir))
        }
        TransportMode::Auto => {
            let detected = detect_transport_mode(artifacts_dir);
            match detected {
                TransportMode::Volume => Box::new(VolumeTransport::new(artifacts_dir)),
                _ => {
                    let url = api_url.unwrap_or("http://localhost:8080");
                    let token = auth_token.unwrap_or("");
                    Box::new(ApiTransport::new(url, token, artifacts_dir))
                }
            }
        }
    }
}

/// Build transport using a refreshable worker token provider.
pub fn build_transport_with_worker_token_provider(
    artifacts_dir: &str,
    api_url: Option<&str>,
    token_provider: Option<Arc<WorkerTokenProvider>>,
    config_transport: &TransportMode,
) -> Box<dyn ArtifactFileTransport> {
    match config_transport {
        TransportMode::Volume => Box::new(VolumeTransport::new(artifacts_dir)),
        TransportMode::Api => {
            let url = api_url.unwrap_or("http://localhost:8080");
            if let Some(provider) = token_provider {
                Box::new(ApiTransport::new_with_worker_token_provider(
                    url,
                    provider,
                    artifacts_dir,
                ))
            } else {
                Box::new(VolumeTransport::new(artifacts_dir))
            }
        }
        TransportMode::Auto => {
            let detected = detect_transport_mode(artifacts_dir);
            match detected {
                TransportMode::Volume => Box::new(VolumeTransport::new(artifacts_dir)),
                _ => {
                    let url = api_url.unwrap_or("http://localhost:8080");
                    if let Some(provider) = token_provider {
                        Box::new(ApiTransport::new_with_worker_token_provider(
                            url,
                            provider,
                            artifacts_dir,
                        ))
                    } else {
                        Box::new(VolumeTransport::new(artifacts_dir))
                    }
                }
            }
        }
    }
}

/// Copy a locally written artifact file into the configured artifact transport.
///
/// Standalone workers/sensor agents may expose `ATTUNE_ARTIFACTS_DIR` as a
/// local staging directory while the API stores artifacts on a separate volume.
/// In that mode, actions/sensors can still write to the usual file path and the
/// service copies the file to the API-backed transport during finalization.
pub async fn sync_local_file_to_transport(
    artifacts_dir: &Path,
    transport: &dyn ArtifactFileTransport,
    file_path: &str,
    content_type: Option<&str>,
) -> Result<Option<u64>> {
    if transport.transport_mode() == "volume" {
        return Ok(None);
    }

    let local_path = artifacts_dir.join(file_path);
    let metadata = match tokio::fs::metadata(&local_path).await {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::Io(format!(
                "Failed to stat local artifact file '{}': {}",
                local_path.display(),
                e
            )));
        }
    };

    if !metadata.is_file() {
        return Err(Error::Io(format!(
            "Local artifact path '{}' is not a regular file",
            local_path.display()
        )));
    }

    let content = tokio::fs::read(&local_path).await.map_err(|e| {
        Error::Io(format!(
            "Failed to read local artifact file '{}': {}",
            local_path.display(),
            e
        ))
    })?;
    transport
        .write_file(file_path, &content, content_type)
        .await?;
    Ok(Some(content.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Debug, Default)]
    struct TestTransport {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    #[async_trait]
    impl ArtifactFileTransport for TestTransport {
        async fn write_file(
            &self,
            file_path: &str,
            content: &[u8],
            _content_type: Option<&str>,
        ) -> Result<()> {
            self.files
                .lock()
                .await
                .insert(file_path.to_string(), content.to_vec());
            Ok(())
        }

        async fn read_file(&self, file_path: &str) -> Result<Vec<u8>> {
            Ok(self
                .files
                .lock()
                .await
                .get(file_path)
                .cloned()
                .unwrap_or_default())
        }

        async fn append_file(&self, file_path: &str, content: &[u8]) -> Result<()> {
            self.files
                .lock()
                .await
                .entry(file_path.to_string())
                .or_default()
                .extend_from_slice(content);
            Ok(())
        }

        async fn file_exists(&self, file_path: &str) -> Result<bool> {
            Ok(self.files.lock().await.contains_key(file_path))
        }

        async fn file_size(&self, file_path: &str) -> Result<Option<u64>> {
            Ok(self
                .files
                .lock()
                .await
                .get(file_path)
                .map(|content| content.len() as u64))
        }

        async fn delete_file(&self, file_path: &str) -> Result<()> {
            self.files.lock().await.remove(file_path);
            Ok(())
        }

        async fn rename_file(&self, from: &str, to: &str) -> Result<()> {
            let content = self.files.lock().await.remove(from);
            if let Some(content) = content {
                self.files.lock().await.insert(to.to_string(), content);
            }
            Ok(())
        }

        async fn create_writer(&self, _file_path: &str) -> Result<BoxAsyncWriter> {
            Err(Error::Internal(
                "test transport does not implement writers".to_string(),
            ))
        }

        async fn open_reader(&self, file_path: &str, offset: u64) -> Result<BoxAsyncReader> {
            let mut content = self.read_file(file_path).await?;
            let offset = offset.min(content.len() as u64) as usize;
            content.drain(..offset);
            Ok(Box::pin(std::io::Cursor::new(content)))
        }

        fn transport_mode(&self) -> &'static str {
            "api"
        }

        fn base_dir(&self) -> &str {
            "/unused"
        }
    }

    #[test]
    fn test_transport_mode_default() {
        let mode = TransportMode::default();
        assert!(matches!(mode, TransportMode::Auto));
    }

    #[tokio::test]
    async fn test_sync_local_file_to_transport_copies_staged_file() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let staged_rel_path = "pack/artifact/v1.txt";
        let staged_full_path = temp_dir.path().join(staged_rel_path);
        tokio::fs::create_dir_all(staged_full_path.parent().unwrap())
            .await
            .expect("create parent");
        tokio::fs::write(&staged_full_path, b"hello from standalone worker")
            .await
            .expect("write staged file");

        let transport = TestTransport::default();
        let copied = sync_local_file_to_transport(
            temp_dir.path(),
            &transport,
            staged_rel_path,
            Some("text/plain"),
        )
        .await
        .expect("sync succeeds");

        assert_eq!(copied, Some(28));
        assert_eq!(
            transport.files.lock().await.get(staged_rel_path).cloned(),
            Some(b"hello from standalone worker".to_vec())
        );
    }

    #[tokio::test]
    async fn test_sync_local_file_to_transport_skips_missing_file() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let transport = TestTransport::default();

        let copied = sync_local_file_to_transport(temp_dir.path(), &transport, "missing.txt", None)
            .await
            .expect("missing local file is not fatal");

        assert_eq!(copied, None);
        assert!(transport.files.lock().await.is_empty());
    }
}
