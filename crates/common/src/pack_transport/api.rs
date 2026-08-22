//! API-based pack transport.
//!
//! Downloads pack archives from the API and extracts them to the local
//! `packs_base_dir`. Used by remote workers/sensors without a shared volume.

use async_trait::async_trait;
use reqwest::Client;
use std::path::{Component, Path};
use std::sync::Arc;
use tracing::{debug, info};

use super::PackFileTransport;
use crate::auth::WorkerTokenProvider;
use crate::error::{Error, Result};
use crate::schema::RefValidator;

const MAX_ARCHIVE_BYTES: u64 = crate::config::PackUploadConfig::DEFAULT_MAX_EXTRACTED_SIZE_BYTES;

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

    fn candidate_archive_url(&self, pack_install_id: i64) -> String {
        format!(
            "{}/api/v1/internal/pack-installs/{}/archive",
            self.api_url, pack_install_id
        )
    }

    async fn download_archive(&self, url: &str, subject: &str) -> Result<Vec<u8>> {
        let token = self.auth_token_source.token()?;
        let mut response = self
            .client
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Failed to download {subject}: {e}")))?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && self.auth_token_source.can_force_refresh()
        {
            let refreshed_token = self.auth_token_source.force_refresh()?;
            response = self
                .client
                .get(url)
                .bearer_auth(&refreshed_token)
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

        let mut archive_bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read {subject}: {e}")))?
        {
            let next_size = archive_bytes
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| Error::validation("Pack archive download size overflow"))?;
            if next_size as u64 > MAX_ARCHIVE_BYTES {
                return Err(Error::validation(format!(
                    "{subject} exceeds the {} byte download limit",
                    MAX_ARCHIVE_BYTES
                )));
            }
            archive_bytes.extend_from_slice(&chunk);
        }
        Ok(archive_bytes)
    }
}

#[async_trait]
impl PackFileTransport for ApiPackTransport {
    async fn sync_pack(&self, pack_ref: &str) -> Result<()> {
        RefValidator::validate_pack_ref(pack_ref)?;
        let url = self.archive_url(pack_ref);
        info!(
            "Downloading pack '{}' from {} to {}",
            pack_ref, url, self.packs_base_dir
        );

        let archive_bytes = self
            .download_archive(&url, &format!("pack archive for '{pack_ref}'"))
            .await?;

        debug!(
            "Downloaded {} bytes for pack '{}', extracting...",
            archive_bytes.len(),
            pack_ref
        );

        let packs_dir = self.packs_base_dir.clone();
        let pack_ref_owned = pack_ref.to_string();
        let extraction_pack_ref = pack_ref_owned.clone();
        tokio::task::spawn_blocking(move || {
            extract_pack_archive(&archive_bytes, Path::new(&packs_dir), &extraction_pack_ref)
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

    async fn sync_pack_test_candidate(
        &self,
        pack_ref: &str,
        pack_install_id: i64,
    ) -> Result<std::path::PathBuf> {
        RefValidator::validate_pack_ref(pack_ref)?;
        if pack_install_id <= 0 {
            return Err(Error::validation("Pack install ID must be positive"));
        }
        let archive_bytes = self
            .download_archive(
                &self.candidate_archive_url(pack_install_id),
                &format!("candidate archive for pack install {pack_install_id}"),
            )
            .await?;
        let attempt_dir = Path::new(&self.packs_base_dir)
            .join(".pack-test-attempts")
            .join(pack_install_id.to_string());
        let pack_dir = attempt_dir.join(pack_ref);
        let packs_dir = self.packs_base_dir.clone();
        let pack_ref = pack_ref.to_string();
        tokio::task::spawn_blocking(move || {
            let _ = std::fs::remove_dir_all(&attempt_dir);
            extract_pack_archive(&archive_bytes, &attempt_dir, &pack_ref)
        })
        .await
        .map_err(|e| Error::Internal(format!("Candidate pack extraction task panicked: {e}")))?
        .map_err(|e| Error::Internal(format!("Failed to extract candidate pack: {e}")))?;
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

    async fn is_pack_local(&self, pack_ref: &str) -> bool {
        if let Err(error) = RefValidator::validate_pack_ref(pack_ref) {
            tracing::warn!(pack_ref, %error, "Rejected invalid pack ref at pack transport boundary");
            return false;
        }
        let pack_dir = std::path::Path::new(&self.packs_base_dir).join(pack_ref);
        pack_dir.is_dir()
    }

    fn transport_mode(&self) -> &'static str {
        "api"
    }
}

fn extract_pack_archive(
    archive_bytes: &[u8],
    packs_dir: &Path,
    pack_ref: &str,
) -> std::io::Result<()> {
    use flate2::read::GzDecoder;
    use std::fs;
    use std::io::{self, Cursor, Read, Write};
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
        let decoder = GzDecoder::new(Cursor::new(archive_bytes));
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

    #[tokio::test]
    async fn test_api_transport_rejects_invalid_pack_refs() {
        let tmp = TempDir::new().unwrap();
        let transport = ApiPackTransport::new(
            "http://localhost:8080",
            "token",
            tmp.path().to_str().unwrap(),
        );

        assert!(transport.remove_pack("../escape").await.is_err());
        assert!(!transport.is_pack_local("../escape").await);
    }

    fn archive_with_entry(path: &str, contents: &[u8], entry_type: tar::EntryType) -> Vec<u8> {
        use flate2::{write::GzEncoder, Compression};

        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(entry_type);
        header.set_mode(0o644);
        header.set_size(contents.len() as u64);
        header.set_cksum();
        archive.append_data(&mut header, path, contents).unwrap();
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn extraction_rejects_links_without_replacing_existing_pack() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("demo");
        std::fs::create_dir(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("pack.yaml"), "old").unwrap();
        let bytes = archive_with_entry("demo/link", b"", tar::EntryType::Symlink);

        assert!(extract_pack_archive(&bytes, tmp.path(), "demo").is_err());
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

        extract_pack_archive(&bytes, tmp.path(), "demo").unwrap();
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

    #[test]
    fn candidate_extraction_does_not_replace_active_pack() {
        let tmp = TempDir::new().unwrap();
        let active = tmp.path().join("demo");
        std::fs::create_dir(&active).unwrap();
        std::fs::write(active.join("pack.yaml"), "old").unwrap();
        let attempt = tmp.path().join(".pack-test-attempts").join("42");
        let bytes = archive_with_entry("demo/pack.yaml", b"ref: demo\n", tar::EntryType::Regular);

        extract_pack_archive(&bytes, &attempt, "demo").unwrap();

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
}
