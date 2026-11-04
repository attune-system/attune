//! Shared-volume transport with per-instance immutable archive verification.

use async_trait::async_trait;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

const MAX_VERIFIED_RELEASES: usize = 256;
type VerificationEntry = Arc<Mutex<Option<ArchiveIdentity>>>;

use super::{release_cache_path, ApiPackTransport, PackFileTransport};
use crate::error::Result;
use crate::schema::RefValidator;
use sha2::{Digest, Sha256};

/// No-op transport for when packs are on a shared volume.
#[derive(Debug)]
pub struct VolumePackTransport {
    packs_base_dir: String,
    candidate_transport: Option<ApiPackTransport>,
    verified: Mutex<HashMap<PathBuf, VerificationEntry>>,
}

impl VolumePackTransport {
    pub fn new(packs_base_dir: &str) -> Self {
        Self {
            packs_base_dir: packs_base_dir.to_string(),
            candidate_transport: None,
            verified: Mutex::new(HashMap::new()),
        }
    }

    async fn verify_release(&self, path: PathBuf, digest: &str) -> Result<bool> {
        let entry = {
            let mut entries = self.verified.lock().await;
            if !entries.contains_key(&path) && entries.len() >= MAX_VERIFIED_RELEASES {
                entries.retain(|_, entry| Arc::strong_count(entry) > 1);
            }
            if entries.len() >= MAX_VERIFIED_RELEASES && !entries.contains_key(&path) {
                Arc::new(Mutex::new(None))
            } else {
                entries.entry(path.clone()).or_default().clone()
            }
        };
        // Single-flight per release, without serializing different archives.
        let mut retained = entry.lock().await;
        let previous = retained.clone();
        let digest = digest.to_owned();
        let (valid, identity, cache_hit) = tokio::task::spawn_blocking(move || {
            release_archive_matches(&path, &digest, previous.as_ref())
        })
        .await
        .map_err(|error| {
            crate::Error::Internal(format!("Release verification task failed: {error}"))
        })?;
        *retained = identity;
        tracing::debug!(cache_hit, valid, "Shared release archive verification");
        Ok(valid)
    }

    /// Installed releases may share a volume; candidates always come from the
    /// API's version-pinned object backend, never an API-local staging path.
    pub fn with_candidate_transport(mut self, transport: ApiPackTransport) -> Self {
        self.candidate_transport = Some(transport);
        self
    }
}

#[async_trait]
impl PackFileTransport for VolumePackTransport {
    async fn sync_release(
        &self,
        pack_ref: &str,
        release_id: i64,
        release_digest: &str,
    ) -> Result<PathBuf> {
        let path = release_cache_path(&self.packs_base_dir, pack_ref, release_digest)?;
        if release_id <= 0 || !self.verify_release(path.clone(), release_digest).await? {
            return Err(crate::Error::validation(format!(
                "Pack release {release_id} ({release_digest}) for '{pack_ref}' is not local on the shared volume"
            )));
        }
        Ok(path)
    }

    async fn sync_pack_test_candidate(
        &self,
        pack_ref: &str,
        pack_install_id: i64,
        candidate_access_token: Option<&str>,
    ) -> Result<PathBuf> {
        let transport = self.candidate_transport.as_ref().ok_or_else(|| {
            crate::Error::configuration(
                "Pack candidates require ATTUNE_API_URL and a worker token even with volume transport",
            )
        })?;
        transport
            .sync_pack_test_candidate(pack_ref, pack_install_id, candidate_access_token)
            .await
    }

    async fn remove_pack(&self, pack_ref: &str) -> Result<()> {
        RefValidator::validate_pack_ref(pack_ref)?;
        self.verified.lock().await.clear();
        tracing::debug!(
            "VolumePackTransport: remove_pack('{}') — no-op (shared volume)",
            pack_ref
        );
        Ok(())
    }

    async fn remove_pack_releases(
        &self,
        pack_ref: &str,
        _release_digests: &[String],
    ) -> Result<()> {
        RefValidator::validate_pack_ref(pack_ref)?;
        self.verified.lock().await.clear();
        Ok(())
    }

    async fn is_release_local(&self, pack_ref: &str, release_digest: &str) -> bool {
        let Ok(path) = release_cache_path(&self.packs_base_dir, pack_ref, release_digest) else {
            return false;
        };
        self.verify_release(path, release_digest)
            .await
            .unwrap_or(false)
    }

    fn transport_mode(&self) -> &'static str {
        "volume"
    }
}

/// Unix ctime detects writes even when mtime and size are restored. File identity
/// also detects replacement. Platforms without this evidence rehash every time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveIdentity {
    digest: String,
    #[cfg(unix)]
    stat: (u64, u64, u64, i64, i64, i64, i64),
}

fn archive_identity(metadata: &std::fs::Metadata, digest: &str) -> Option<ArchiveIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(ArchiveIdentity {
            digest: digest.to_owned(),
            stat: (
                metadata.dev(),
                metadata.ino(),
                metadata.len(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, digest);
        None
    }
}

fn release_archive_matches(
    pack_path: &Path,
    expected_digest: &str,
    previous: Option<&ArchiveIdentity>,
) -> (bool, Option<ArchiveIdentity>, bool) {
    use std::io::Read;
    let verify = || -> std::io::Result<(bool, Option<ArchiveIdentity>, bool)> {
        if !pack_path.join("pack.yaml").is_file() {
            return Ok((false, None, false));
        }
        let archive_path = pack_path.parent().unwrap_or(pack_path).join("pack.tar.gz");
        let mut archive = std::fs::File::open(&archive_path)?;
        let before = archive.metadata()?;
        if !before.is_file() {
            return Ok((false, None, false));
        }
        let identity = archive_identity(&before, expected_digest);
        if identity.is_some() && identity.as_ref() == previous {
            return Ok((true, identity, true));
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = archive.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        // A concurrent replacement/write cannot populate a reusable proof.
        let stable = archive_identity(&archive.metadata()?, expected_digest) == identity
            && archive_identity(&std::fs::metadata(&archive_path)?, expected_digest) == identity;
        if actual != expected_digest || !stable {
            return Ok((false, None, false));
        }
        Ok((true, identity, false))
    };
    verify().unwrap_or((false, None, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn warm_archive_proof_reuses_only_unchanged_file_identity() {
        let tmp = TempDir::new().unwrap();
        let bytes = b"original archive";
        let digest = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = release_cache_path(tmp.path(), "demo", &digest).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("pack.yaml"), "ref: demo").unwrap();
        let archive = path.parent().unwrap().join("pack.tar.gz");
        std::fs::write(&archive, bytes).unwrap();
        let (valid, identity, hit) = release_archive_matches(&path, &digest, None);
        assert!(valid && !hit);
        let (valid, _, hit) = release_archive_matches(&path, &digest, identity.as_ref());
        assert!(valid);
        assert_eq!(hit, cfg!(unix));
        let metadata = std::fs::metadata(&archive).unwrap();
        std::fs::write(&archive, b"tampered archive").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&archive)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        assert!(
            !release_archive_matches(&path, &digest, identity.as_ref()).0,
            "same-size writes with restored mtime must invalidate verification"
        );
        std::fs::write(&archive, bytes).unwrap();
        assert!(release_archive_matches(&path, &digest, identity.as_ref()).0);
        std::fs::remove_file(path.join("pack.yaml")).unwrap();
        assert!(!release_archive_matches(&path, &digest, identity.as_ref()).0);
    }

    #[tokio::test]
    async fn release_verification_handles_concurrency_replacement_and_eviction() {
        let tmp = TempDir::new().unwrap();
        let digest = Sha256::digest(b"archive")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = release_cache_path(tmp.path(), "demo", &digest).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("pack.yaml"), "ref: demo").unwrap();
        let archive = path.parent().unwrap().join("pack.tar.gz");
        std::fs::write(&archive, b"archive").unwrap();
        let transport = VolumePackTransport::new(tmp.path().to_str().unwrap());
        let (left, right) = tokio::join!(
            transport.sync_release("demo", 1, &digest),
            transport.sync_release("demo", 1, &digest)
        );
        assert_eq!(left.unwrap(), path);
        assert_eq!(right.unwrap(), path);
        let replacement = archive.with_extension("replacement");
        std::fs::write(&replacement, b"corrupt").unwrap();
        std::fs::rename(replacement, &archive).unwrap();
        assert!(transport.sync_release("demo", 1, &digest).await.is_err());
        std::fs::write(&archive, b"archive").unwrap();
        assert!(transport.sync_release("demo", 1, &digest).await.is_ok());
        transport
            .remove_pack_releases("demo", &[digest])
            .await
            .unwrap();
        assert!(transport.verified.lock().await.is_empty());
    }

    #[tokio::test]
    async fn candidate_requires_object_download_even_when_local_staging_exists() {
        let tmp = TempDir::new().unwrap();
        let candidate = tmp.path().join(".pack-test-42");
        std::fs::create_dir(&candidate).unwrap();
        std::fs::write(candidate.join("pack.yaml"), "ref: demo").unwrap();
        let transport = VolumePackTransport::new(tmp.path().to_str().unwrap());
        assert!(transport
            .sync_pack_test_candidate("demo", 42, Some("attempt-token"))
            .await
            .unwrap_err()
            .to_string()
            .contains("ATTUNE_API_URL"));
        assert!(candidate.exists());
    }

    #[tokio::test]
    async fn test_volume_transport_is_pack_local() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumePackTransport::new(tmp.path().to_str().unwrap());

        // Pack doesn't exist
        assert!(!transport.is_release_local("mypack", &"a".repeat(64)).await);

        // Create pack dir
        let archive = b"archive";
        let digest: String = Sha256::digest(archive)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let pack = release_cache_path(tmp.path(), "mypack", &digest).unwrap();
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("pack.yaml"), "ref: mypack").unwrap();
        std::fs::write(pack.parent().unwrap().join("pack.tar.gz"), archive).unwrap();
        assert!(transport.is_release_local("mypack", &digest).await);
    }

    #[tokio::test]
    async fn test_volume_transport_sync_is_noop() {
        let tmp = TempDir::new().unwrap();
        let transport = VolumePackTransport::new(tmp.path().to_str().unwrap());
        // Should not error
        let archive = b"archive";
        let digest: String = Sha256::digest(archive)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let path = release_cache_path(tmp.path(), "anything", &digest).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("pack.yaml"), "ref: anything").unwrap();
        std::fs::write(path.parent().unwrap().join("pack.tar.gz"), archive).unwrap();
        transport
            .sync_release("anything", 1, &digest)
            .await
            .unwrap();
        transport.remove_pack("anything").await.unwrap();
        assert!(transport
            .sync_release("../escape", 1, &digest)
            .await
            .is_err());
        assert!(transport.remove_pack("../escape").await.is_err());
        assert!(!transport.is_release_local("../escape", &digest).await);
    }
}
