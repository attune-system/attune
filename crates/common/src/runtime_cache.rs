use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const RUNTIME_CACHE_FORMAT_VERSION: &str = "1";
pub const READY_MARKER: &str = ".attune-runtime-ready";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCacheKey {
    digest: String,
}

impl RuntimeCacheKey {
    pub fn new(
        release_digest: &str,
        dependency_digest: &str,
        runtime_name: &str,
        runtime_version: &str,
        worker_image_format: &str,
    ) -> Result<Self> {
        validate_digest(release_digest, "release")?;
        validate_digest(dependency_digest, "dependency")?;
        let platform = format!(
            "{}:{}:{}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            target_libc()
        );
        let mut hasher = Sha256::new();
        for value in [
            RUNTIME_CACHE_FORMAT_VERSION,
            release_digest,
            dependency_digest,
            runtime_name,
            runtime_version,
            &platform,
            worker_image_format,
        ] {
            hasher.update(value.len().to_le_bytes());
            hasher.update(value.as_bytes());
        }
        Ok(Self {
            digest: hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        })
    }

    pub fn path(&self, base: &Path) -> PathBuf {
        base.join("sha256").join(&self.digest)
    }

    pub fn is_ready(&self, path: &Path) -> bool {
        std::fs::read_to_string(path.join(READY_MARKER))
            .is_ok_and(|value| value.trim() == self.digest)
    }

    pub fn write_ready_marker(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path.join(READY_MARKER), &self.digest)
    }

    pub fn temporary_sibling(&self, destination: &Path) -> Result<PathBuf> {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::validation("Runtime cache destination has no parent"))?;
        Ok(parent.join(format!(".{}.{}.tmp", self.digest, uuid::Uuid::new_v4())))
    }

    pub fn publish(&self, temporary: &Path, destination: &Path) -> std::io::Result<()> {
        if !self.is_ready(temporary) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "runtime cache candidate is not validated",
            ));
        }
        match std::fs::rename(temporary, destination) {
            Ok(()) => Ok(()),
            Err(_) if self.is_ready(destination) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_digest(value: &str, label: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(Error::validation(format!(
            "Runtime cache {label} digest must be lowercase SHA-256"
        )))
    }
}

fn target_libc() -> &'static str {
    if cfg!(target_env = "musl") {
        "musl"
    } else if cfg!(target_env = "gnu") {
        "gnu"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_changes_for_every_correctness_input() {
        let digest = "a".repeat(64);
        let dependency = "b".repeat(64);
        let base =
            RuntimeCacheKey::new(&digest, &dependency, "python", "3.12", "worker-v1").unwrap();
        for changed in [
            RuntimeCacheKey::new(&"c".repeat(64), &dependency, "python", "3.12", "worker-v1")
                .unwrap(),
            RuntimeCacheKey::new(&digest, &"c".repeat(64), "python", "3.12", "worker-v1").unwrap(),
            RuntimeCacheKey::new(&digest, &dependency, "node", "3.12", "worker-v1").unwrap(),
            RuntimeCacheKey::new(&digest, &dependency, "python", "3.13", "worker-v1").unwrap(),
            RuntimeCacheKey::new(&digest, &dependency, "python", "3.12", "worker-v2").unwrap(),
        ] {
            assert_ne!(base, changed);
        }
    }

    #[test]
    fn only_matching_ready_marker_publishes_a_cache_hit() {
        let temp = tempfile::tempdir().unwrap();
        let key = RuntimeCacheKey::new(
            &"a".repeat(64),
            &"b".repeat(64),
            "python",
            "3.12",
            "worker-v1",
        )
        .unwrap();
        assert!(!key.is_ready(temp.path()));
        key.write_ready_marker(temp.path()).unwrap();
        assert!(key.is_ready(temp.path()));
    }

    #[test]
    fn empty_dir_and_pod_rwo_roots_share_atomic_publication_rules() {
        for volume_kind in ["empty-dir", "pod-rwo"] {
            let root = tempfile::tempdir().unwrap();
            let base = root.path().join(volume_kind);
            let key = RuntimeCacheKey::new(
                &"a".repeat(64),
                &"b".repeat(64),
                "python",
                "3.12",
                "worker-v1",
            )
            .unwrap();
            let destination = key.path(&base);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();

            let interrupted = key.temporary_sibling(&destination).unwrap();
            std::fs::create_dir(&interrupted).unwrap();
            std::fs::write(interrupted.join("partial"), b"partial").unwrap();
            assert!(key.publish(&interrupted, &destination).is_err());
            assert!(!destination.exists());

            let temporary = key.temporary_sibling(&destination).unwrap();
            std::fs::create_dir(&temporary).unwrap();
            std::fs::write(temporary.join("complete"), b"complete").unwrap();
            key.write_ready_marker(&temporary).unwrap();
            key.publish(&temporary, &destination).unwrap();
            assert_eq!(
                std::fs::read(destination.join("complete")).unwrap(),
                b"complete"
            );
        }
    }
}
