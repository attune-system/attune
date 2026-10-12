//! Volume-based pack transport (no-op — files already on shared volume).

use async_trait::async_trait;
use std::path::PathBuf;

use super::{release_cache_path, ApiPackTransport, PackFileTransport};
use crate::error::Result;
use crate::schema::RefValidator;
use sha2::{Digest, Sha256};

/// No-op transport for when packs are on a shared volume.
#[derive(Debug)]
pub struct VolumePackTransport {
    packs_base_dir: String,
    candidate_transport: Option<ApiPackTransport>,
}

impl VolumePackTransport {
    pub fn new(packs_base_dir: &str) -> Self {
        Self {
            packs_base_dir: packs_base_dir.to_string(),
            candidate_transport: None,
        }
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
        if release_id <= 0 || !release_archive_matches(&path, release_digest) {
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
        Ok(())
    }

    async fn is_release_local(&self, pack_ref: &str, release_digest: &str) -> bool {
        release_cache_path(&self.packs_base_dir, pack_ref, release_digest)
            .is_ok_and(|path| release_archive_matches(&path, release_digest))
    }

    fn transport_mode(&self) -> &'static str {
        "volume"
    }
}

fn release_archive_matches(pack_path: &std::path::Path, expected_digest: &str) -> bool {
    if !pack_path.join("pack.yaml").is_file() {
        return false;
    }
    std::fs::read(pack_path.parent().unwrap_or(pack_path).join("pack.tar.gz"))
        .ok()
        .map(|bytes| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                == expected_digest
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
