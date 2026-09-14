use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const RUNTIME_CACHE_FORMAT_VERSION: &str = "1";
pub const READY_MARKER: &str = ".attune-runtime-ready";
pub const PACK_REF_MARKER: &str = ".attune-pack-ref";

pub fn pack_runtime_environment_relative_paths(
    root: &Path,
    recorded_paths: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut relative_paths = Vec::with_capacity(recorded_paths.len());
    for path in recorded_paths {
        let relative = path.strip_prefix(root).map_err(|_| {
            Error::validation(format!(
                "Runtime environment path {} is outside the runtime environment root",
                path.display()
            ))
        })?;
        if relative.components().count() < 2
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(Error::validation(format!(
                "Recorded runtime environment path {} is too broad to purge",
                path.display()
            )));
        }
        if !relative_paths.contains(&relative.to_path_buf()) {
            relative_paths.push(relative.to_path_buf());
        }
    }
    Ok(relative_paths)
}

pub fn purge_pack_runtime_environments(
    root: &Path,
    pack_ref: &str,
    recorded_paths: &[PathBuf],
) -> Result<usize> {
    let mut validated = validated_pack_runtime_paths(root, pack_ref, recorded_paths)?;
    validated.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut removed = 0;
    for path in validated {
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| {
                Error::io(format!(
                    "Failed to remove runtime environment at {}: {error}",
                    path.display()
                ))
            })?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub struct RuntimeEnvironmentRemoval {
    staged: Vec<(PathBuf, PathBuf)>,
    committed: bool,
}

impl RuntimeEnvironmentRemoval {
    pub fn finalize_after_commit(&mut self) -> Result<usize> {
        self.committed = true;
        let mut removed = 0;
        for (_, backup) in &self.staged {
            if backup.exists() {
                fs::remove_dir_all(backup).map_err(|error| {
                    Error::io(format!(
                        "Failed to remove staged runtime environment at {}: {error}",
                        backup.display()
                    ))
                })?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl Drop for RuntimeEnvironmentRemoval {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for (destination, backup) in self.staged.iter().rev() {
            if backup.exists() && !destination.exists() {
                let _ = fs::rename(backup, destination);
            }
        }
    }
}

pub fn stage_pack_runtime_environment_removal(
    root: &Path,
    pack_ref: &str,
    recorded_paths: &[PathBuf],
) -> Result<RuntimeEnvironmentRemoval> {
    let mut paths = validated_pack_runtime_paths(root, pack_ref, recorded_paths)?;
    paths.sort_by_key(|path| path.components().count());
    let mut top_level_paths = Vec::new();
    for path in paths {
        if !top_level_paths
            .iter()
            .any(|parent: &PathBuf| path.starts_with(parent))
        {
            top_level_paths.push(path);
        }
    }

    let mut removal = RuntimeEnvironmentRemoval {
        staged: Vec::with_capacity(top_level_paths.len()),
        committed: false,
    };
    for destination in top_level_paths {
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("runtime");
        let backup =
            destination.with_file_name(format!(".{file_name}.{}.deleting", uuid::Uuid::new_v4()));
        fs::rename(&destination, &backup).map_err(|error| {
            Error::io(format!(
                "Failed to stage runtime environment removal at {}: {error}",
                destination.display()
            ))
        })?;
        removal.staged.push((destination, backup));
    }
    Ok(removal)
}

fn validated_pack_runtime_paths(
    root: &Path,
    pack_ref: &str,
    recorded_paths: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    crate::schema::RefValidator::validate_pack_ref(pack_ref)?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    if fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(Error::validation(
            "Refusing to purge a symlinked runtime environment root",
        ));
    }

    let canonical_root = root.canonicalize().map_err(|error| {
        Error::io(format!(
            "Failed to resolve runtime environment root {}: {error}",
            root.display()
        ))
    })?;
    let mut candidates = Vec::with_capacity(recorded_paths.len() + 1);
    candidates.push((root.join(pack_ref), true));
    candidates.extend(recorded_paths.iter().cloned().map(|path| (path, false)));
    let cache_root = root.join("sha256");
    if let Ok(entries) = fs::read_dir(&cache_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if fs::read_to_string(path.join(PACK_REF_MARKER))
                .is_ok_and(|owner| owner.trim() == pack_ref)
            {
                candidates.push((path, false));
            }
        }
    }

    let mut validated = Vec::new();
    for (candidate, allow_direct_child) in candidates {
        if !candidate.exists() {
            continue;
        }
        if fs::symlink_metadata(&candidate).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(Error::validation(format!(
                "Refusing to purge symlinked runtime environment path {}",
                candidate.display()
            )));
        }
        let canonical_candidate = candidate.canonicalize().map_err(|error| {
            Error::io(format!(
                "Failed to resolve runtime environment path {}: {error}",
                candidate.display()
            ))
        })?;
        if canonical_candidate == canonical_root
            || !canonical_candidate.starts_with(&canonical_root)
        {
            return Err(Error::validation(format!(
                "Runtime environment path {} is outside the runtime environment root",
                candidate.display()
            )));
        }
        let relative = canonical_candidate
            .strip_prefix(&canonical_root)
            .expect("runtime environment containment checked above");
        if !allow_direct_child && relative.components().count() < 2 {
            return Err(Error::validation(format!(
                "Recorded runtime environment path {} is too broad to purge",
                candidate.display()
            )));
        }
        if !validated.contains(&canonical_candidate) {
            validated.push(canonical_candidate);
        }
    }
    Ok(validated)
}

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

    pub fn write_pack_ref_marker(&self, path: &Path, pack_ref: &str) -> Result<()> {
        crate::schema::RefValidator::validate_pack_ref(pack_ref)?;
        std::fs::write(path.join(PACK_REF_MARKER), pack_ref).map_err(|error| {
            Error::io(format!(
                "Failed to write runtime cache pack marker at {}: {error}",
                path.display()
            ))
        })
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
    use tempfile::TempDir;

    #[test]
    fn purge_pack_runtime_environments_removes_only_owned_paths() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("runtime_envs");
        let pack_path = root.join("removed_pack").join("python");
        let content_addressed_path = root.join("sha256").join("a".repeat(64));
        let retained_path = root.join("other_pack").join("python");
        std::fs::create_dir_all(&pack_path).unwrap();
        std::fs::create_dir_all(&content_addressed_path).unwrap();
        std::fs::create_dir_all(&retained_path).unwrap();
        std::fs::write(content_addressed_path.join(PACK_REF_MARKER), "removed_pack").unwrap();

        let removed = purge_pack_runtime_environments(&root, "removed_pack", &[]).unwrap();

        assert_eq!(removed, 2);
        assert!(!root.join("removed_pack").exists());
        assert!(!content_addressed_path.exists());
        assert!(retained_path.exists());
    }

    #[test]
    fn purge_pack_runtime_environments_rejects_paths_outside_root_before_deleting() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("runtime_envs");
        let pack_path = root.join("removed_pack").join("python");
        let outside_path = temp.path().join("outside");
        std::fs::create_dir_all(&pack_path).unwrap();
        std::fs::create_dir_all(&outside_path).unwrap();

        let error = purge_pack_runtime_environments(
            &root,
            "removed_pack",
            std::slice::from_ref(&outside_path),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("outside the runtime environment root"));
        assert!(pack_path.exists());
        assert!(outside_path.exists());
    }

    #[test]
    fn purge_pack_runtime_environments_rejects_recorded_namespace_roots() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("runtime_envs");
        let pack_path = root.join("removed_pack").join("python");
        let cache_root = root.join("sha256");
        std::fs::create_dir_all(&pack_path).unwrap();
        std::fs::create_dir_all(cache_root.join("cached-environment")).unwrap();

        let error = purge_pack_runtime_environments(
            &root,
            "removed_pack",
            std::slice::from_ref(&cache_root),
        )
        .unwrap_err();

        assert!(error.to_string().contains("too broad to purge"));
        assert!(pack_path.exists());
        assert!(cache_root.exists());
    }

    #[test]
    fn pack_runtime_environment_paths_are_serialized_relative_to_the_root() {
        let root = PathBuf::from("/opt/attune/runtime_envs");
        let paths = vec![
            root.join("sha256").join("a".repeat(64)),
            root.join("example").join("python-3.12"),
        ];

        let relative = pack_runtime_environment_relative_paths(&root, &paths).unwrap();

        assert_eq!(
            relative,
            vec![
                PathBuf::from("sha256").join("a".repeat(64)),
                PathBuf::from("example").join("python-3.12"),
            ]
        );
    }

    #[test]
    fn staged_runtime_environment_removal_restores_on_drop() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("runtime_envs");
        let pack_path = root.join("removed_pack").join("python");
        std::fs::create_dir_all(&pack_path).unwrap();

        let removal = stage_pack_runtime_environment_removal(&root, "removed_pack", &[]).unwrap();
        assert!(!root.join("removed_pack").exists());

        drop(removal);

        assert!(pack_path.exists());
    }

    #[test]
    fn staged_runtime_environment_removal_deletes_backups_after_commit() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("runtime_envs");
        let pack_path = root.join("removed_pack").join("python");
        std::fs::create_dir_all(&pack_path).unwrap();

        let mut removal =
            stage_pack_runtime_environment_removal(&root, "removed_pack", &[]).unwrap();
        assert_eq!(removal.finalize_after_commit().unwrap(), 1);
        drop(removal);

        assert!(!root.join("removed_pack").exists());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    }

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
