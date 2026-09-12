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
mod path;
mod volume;

pub use api::ApiTransport;
pub use detection::{detect_transport_mode, TransportMode};
pub use path::{
    ensure_checked_parent_dirs, reject_hard_linked_regular_file, resolve_checked_path,
    ValidatedRelativePath,
};
pub use volume::VolumeTransport;

use async_trait::async_trait;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

use crate::auth::WorkerTokenProvider;
use crate::error::{Error, Result};

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

    /// Stream a local file into the transport and return its size.
    async fn write_file_from_path(
        &self,
        file_path: &str,
        source_path: &Path,
        content_type: Option<&str>,
    ) -> Result<u64> {
        let _ = (file_path, source_path, content_type);
        Err(Error::invalid_state(
            "transport does not support streamed file uploads",
        ))
    }

    /// Check whether a file exists.
    async fn file_exists(&self, file_path: &str) -> Result<bool>;

    /// Return the file size in bytes, or `None` if the file does not exist.
    async fn file_size(&self, file_path: &str) -> Result<Option<u64>>;

    /// Publish a completed staging file and return its durable size.
    async fn complete_file(&self, file_path: &str) -> Result<Option<u64>> {
        self.file_size(file_path).await
    }

    /// Delete a file. No error if it does not exist.
    async fn delete_file(&self, file_path: &str) -> Result<()>;

    async fn append_log_file(&self, file_path: &str, content: &[u8]) -> Result<()> {
        let _ = (file_path, content);
        Err(Error::invalid_state(
            "transport does not support shared log files",
        ))
    }

    async fn commit_log_segment(
        &self,
        artifact_version: i64,
        sequence: i64,
        content: &[u8],
    ) -> Result<()> {
        let _ = (artifact_version, sequence, content);
        Err(Error::invalid_state(
            "transport does not support immutable log segments",
        ))
    }

    async fn seal_log_stream(&self, artifact_version: i64, truncated: bool) -> Result<()> {
        let _ = (artifact_version, truncated);
        Err(Error::invalid_state(
            "transport does not support immutable log streams",
        ))
    }

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
) -> Result<Box<dyn ArtifactFileTransport>> {
    match config_transport {
        TransportMode::Volume => Ok(Box::new(VolumeTransport::new(artifacts_dir))),
        TransportMode::Api => {
            validate_api_artifact_transport(
                api_url,
                auth_token.is_some_and(|token| !token.trim().is_empty()),
            )?;
            Ok(Box::new(ApiTransport::new(
                api_url.expect("API URL was validated"),
                auth_token.expect("auth token was validated"),
                artifacts_dir,
            )))
        }
        TransportMode::Auto => {
            let detected = detect_transport_mode(artifacts_dir);
            match detected {
                TransportMode::Volume => Ok(Box::new(VolumeTransport::new(artifacts_dir))),
                _ => {
                    let url = api_url.unwrap_or("http://localhost:8080");
                    let token = auth_token.unwrap_or("");
                    Ok(Box::new(ApiTransport::new(url, token, artifacts_dir)))
                }
            }
        }
    }
}

/// Validate the inputs required before starting an API-backed artifact consumer.
pub fn validate_api_artifact_transport(
    api_url: Option<&str>,
    has_worker_token: bool,
) -> Result<()> {
    api_url
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| {
            Error::configuration(
                "artifacts.transport is 'api' but ATTUNE_API_URL is missing or empty",
            )
        })?;
    if !has_worker_token {
        return Err(Error::configuration(
            "artifacts.transport is 'api' but worker token configuration is missing",
        ));
    }

    Ok(())
}

/// Build transport using a refreshable worker token provider.
pub fn build_transport_with_worker_token_provider(
    artifacts_dir: &str,
    api_url: Option<&str>,
    token_provider: Option<Arc<WorkerTokenProvider>>,
    config_transport: &TransportMode,
) -> Result<Box<dyn ArtifactFileTransport>> {
    match config_transport {
        TransportMode::Volume => match token_provider {
            Some(provider) => Ok(Box::new(VolumeTransport::new_with_completion_api(
                artifacts_dir,
                api_url.unwrap_or("http://localhost:8080"),
                provider,
            ))),
            None => Ok(Box::new(VolumeTransport::new(artifacts_dir))),
        },
        TransportMode::Api => {
            validate_api_artifact_transport(api_url, token_provider.is_some())?;
            let provider = token_provider.ok_or_else(|| {
                Error::configuration(
                    "artifacts.transport is 'api' but worker token configuration is missing",
                )
            })?;
            Ok(Box::new(ApiTransport::new_with_worker_token_provider(
                api_url.expect("API URL was validated"),
                provider,
                artifacts_dir,
            )))
        }
        TransportMode::Auto => {
            let detected = detect_transport_mode(artifacts_dir);
            match detected {
                TransportMode::Volume => match token_provider {
                    Some(provider) => Ok(Box::new(VolumeTransport::new_with_completion_api(
                        artifacts_dir,
                        api_url.unwrap_or("http://localhost:8080"),
                        provider,
                    ))),
                    None => Ok(Box::new(VolumeTransport::new(artifacts_dir))),
                },
                _ => {
                    let url = api_url.unwrap_or("http://localhost:8080");
                    if let Some(provider) = token_provider {
                        Ok(Box::new(ApiTransport::new_with_worker_token_provider(
                            url,
                            provider,
                            artifacts_dir,
                        )))
                    } else {
                        Ok(Box::new(VolumeTransport::new(artifacts_dir)))
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
    let relative = ValidatedRelativePath::new(file_path)?;
    if transport.transport_mode() == "volume" {
        return Ok(None);
    }

    let local_path = match resolve_checked_path(artifacts_dir, &relative).await {
        Ok(path) => path,
        Err(Error::Io(message)) if message.contains("No such file or directory") => {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    let metadata = match tokio::fs::symlink_metadata(&local_path).await {
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
    reject_hard_linked_regular_file(&local_path, &metadata)?;

    let size = transport
        .write_file_from_path(file_path, &local_path, content_type)
        .await?;
    Ok(Some(size))
}

#[cfg(test)]
mod tests {
    use super::detection::write_sentinel;
    use super::*;
    use crate::auth::jwt::JwtConfig;
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

        async fn write_file_from_path(
            &self,
            file_path: &str,
            source_path: &Path,
            content_type: Option<&str>,
        ) -> Result<u64> {
            let content = tokio::fs::read(source_path)
                .await
                .map_err(|error| Error::Io(error.to_string()))?;
            self.write_file(file_path, &content, content_type).await?;
            Ok(content.len() as u64)
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

        async fn open_reader(&self, file_path: &str, offset: u64) -> Result<BoxAsyncReader> {
            let mut content = self
                .files
                .lock()
                .await
                .get(file_path)
                .cloned()
                .unwrap_or_default();
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
        write_sentinel(temp.path().to_str().unwrap(), "http://api:8080").unwrap();

        let error = build_transport_with_worker_token_provider(
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
        let error = build_transport_with_worker_token_provider(
            temp.path().to_str().unwrap(),
            Some("http://api:8080"),
            None,
            &TransportMode::Api,
        )
        .unwrap_err();

        assert!(error.to_string().contains("worker token configuration"));
    }

    #[test]
    fn static_token_api_mode_rejects_blank_token() {
        let error = build_transport(
            "/tmp/artifacts",
            Some("http://api:8080"),
            Some(" "),
            &TransportMode::Api,
        )
        .unwrap_err();

        assert!(error.to_string().contains("worker token configuration"));
    }

    #[test]
    fn auto_mode_retains_sentinel_compatibility() {
        let temp = tempfile::tempdir().unwrap();
        write_sentinel(temp.path().to_str().unwrap(), "http://api:8080").unwrap();

        let transport = build_transport_with_worker_token_provider(
            temp.path().to_str().unwrap(),
            None,
            None,
            &TransportMode::Auto,
        )
        .unwrap();

        assert_eq!(transport.transport_mode(), "volume");
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_sync_local_file_to_transport_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        tokio::fs::write(outside.path().join("secret"), b"secret")
            .await
            .expect("write outside file");
        symlink(outside.path(), temp_dir.path().join("link")).expect("create symlink");

        let error = sync_local_file_to_transport(
            temp_dir.path(),
            &TestTransport::default(),
            "link/secret",
            None,
        )
        .await
        .expect_err("symlink must be rejected");
        assert!(matches!(error, Error::PermissionDenied(_)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_sync_local_file_to_transport_rejects_hard_link() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let staged_path = temp_dir.path().join("staged.txt");
        tokio::fs::write(&staged_path, b"shared")
            .await
            .expect("write staged file");
        std::fs::hard_link(&staged_path, temp_dir.path().join("alias.txt"))
            .expect("create hard link");

        let error = sync_local_file_to_transport(
            temp_dir.path(),
            &TestTransport::default(),
            "staged.txt",
            None,
        )
        .await
        .expect_err("hard-linked file must be rejected");
        assert!(matches!(error, Error::PermissionDenied(_)));
    }
}
