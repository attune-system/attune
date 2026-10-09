//! API-based pack transport.
//!
//! Downloads pack archives from the API and extracts them to the local
//! `packs_base_dir`. Used by remote workers/sensors without a shared volume.

use async_trait::async_trait;
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

use super::{release_cache_path, PackFileTransport};
use crate::auth::WorkerTokenProvider;
use crate::error::{Error, Result};
use crate::pack_registry::PackStorage;
use crate::schema::RefValidator;

const MAX_ARCHIVE_BYTES: u64 = crate::config::PackUploadConfig::DEFAULT_MAX_EXTRACTED_SIZE_BYTES;
const RELEASE_READY_MARKER: &str = ".attune-release-ready";

struct DownloadedArchive {
    path: PathBuf,
    size: u64,
    sha256: [u8; 32],
}

impl Drop for DownloadedArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

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
    pub fn new(api_url: &str, auth_token: &str, packs_base_dir: &str) -> Result<Self> {
        let client = crate::http_client::build_http_client(|| {
            Client::builder().timeout(std::time::Duration::from_secs(300))
        })
        .map_err(|error| {
            Error::configuration(format!("Failed to build pack HTTP client: {error}"))
        })?;

        Ok(Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            auth_token_source: AuthTokenSource::Static(auth_token.to_string()),
            packs_base_dir: packs_base_dir.to_string(),
            client,
        })
    }

    pub fn new_with_worker_token_provider(
        api_url: &str,
        token_provider: Arc<WorkerTokenProvider>,
        packs_base_dir: &str,
    ) -> Result<Self> {
        let client = crate::http_client::build_http_client(|| {
            Client::builder().timeout(std::time::Duration::from_secs(300))
        })
        .map_err(|error| {
            Error::configuration(format!("Failed to build pack HTTP client: {error}"))
        })?;

        Ok(Self {
            api_url: api_url.trim_end_matches('/').to_string(),
            auth_token_source: AuthTokenSource::WorkerProvider(token_provider),
            packs_base_dir: packs_base_dir.to_string(),
            client,
        })
    }

    /// Update the auth token (e.g., after token refresh).
    pub fn set_auth_token(&mut self, token: &str) {
        self.auth_token_source = AuthTokenSource::Static(token.to_string());
    }

    fn archive_url(&self, release_id: i64) -> String {
        format!(
            "{}/api/v1/internal/pack-releases/{}/archive",
            self.api_url, release_id
        )
    }

    fn candidate_archive_url(&self, pack_install_id: i64) -> String {
        format!(
            "{}/api/v1/internal/pack-installs/{}/archive",
            self.api_url, pack_install_id
        )
    }

    async fn download_archive(
        &self,
        url: &str,
        subject: &str,
        candidate_access_token: Option<&str>,
    ) -> Result<DownloadedArchive> {
        let token = self.auth_token_source.token()?;
        let request = self.client.get(url).bearer_auth(&token);
        let request = if let Some(candidate_access_token) = candidate_access_token {
            request.header("x-attune-pack-candidate-token", candidate_access_token)
        } else {
            request
        };
        let mut response = request
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Failed to download {subject}: {e}")))?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && self.auth_token_source.can_force_refresh()
        {
            let refreshed_token = self.auth_token_source.force_refresh()?;
            let request = self.client.get(url).bearer_auth(&refreshed_token);
            let request = if let Some(candidate_access_token) = candidate_access_token {
                request.header("x-attune-pack-candidate-token", candidate_access_token)
            } else {
                request
            };
            response = request
                .send()
                .await
                .map_err(|e| Error::Internal(format!("Failed to retry {subject} download: {e}")))?;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Internal(format!(
                "{subject} download returned {status}: {body}"
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
        {
            return Err(Error::validation(format!(
                "{subject} exceeds the {} byte download limit",
                MAX_ARCHIVE_BYTES
            )));
        }

        let download_dir = Path::new(&self.packs_base_dir).join(".pack-downloads");
        tokio::fs::create_dir_all(&download_dir)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to create pack download directory: {e}"))
            })?;
        let path = download_dir.join(format!("{}.tar.gz", uuid::Uuid::new_v4()));
        let mut download = DownloadedArchive {
            path,
            size: 0,
            sha256: [0; 32],
        };
        let mut file = tokio::fs::File::create(&download.path).await.map_err(|e| {
            Error::Internal(format!("Failed to create {subject} staging file: {e}"))
        })?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read {subject}: {e}")))?
        {
            let next_size = download
                .size
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| Error::validation("Pack archive download size overflow"))?;
            if next_size > MAX_ARCHIVE_BYTES {
                return Err(Error::validation(format!(
                    "{subject} exceeds the {} byte download limit",
                    MAX_ARCHIVE_BYTES
                )));
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| Error::Internal(format!("Failed to stage {subject}: {e}")))?;
            hasher.update(&chunk);
            download.size = next_size;
        }
        file.flush()
            .await
            .map_err(|e| Error::Internal(format!("Failed to flush {subject}: {e}")))?;
        download.sha256 = hasher.finalize().into();
        Ok(download)
    }
}

#[async_trait]
impl PackFileTransport for ApiPackTransport {
    async fn sync_release(
        &self,
        pack_ref: &str,
        release_id: i64,
        release_digest: &str,
    ) -> Result<std::path::PathBuf> {
        RefValidator::validate_pack_ref(pack_ref)?;
        if release_id <= 0 {
            return Err(Error::validation("Pack release ID must be positive"));
        }
        let final_pack = release_cache_path(&self.packs_base_dir, pack_ref, release_digest)?;
        if release_is_ready(&final_pack, release_digest) {
            return Ok(final_pack);
        }
        let url = self.archive_url(release_id);
        info!(
            "Downloading pack '{}' from {} to {}",
            pack_ref, url, self.packs_base_dir
        );

        let archive = self
            .download_archive(&url, &format!("pack archive for '{pack_ref}'"), None)
            .await?;

        verify_archive_digest(archive.sha256, release_id, release_digest)?;

        debug!(
            "Downloaded {} bytes for pack '{}', extracting...",
            archive.size, pack_ref
        );

        let packs_dir = self.packs_base_dir.clone();
        let pack_ref_owned = pack_ref.to_string();
        let extraction_pack_ref = pack_ref_owned.clone();
        let extraction_digest = release_digest.to_string();
        tokio::task::spawn_blocking(move || {
            extract_release_archive(
                &archive.path,
                Path::new(&packs_dir),
                &extraction_pack_ref,
                &extraction_digest,
            )
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
        Ok(final_pack)
    }

    async fn sync_pack_test_candidate(
        &self,
        pack_ref: &str,
        pack_install_id: i64,
        candidate_access_token: Option<&str>,
    ) -> Result<std::path::PathBuf> {
        RefValidator::validate_pack_ref(pack_ref)?;
        if pack_install_id <= 0 {
            return Err(Error::validation("Pack install ID must be positive"));
        }
        let candidate_access_token = candidate_access_token
            .ok_or_else(|| Error::validation("Pack install candidate access token is required"))?;
        let archive = self
            .download_archive(
                &self.candidate_archive_url(pack_install_id),
                &format!("candidate archive for pack install {pack_install_id}"),
                Some(candidate_access_token),
            )
            .await?;
        let attempt_dir = Path::new(&self.packs_base_dir)
            .join(".pack-test-attempts")
            .join(pack_install_id.to_string());
        let pack_dir = attempt_dir.join(pack_ref);
        let packs_dir = self.packs_base_dir.clone();
        let pack_ref = pack_ref.to_string();
        let extraction_dir = attempt_dir.clone();
        let extraction = tokio::task::spawn_blocking(move || {
            let _ = std::fs::remove_dir_all(&extraction_dir);
            let file = std::fs::File::open(&archive.path)?;
            extract_pack_archive(file, &extraction_dir, &pack_ref)
        })
        .await
        .map_err(|e| Error::Internal(format!("Candidate pack extraction task panicked: {e}")))?;
        if let Err(error) = extraction {
            let _ = std::fs::remove_dir_all(&attempt_dir);
            return Err(Error::Internal(format!(
                "Failed to extract candidate pack: {error}"
            )));
        }
        debug!(packs_base_dir = %packs_dir, pack_install_id, "Candidate pack synced for testing");
        Ok(pack_dir)
    }

    async fn remove_pack(&self, pack_ref: &str) -> Result<()> {
        RefValidator::validate_pack_ref(pack_ref)?;
        let pack_dir = std::path::Path::new(&self.packs_base_dir).join(pack_ref); // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- RefValidator rejects separators and traversal components before joining to the trusted pack root.
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

    async fn remove_pack_releases(&self, pack_ref: &str, release_digests: &[String]) -> Result<()> {
        RefValidator::validate_pack_ref(pack_ref)?;
        let storage = PackStorage::new(&self.packs_base_dir);
        for digest in release_digests {
            let pack_dir = release_cache_path(&self.packs_base_dir, pack_ref, digest)?;
            storage.remove_release_tree(digest, &pack_dir.to_string_lossy())?;
        }
        Ok(())
    }

    async fn is_release_local(&self, pack_ref: &str, release_digest: &str) -> bool {
        release_cache_path(&self.packs_base_dir, pack_ref, release_digest)
            .is_ok_and(|path| release_is_ready(&path, release_digest))
    }

    fn transport_mode(&self) -> &'static str {
        "api"
    }
}

fn verify_archive_digest(digest: [u8; 32], release_id: i64, expected: &str) -> Result<()> {
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if actual == expected {
        Ok(())
    } else {
        Err(Error::validation(format!(
            "Pack release {release_id} digest mismatch: expected {expected}, got {actual}"
        )))
    }
}

fn extract_release_archive(
    archive_path: &Path,
    packs_dir: &Path,
    pack_ref: &str,
    release_digest: &str,
) -> std::io::Result<()> {
    let cache_root = packs_dir.join(".releases").join("sha256");
    let destination = cache_root.join(release_digest);
    if release_is_ready(&destination.join("pack"), release_digest) {
        return Ok(());
    }
    std::fs::create_dir_all(&cache_root)?;
    let temporary = cache_root.join(format!(".{release_digest}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let file = std::fs::File::open(archive_path)?;
        extract_pack_archive(file, &temporary, pack_ref)?;
        let extracted = temporary.join(pack_ref);
        std::fs::rename(extracted, temporary.join("pack"))?;
        std::fs::write(temporary.join(RELEASE_READY_MARKER), release_digest)?;
        match std::fs::rename(&temporary, &destination) {
            Ok(()) => Ok(()),
            Err(_) if release_is_ready(&destination.join("pack"), release_digest) => Ok(()),
            Err(error) => Err(error),
        }
    })();
    let _ = std::fs::remove_dir_all(&temporary);
    result
}

fn release_is_ready(pack_path: &Path, release_digest: &str) -> bool {
    pack_path.join("pack.yaml").is_file()
        && std::fs::read_to_string(
            pack_path
                .parent()
                .unwrap_or(pack_path)
                .join(RELEASE_READY_MARKER),
        )
        .is_ok_and(|marker| marker.trim() == release_digest)
}

fn extract_pack_archive(
    archive_reader: impl std::io::Read,
    packs_dir: &Path,
    pack_ref: &str,
) -> std::io::Result<()> {
    use flate2::read::GzDecoder;
    use std::fs;
    use std::io::{self, Read, Write};
    use tar::EntryType;

    RefValidator::validate_pack_ref(pack_ref)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    fs::create_dir_all(packs_dir)?;

    let staging_dir = packs_dir.join(format!(
        ".attune-pack-sync-{}-{}",
        pack_ref,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&staging_dir)?;

    let extraction_result = (|| {
        let decoder = GzDecoder::new(archive_reader);
        let mut archive = tar::Archive::new(decoder);
        archive.set_overwrite(false);
        archive.set_unpack_xattrs(false);
        archive.set_preserve_permissions(false);
        archive.set_preserve_mtime(false);

        let mut entry_count = 0_u32;
        let mut total_bytes = 0_u64;
        for entry in archive.entries()? {
            let mut entry = entry?;
            entry_count = entry_count.saturating_add(1);
            if entry_count > crate::config::PackUploadConfig::DEFAULT_MAX_FILE_COUNT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Pack archive contains too many entries",
                ));
            }

            let entry_type = entry.header().entry_type();
            if !matches!(entry_type, EntryType::Regular | EntryType::Directory) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Pack archive contains forbidden entry type {entry_type:?}"),
                ));
            }
            let declared_size = entry.header().size()?;
            if declared_size > crate::config::PackUploadConfig::DEFAULT_MAX_PER_ENTRY_SIZE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Pack archive entry exceeds the per-entry size limit",
                ));
            }

            let path = entry.path()?.into_owned();
            if path.is_absolute()
                || path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                || !matches!(path.components().next(), Some(Component::Normal(part)) if part == pack_ref)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Pack archive entry '{}' is not confined to the '{}' directory",
                        path.display(),
                        pack_ref
                    ),
                ));
            }

            let target = staging_dir.join(&path);
            match entry_type {
                EntryType::Directory => fs::create_dir_all(&target)?,
                EntryType::Regular => {
                    #[cfg(unix)]
                    let executable = entry.header().mode()? & 0o111 != 0;
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&target)?;
                    let mut writer = io::BufWriter::new(file);
                    let mut limited = (&mut entry).take(
                        crate::config::PackUploadConfig::DEFAULT_MAX_PER_ENTRY_SIZE_BYTES + 1,
                    );
                    let written = io::copy(&mut limited, &mut writer)?;
                    writer.flush()?;
                    if written > crate::config::PackUploadConfig::DEFAULT_MAX_PER_ENTRY_SIZE_BYTES {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Pack archive entry exceeds the per-entry size limit",
                        ));
                    }
                    total_bytes = total_bytes.checked_add(written).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "Pack archive size overflow")
                    })?;
                    if total_bytes
                        > crate::config::PackUploadConfig::DEFAULT_MAX_EXTRACTED_SIZE_BYTES
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Pack archive exceeds the total extracted size limit",
                        ));
                    }
                    #[cfg(unix)]
                    if executable {
                        use std::os::unix::fs::PermissionsExt;

                        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
                    }
                }
                _ => unreachable!("entry type validated above"),
            }
        }

        let staged_pack = staging_dir.join(pack_ref);
        if !staged_pack.join("pack.yaml").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Pack archive does not contain pack.yaml at its pack root",
            ));
        }

        let final_pack = packs_dir.join(pack_ref);
        let backup = packs_dir.join(format!(
            ".attune-pack-backup-{}-{}",
            pack_ref,
            uuid::Uuid::new_v4()
        ));
        activate_staged_pack(&staged_pack, &final_pack, &backup, remove_path)
    })();

    let _ = fs::remove_dir_all(&staging_dir);
    extraction_result
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn activate_staged_pack(
    staged_pack: &Path,
    final_pack: &Path,
    backup: &Path,
    cleanup_backup: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let had_existing = std::fs::symlink_metadata(final_pack).is_ok();
    if had_existing {
        std::fs::rename(final_pack, backup)?;
    }
    if let Err(error) = std::fs::rename(staged_pack, final_pack) {
        if had_existing {
            if let Err(restore_error) = std::fs::rename(backup, final_pack) {
                return Err(std::io::Error::new(
                    restore_error.kind(),
                    format!(
                        "failed to activate staged pack ({error}) and restore previous pack ({restore_error})"
                    ),
                ));
            }
        }
        return Err(error);
    }

    if had_existing {
        if let Err(error) = cleanup_backup(backup) {
            tracing::warn!(
                backup = %backup.display(),
                %error,
                "Pack activated successfully but its backup could not be removed"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn spawn_archive_server(archive: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.contains("GET /api/v1/internal/pack-releases/42/archive"));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer token"));
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                archive.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&archive).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        (format!("http://{address}"), handle)
    }

    #[tokio::test]
    async fn test_api_transport_is_pack_local() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        )
        .unwrap();

        let digest = "a".repeat(64);
        assert!(!transport.is_release_local("mypack", &digest).await);

        let pack = release_cache_path(tmp.path(), "mypack", &digest).unwrap();
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("pack.yaml"), "ref: mypack").unwrap();
        std::fs::write(pack.parent().unwrap().join(RELEASE_READY_MARKER), &digest).unwrap();
        assert!(transport.is_release_local("mypack", &digest).await);
    }

    #[tokio::test]
    async fn test_api_transport_remove_pack() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        )
        .unwrap();

        // Create a pack dir with a file
        let pack_dir = tmp.path().join("mypack");
        std::fs::create_dir(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("pack.yaml"), "ref: mypack").unwrap();

        assert!(tmp.path().join("mypack").is_dir());
        transport.remove_pack("mypack").await.unwrap();
        assert!(!tmp.path().join("mypack").exists());
    }

    #[tokio::test]
    async fn test_api_transport_removes_selected_release_caches() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        )
        .unwrap();
        let removed_digest = "a".repeat(64);
        let retained_digest = "b".repeat(64);
        let removed = release_cache_path(tmp.path(), "mypack", &removed_digest).unwrap();
        let retained = release_cache_path(tmp.path(), "other", &retained_digest).unwrap();
        std::fs::create_dir_all(&removed).unwrap();
        std::fs::create_dir_all(&retained).unwrap();

        transport
            .remove_pack_releases("mypack", std::slice::from_ref(&removed_digest))
            .await
            .unwrap();

        assert!(!removed.exists());
        assert!(retained.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_api_transport_rejects_symlinked_release_cache_ancestors() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let digest = "a".repeat(64);
        let outside_release = outside.path().join("sha256").join(&digest).join("pack");
        std::fs::create_dir_all(&outside_release).unwrap();
        std::fs::create_dir_all(tmp.path().join(".releases")).unwrap();
        symlink(
            outside.path().join("sha256"),
            tmp.path().join(".releases").join("sha256"),
        )
        .unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        )
        .unwrap();

        let error = transport
            .remove_pack_releases("mypack", std::slice::from_ref(&digest))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("must be a real directory"));
        assert!(outside_release.exists());
    }

    #[tokio::test]
    async fn test_api_transport_remove_nonexistent_pack() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        )
        .unwrap();

        // Should not error
        transport.remove_pack("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_api_transport_rejects_invalid_pack_refs() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        )
        .unwrap();

        assert!(transport.remove_pack("../escape").await.is_err());
        assert!(
            !transport
                .is_release_local("../escape", &"a".repeat(64))
                .await
        );
    }

    fn archive_with_entry_mode(
        path: &str,
        contents: &[u8],
        entry_type: tar::EntryType,
        mode: u32,
    ) -> Vec<u8> {
        use flate2::{write::GzEncoder, Compression};

        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(entry_type);
        header.set_mode(mode);
        header.set_size(contents.len() as u64);
        header.set_cksum();
        archive.append_data(&mut header, path, contents).unwrap();
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    fn archive_with_entry(path: &str, contents: &[u8], entry_type: tar::EntryType) -> Vec<u8> {
        archive_with_entry_mode(path, contents, entry_type, 0o644)
    }

    #[test]
    fn extraction_rejects_links_without_replacing_existing_pack() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("demo");
        std::fs::create_dir(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("pack.yaml"), "old").unwrap();
        let bytes = archive_with_entry("demo/link", b"", tar::EntryType::Symlink);

        assert!(extract_pack_archive(bytes.as_slice(), tmp.path(), "demo").is_err());
        assert_eq!(
            std::fs::read_to_string(pack_dir.join("pack.yaml")).unwrap(),
            "old"
        );
    }

    #[test]
    fn extraction_stages_and_replaces_pack() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("demo");
        std::fs::create_dir(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("pack.yaml"), "old").unwrap();
        let bytes = archive_with_entry("demo/pack.yaml", b"ref: demo\n", tar::EntryType::Regular);

        extract_pack_archive(bytes.as_slice(), tmp.path(), "demo").unwrap();
        assert_eq!(
            std::fs::read_to_string(pack_dir.join("pack.yaml")).unwrap(),
            "ref: demo\n"
        );
        assert!(std::fs::read_dir(tmp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".attune-pack-")));
    }

    #[cfg(unix)]
    #[test]
    fn extraction_preserves_executable_intent() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        use flate2::{write::GzEncoder, Compression};
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (path, contents, mode) in [
            ("demo/pack.yaml", b"ref: demo\n".as_slice(), 0o644),
            ("demo/run", b"#!/bin/sh\n".as_slice(), 0o755),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(mode);
            header.set_size(contents.len() as u64);
            header.set_cksum();
            archive.append_data(&mut header, path, contents).unwrap();
        }
        let encoder = archive.into_inner().unwrap();
        let bytes = encoder.finish().unwrap();

        extract_pack_archive(bytes.as_slice(), tmp.path(), "demo").unwrap();

        let mode = std::fs::metadata(tmp.path().join("demo/run"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
        let mode = std::fs::metadata(tmp.path().join("demo/pack.yaml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0);
    }

    #[test]
    fn candidate_extraction_does_not_replace_active_pack() {
        let tmp = TempDir::new().unwrap();
        let active = tmp.path().join("demo");
        std::fs::create_dir(&active).unwrap();
        std::fs::write(active.join("pack.yaml"), "old").unwrap();
        let attempt = tmp.path().join(".pack-test-attempts").join("42");
        let bytes = archive_with_entry("demo/pack.yaml", b"ref: demo\n", tar::EntryType::Regular);

        extract_pack_archive(bytes.as_slice(), &attempt, "demo").unwrap();

        assert_eq!(
            std::fs::read_to_string(active.join("pack.yaml")).unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(attempt.join("demo/pack.yaml")).unwrap(),
            "ref: demo\n"
        );
    }

    #[test]
    fn activation_succeeds_when_backup_cleanup_fails() {
        let tmp = TempDir::new().unwrap();
        let staged = tmp.path().join("staged");
        let active = tmp.path().join("demo");
        let backup = tmp.path().join("backup");
        std::fs::create_dir(&staged).unwrap();
        std::fs::write(staged.join("pack.yaml"), "new").unwrap();
        std::fs::create_dir(&active).unwrap();
        std::fs::write(active.join("pack.yaml"), "old").unwrap();

        activate_staged_pack(&staged, &active, &backup, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected cleanup failure",
            ))
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(active.join("pack.yaml")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(backup.join("pack.yaml")).unwrap(),
            "old"
        );
    }

    #[test]
    fn activation_failure_restores_previous_pack() {
        let tmp = TempDir::new().unwrap();
        let missing_staged = tmp.path().join("missing-staged");
        let active = tmp.path().join("demo");
        let backup = tmp.path().join("backup");
        std::fs::create_dir(&active).unwrap();
        std::fs::write(active.join("pack.yaml"), "old").unwrap();

        assert!(activate_staged_pack(&missing_staged, &active, &backup, remove_path).is_err());
        assert_eq!(
            std::fs::read_to_string(active.join("pack.yaml")).unwrap(),
            "old"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn digest_mismatch_is_rejected_before_extraction() {
        let bytes = archive_with_entry("demo/pack.yaml", b"ref: demo\n", tar::EntryType::Regular);
        let error =
            verify_archive_digest(Sha256::digest(&bytes).into(), 42, &"0".repeat(64)).unwrap_err();
        assert!(error.to_string().contains("digest mismatch"));
    }

    #[test]
    fn interrupted_extraction_never_publishes_a_release() {
        let tmp = TempDir::new().unwrap();
        let digest = "a".repeat(64);
        let archive = tmp.path().join("truncated.tar.gz");
        std::fs::write(&archive, b"truncated").unwrap();
        assert!(extract_release_archive(&archive, tmp.path(), "demo", &digest).is_err());
        assert!(!release_cache_path(tmp.path(), "demo", &digest)
            .unwrap()
            .exists());
    }

    #[test]
    fn concurrent_release_materialization_publishes_one_complete_tree() {
        let tmp = TempDir::new().unwrap();
        let bytes = archive_with_entry("demo/pack.yaml", b"ref: demo\n", tar::EntryType::Regular);
        let archive = Arc::new(tmp.path().join("release.tar.gz"));
        std::fs::write(archive.as_ref(), bytes).unwrap();
        let digest = "b".repeat(64);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let root = tmp.path().to_path_buf();
                let archive = archive.clone();
                let digest = digest.clone();
                std::thread::spawn(move || {
                    extract_release_archive(&archive, &root, "demo", &digest)
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let pack = release_cache_path(tmp.path(), "demo", &digest).unwrap();
        assert_eq!(
            std::fs::read_to_string(pack.join("pack.yaml")).unwrap(),
            "ref: demo\n"
        );
        let cache_root = tmp.path().join(".releases").join("sha256");
        assert!(std::fs::read_dir(cache_root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.')));
    }

    #[tokio::test]
    async fn api_materialization_is_identical_across_release_byte_backends() {
        let archive = archive_with_entry(
            "demo/pack.yaml",
            b"ref: demo\nversion: 1.0.0\n",
            tar::EntryType::Regular,
        );
        let digest = Sha256::digest(&archive)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let first_cache = TempDir::new().unwrap();
        let second_cache = TempDir::new().unwrap();
        let (first_url, first_server) = spawn_archive_server(archive.clone()).await;
        let (second_url, second_server) = spawn_archive_server(archive).await;

        let first =
            ApiPackTransport::new(&first_url, "token", first_cache.path().to_str().unwrap())
                .unwrap()
                .sync_release("demo", 42, &digest)
                .await
                .unwrap();
        let second =
            ApiPackTransport::new(&second_url, "token", second_cache.path().to_str().unwrap())
                .unwrap()
                .sync_release("demo", 42, &digest)
                .await
                .unwrap();

        first_server.await.unwrap();
        second_server.await.unwrap();
        assert_eq!(
            std::fs::read(first.join("pack.yaml")).unwrap(),
            std::fs::read(second.join("pack.yaml")).unwrap()
        );
        assert!(first.ends_with(format!(".releases/sha256/{digest}/pack")));
        assert!(second.ends_with(format!(".releases/sha256/{digest}/pack")));
    }
}
