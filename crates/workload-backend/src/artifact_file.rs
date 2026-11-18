//! Selected artifact bytes only. The concrete adapter proves exact payload
//! termination before calling this; publication and cleanup remain caller-owned.
use crate::cache::{CacheError, Result};
use attune_common::{artifact_transport::ValidatedRelativePath, workload_artifact::ArtifactLimits};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
pub(crate) fn permits_files(
    spec: &attune_common::isolated_process::ProcessSpec,
) -> attune_common::Result<()> {
    use attune_common::workload_launch::LaunchPurpose;
    if !matches!(
        spec.purpose,
        LaunchPurpose::Action | LaunchPurpose::Sensor | LaunchPurpose::CandidateTest
    ) {
        return Err(crate::process_policy::invalid());
    }
    Ok(())
}

pub(crate) fn validate_tree(root: &Path, limits: ArtifactLimits) -> Result<()> {
    let metadata = fs::symlink_metadata(root).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            CacheError::LostArtifactStaging
        } else {
            CacheError::Io(e)
        }
    })?;
    if !metadata.is_dir() {
        return Err(CacheError::UnsafeEntry);
    }
    let mut directories = vec![root.to_path_buf()];
    let (mut entries, mut bytes) = (0u64, 0u64);
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            entries = entries.checked_add(1).ok_or(CacheError::Quota)?;
            if entries > limits.max_entries {
                return Err(CacheError::Quota);
            }
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() && metadata.nlink() == 1 {
                bytes = bytes.checked_add(metadata.len()).ok_or(CacheError::Quota)?;
                if bytes > limits.max_total_bytes {
                    return Err(CacheError::Quota);
                }
            } else {
                return Err(CacheError::UnsafeEntry);
            }
        }
    }
    Ok(())
}

pub(crate) fn read_selected(
    root: &Path,
    relative: &ValidatedRelativePath,
    remaining: u64,
    limits: ArtifactLimits,
) -> Result<Option<File>> {
    // In particular, a missing root or an unsafe unrelated tree member is NOT
    // evidence that an allocated selected file is absent.
    validate_tree(root, limits)?;
    let mut path = root.to_path_buf();
    let components = relative.as_path().components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if index + 1 < components.len() {
            if !metadata.is_dir() {
                return Err(CacheError::UnsafeEntry);
            }
        } else {
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(CacheError::UnsafeEntry);
            }
            if metadata.len() > remaining {
                return Err(CacheError::Quota);
            }
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)?;
            let actual = file.metadata()?;
            if !actual.is_file()
                || actual.nlink() != 1
                || actual.dev() != metadata.dev()
                || actual.ino() != metadata.ino()
                || actual.len() != metadata.len()
            {
                return Err(CacheError::UnsafeEntry);
            }
            file.rewind()?;
            return Ok(Some(file));
        }
    }
    Err(CacheError::UnsafeEntry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    fn limits() -> ArtifactLimits {
        ArtifactLimits {
            max_total_bytes: 16,
            max_entries: 4,
        }
    }
    #[test]
    fn selected_missing_and_zero_bytes_differ_from_lost_staging_and_invalid_tree() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("artifacts");
        let empty = ValidatedRelativePath::new("empty").unwrap();
        assert!(matches!(
            read_selected(&directory, &empty, 0, limits()),
            Err(CacheError::LostArtifactStaging)
        ));
        fs::create_dir(&directory).unwrap();
        assert!(read_selected(&directory, &empty, 0, limits())
            .unwrap()
            .is_none());
        fs::write(directory.join("empty"), b"").unwrap();
        let mut file = read_selected(&directory, &empty, 0, limits())
            .unwrap()
            .unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert!(bytes.is_empty());
        fs::write(directory.join("content"), b"owned").unwrap();
        let content = ValidatedRelativePath::new("content").unwrap();
        assert!(matches!(
            read_selected(&directory, &content, 4, limits()),
            Err(CacheError::Quota)
        ));
        let mut file = read_selected(&directory, &content, 5, limits())
            .unwrap()
            .unwrap();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"owned");
        std::os::unix::fs::symlink("/etc/passwd", directory.join("unselected-escape")).unwrap();
        assert!(matches!(
            read_selected(
                &directory,
                &ValidatedRelativePath::new("missing").unwrap(),
                16,
                limits()
            ),
            Err(CacheError::UnsafeEntry)
        ));
    }
    #[test]
    fn retained_tree_counts_directories_and_rejects_hardlinks_special_files_and_overflow() {
        for case in 0..4 {
            let root = tempfile::tempdir().unwrap();
            match case {
                0 => {
                    fs::write(root.path().join("first"), b"owned").unwrap();
                    fs::hard_link(root.path().join("first"), root.path().join("alias")).unwrap();
                }
                1 => {
                    let _socket =
                        std::os::unix::net::UnixListener::bind(root.path().join("socket")).unwrap();
                }
                2 => fs::create_dir_all(root.path().join("a/b/c/d/e")).unwrap(),
                3 => fs::write(root.path().join("too-large"), [0; 17]).unwrap(),
                _ => unreachable!(),
            }
            let path = ValidatedRelativePath::new("not-found").unwrap();
            assert!(
                read_selected(root.path(), &path, 16, limits()).is_err(),
                "case {case}"
            );
        }
    }
}
