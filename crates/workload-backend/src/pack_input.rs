//! Convert the existing transport's retained primary release, never its
//! extracted tree. Publication and API authentication remain with their owners.
use crate::cache::CacheStore;
use attune_common::{
    models::PackRelease,
    pack_registry::storage::PackReleaseManifest,
    workload_archive::ArchivePolicy,
    workload_config::WorkloadStorageLimits,
    workload_launch::{LaunchInputKind, PinnedLaunchInput, Sha256Digest},
    workload_pack_input::{release_zip, ReleaseInputPin},
    Error, Result,
};
use std::path::Path;

/// Blocking bounded verification, identical to the native action producer's
/// primary size/hash/manifest and ownership contract. Caller owns offloading.
#[cfg(unix)]
pub fn install_release(
    cache: &CacheStore,
    release: PackRelease,
    packs_base_dir: &Path,
    limits: WorkloadStorageLimits,
    pin_id: i64,
) -> Result<PinnedLaunchInput> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let denied = || Error::PermissionDenied("Native pack input unavailable".into());
    let manifest: PackReleaseManifest =
        serde_json::from_value(release.manifest).map_err(|_| denied())?;
    if manifest.version != release.version || manifest.pack_ref != release.pack_ref || pin_id <= 0 {
        return Err(denied());
    }
    let archive = attune_common::pack_transport::release_cache_path(
        packs_base_dir,
        &release.pack_ref,
        &release.digest,
    )?
    .parent()
    .ok_or_else(denied)?
    .join("pack.tar.gz");
    let source = ReleaseInputPin {
        pack_ref: release.pack_ref,
        size_bytes: u64::try_from(release.archive_size).map_err(|_| denied())?,
        sha256: Sha256Digest::parse(&release.digest)?,
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(archive)
        .map_err(|_| denied())?;
    let metadata = file.metadata().map_err(|_| denied())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o022 != 0
        || !(metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() })
    {
        return Err(denied());
    }
    let prepared = release_zip(
        file,
        &source,
        &manifest,
        attune_common::pack_format::Limits {
            compressed_bytes: limits.compressed_bytes,
            extracted_bytes: limits.expanded_bytes,
            file_bytes: limits.expanded_bytes,
            entries: limits.entries,
            ..Default::default()
        },
        &ArchivePolicy {
            root: LaunchInputKind::Pack,
            max_compressed_bytes: limits.compressed_bytes,
            max_expanded_bytes: limits.expanded_bytes,
            max_files: limits.entries,
        },
    )?;
    let pin = PinnedLaunchInput {
        pin_id,
        kind: LaunchInputKind::Pack,
        object_id: release.id,
        object_version: release.digest,
        size_bytes: prepared.size_bytes(),
        sha256: prepared.sha256().clone(),
        max_expanded_bytes: prepared.expanded_bytes(),
        max_files: prepared.entries(),
    };
    let mut body = prepared.into_file();
    cache
        .install_input(&pin, body.as_file_mut())
        .map_err(|_| denied())?;
    Ok(pin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::pack_registry::storage::PackStorage;
    use std::os::unix::fs::PermissionsExt;
    fn cleanup(root: tempfile::TempDir) {
        fn unseal(path: &Path) {
            let meta = std::fs::symlink_metadata(path).unwrap();
            assert!(!meta.file_type().is_symlink());
            if meta.is_dir() {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
                for entry in std::fs::read_dir(path).unwrap() {
                    unseal(&entry.unwrap().path());
                }
            }
        }
        unseal(root.path());
        root.close().unwrap();
    }
    #[test]
    fn primary_release_converter_checks_bytes_metadata_ownership_and_bounds() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir_all(source.join("runtimes")).unwrap();
        std::fs::write(source.join("pack.yaml"), b"ref: core\nversion: 1.0.0\n").unwrap();
        let yaml = include_str!("../../../packs/core/runtimes/python.yaml");
        std::fs::write(source.join("runtimes/python.yaml"), yaml).unwrap();
        let metadata: serde_json::Value = serde_yaml_ng::from_str(yaml).unwrap();
        let packs = root.path().join("packs");
        let published = PackStorage::new(&packs)
            .publish_release(&source, "core", "1.0.0")
            .unwrap();
        let release = PackRelease {
            id: 1,
            pack: 1,
            pack_ref: "core".into(),
            version: "1.0.0".into(),
            digest: published.digest.clone(),
            object_key: Some("owned-primary".into()),
            provider_version: Some("owned-version".into()),
            content_path: published.pack_path.to_string_lossy().into_owned(),
            archive_size: published.archive_size as i64,
            manifest: serde_json::to_value(&published.manifest).unwrap(),
            created: chrono::Utc::now(),
            inactive_since: None,
        };
        let limits = WorkloadStorageLimits {
            compressed_bytes: 1024 * 1024,
            expanded_bytes: 1024 * 1024,
            entries: 100,
            artifacts: Default::default(),
        };
        let cache = CacheStore::open(
            root.path().join("cache"),
            crate::cache::CacheLimits {
                compressed_bytes: limits.compressed_bytes,
                expanded_bytes: limits.expanded_bytes,
                files: limits.entries,
            },
        )
        .unwrap();
        let pin = install_release(&cache, release.clone(), &packs, limits, 2).unwrap();
        crate::planning::validate_runtime_release_metadata(
            cache.open_input_archive(&pin).unwrap(),
            "core.python",
            &metadata["execution_config"],
            None,
            Some((&metadata["distributions"], None)),
        )
        .unwrap();
        assert!(crate::planning::validate_runtime_release_metadata(
            cache.open_input_archive(&pin).unwrap(),
            "core.python",
            &serde_json::json!({}),
            None,
            None
        )
        .is_err());
        let mut changed = release.clone();
        changed.archive_size += 1;
        assert!(install_release(&cache, changed, &packs, limits, 2).is_err());
        let mut wrong_manifest = release.clone();
        wrong_manifest.manifest["version"] = "changed".into();
        assert!(install_release(&cache, wrong_manifest, &packs, limits, 2).is_err());
        let mut small = limits;
        small.expanded_bytes = 1;
        assert!(install_release(&cache, release.clone(), &packs, small, 2).is_err());
        std::fs::set_permissions(
            &published.archive_path,
            std::fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        assert!(install_release(&cache, release, &packs, limits, 2).is_err());
        drop(cache);
        cleanup(root);
    }
}
