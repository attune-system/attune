//! Pack Storage Management
//!
//! This module provides utilities for managing pack storage, including:
//! - Checksum calculation (SHA256)
//! - Pack directory management
//! - Storage path resolution
//! - Pack content verification

use crate::error::{Error, Result};
use crate::schema::RefValidator;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

/// Pack storage manager
pub struct PackStorage {
    base_dir: PathBuf,
}

/// Rollback guard for a pack directory being removed.
pub struct PackRemoval {
    destination: PathBuf,
    backup: Option<PathBuf>,
    committed: bool,
}

/// A staged replacement that can restore the previous active pack until committed.
pub struct PackReplacement {
    destination: PathBuf,
    staging: PathBuf,
    backup: Option<PathBuf>,
    activated: bool,
    committed: bool,
}

impl PackStorage {
    /// Create a new PackStorage instance
    ///
    /// # Arguments
    ///
    /// * `base_dir` - Base directory for pack storage (e.g., /opt/attune/packs)
    pub fn new<P: Into<PathBuf>>(base_dir: P) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Get the storage path for a pack
    ///
    /// # Arguments
    ///
    /// * `pack_ref` - Pack reference (e.g., "core", "my_pack")
    /// * `version` - Optional version (e.g., "1.0.0")
    ///
    /// # Returns
    ///
    /// Path where the pack should be stored
    pub fn get_pack_path(&self, pack_ref: &str, version: Option<&str>) -> Result<PathBuf> {
        validate_storage_ref(pack_ref, version)?;
        if let Some(v) = version {
            Ok(self.base_dir.join(format!("{}-{}", pack_ref, v)))
        } else {
            Ok(self.base_dir.join(pack_ref))
        }
    }

    /// Ensure the base directory exists
    pub fn ensure_base_dir(&self) -> Result<()> {
        if !self.base_dir.exists() {
            fs::create_dir_all(&self.base_dir).map_err(|e| {
                Error::io(format!(
                    "Failed to create pack storage directory {}: {}",
                    self.base_dir.display(),
                    e
                ))
            })?;
        }
        let metadata = fs::symlink_metadata(&self.base_dir).map_err(|error| {
            Error::io(format!("Failed to inspect pack storage directory: {error}"))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::validation(
                "Pack storage base must be a real directory",
            ));
        }
        Ok(())
    }

    /// Move a pack from temporary location to permanent storage
    ///
    /// # Arguments
    ///
    /// * `source` - Source directory (temporary location)
    /// * `pack_ref` - Pack reference
    /// * `version` - Optional version
    ///
    /// # Returns
    ///
    /// The final storage path
    pub fn install_pack<P: AsRef<Path>>(
        &self,
        source: P,
        pack_ref: &str,
        version: Option<&str>,
    ) -> Result<PathBuf> {
        self.ensure_base_dir()?;

        let mut replacement = self.stage_pack(source, pack_ref, version)?;
        replacement.activate()?;
        replacement.commit()
    }

    /// Copy a candidate to a private sibling directory without changing the active pack.
    pub fn stage_pack<P: AsRef<Path>>(
        &self,
        source: P,
        pack_ref: &str,
        version: Option<&str>,
    ) -> Result<PackReplacement> {
        self.ensure_base_dir()?;
        let destination = self.get_pack_path(pack_ref, version)?;
        let staging = self
            .base_dir
            .join(format!(".{}.{}.staging", pack_ref, uuid::Uuid::new_v4()));
        copy_dir_all(source.as_ref(), &staging).inspect_err(|_| {
            let _ = fs::remove_dir_all(&staging);
        })?;
        Ok(PackReplacement {
            destination,
            staging,
            backup: None,
            activated: false,
            committed: false,
        })
    }

    /// Remove a pack from storage
    ///
    /// # Arguments
    ///
    /// * `pack_ref` - Pack reference
    /// * `version` - Optional version
    pub fn uninstall_pack(&self, pack_ref: &str, version: Option<&str>) -> Result<()> {
        self.ensure_base_dir()?;
        let path = self.get_pack_path(pack_ref, version)?;

        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(Error::validation(
                "Refusing to uninstall a symlinked pack path",
            ));
        }
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|e| {
                Error::io(format!(
                    "Failed to remove pack at {}: {}",
                    path.display(),
                    e
                ))
            })?;
        }

        Ok(())
    }

    /// Move an installed pack aside so deletion can be committed or rolled back.
    pub fn stage_uninstall(&self, pack_ref: &str, version: Option<&str>) -> Result<PackRemoval> {
        self.ensure_base_dir()?;
        let destination = self.get_pack_path(pack_ref, version)?;
        if fs::symlink_metadata(&destination)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(Error::validation(
                "Refusing to uninstall a symlinked pack path",
            ));
        }
        let backup = if destination.exists() {
            let backup = destination.with_file_name(format!(
                ".{}.{}.deleting",
                destination
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("pack"),
                uuid::Uuid::new_v4()
            ));
            fs::rename(&destination, &backup)
                .map_err(|error| Error::io(format!("Failed to stage pack removal: {error}")))?;
            Some(backup)
        } else {
            None
        };
        Ok(PackRemoval {
            destination,
            backup,
            committed: false,
        })
    }

    /// Check if a pack is installed
    pub fn is_installed(&self, pack_ref: &str, version: Option<&str>) -> bool {
        let Ok(path) = self.get_pack_path(pack_ref, version) else {
            return false;
        };
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
    }

    /// List all installed packs
    pub fn list_installed(&self) -> Result<Vec<String>> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }

        let mut packs = Vec::new();

        let entries = fs::read_dir(&self.base_dir).map_err(|e| {
            Error::io(format!(
                "Failed to read pack directory {}: {}",
                self.base_dir.display(),
                e
            ))
        })?;

        for entry in entries {
            let entry =
                entry.map_err(|e| Error::io(format!("Failed to read directory entry: {}", e)))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                Error::io(format!("Failed to inspect pack directory entry: {error}"))
            })?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    packs.push(name.to_string());
                }
            }
        }

        Ok(packs)
    }
}

impl PackReplacement {
    pub fn activate(&mut self) -> Result<&Path> {
        if self.activated {
            return Ok(&self.destination);
        }
        if fs::symlink_metadata(&self.destination)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(Error::validation(
                "Refusing to replace a symlinked pack path",
            ));
        }
        if self.destination.exists() {
            let backup = self.destination.with_file_name(format!(
                ".{}.{}.backup",
                self.destination
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("pack"),
                uuid::Uuid::new_v4()
            ));
            fs::rename(&self.destination, &backup).map_err(|error| {
                Error::io(format!("Failed to stage active pack backup: {error}"))
            })?;
            self.backup = Some(backup);
        }
        if let Err(error) = fs::rename(&self.staging, &self.destination) {
            if let Some(backup) = &self.backup {
                let _ = fs::rename(backup, &self.destination);
            }
            self.backup = None;
            return Err(Error::io(format!(
                "Failed to activate staged pack: {error}"
            )));
        }
        self.activated = true;
        Ok(&self.destination)
    }

    pub fn path(&self) -> &Path {
        &self.destination
    }

    pub fn rollback(&mut self) -> Result<()> {
        if self.activated {
            if self.destination.exists() {
                if let Some(backup) = self.backup.take() {
                    let failed = self.destination.with_file_name(format!(
                        ".{}.{}.failed",
                        self.destination
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("pack"),
                        uuid::Uuid::new_v4()
                    ));
                    fs::rename(&self.destination, &failed).map_err(|error| {
                        Error::io(format!("Failed to move failed pack activation: {error}"))
                    })?;
                    if let Err(error) = fs::rename(&backup, &self.destination) {
                        let _ = fs::rename(&failed, &self.destination);
                        self.backup = Some(backup);
                        return Err(Error::io(format!(
                            "Failed to restore previous pack: {error}"
                        )));
                    }
                    let _ = fs::remove_dir_all(failed);
                } else {
                    fs::remove_dir_all(&self.destination).map_err(|error| {
                        Error::io(format!("Failed to remove failed pack activation: {error}"))
                    })?;
                }
            }
            if let Some(backup) = self.backup.take() {
                fs::rename(&backup, &self.destination).map_err(|error| {
                    Error::io(format!("Failed to restore previous pack: {error}"))
                })?;
            }
            self.activated = false;
        } else if self.staging.exists() {
            fs::remove_dir_all(&self.staging)
                .map_err(|error| Error::io(format!("Failed to remove staged pack: {error}")))?;
        }
        Ok(())
    }

    pub fn commit(mut self) -> Result<PathBuf> {
        if !self.activated {
            return Err(Error::validation(
                "Cannot commit a pack replacement before activation",
            ));
        }
        self.committed = true;
        if let Some(backup) = self.backup.take() {
            let _ = fs::remove_dir_all(&backup);
        }
        Ok(self.destination.clone())
    }
}

impl PackRemoval {
    pub fn commit(mut self) -> Result<()> {
        if let Some(backup) = &self.backup {
            fs::remove_dir_all(backup)
                .map_err(|error| Error::io(format!("Failed to finalize pack removal: {error}")))?;
        }
        self.backup = None;
        self.committed = true;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<()> {
        if let Some(backup) = self.backup.take() {
            fs::rename(&backup, &self.destination).map_err(|error| {
                self.backup = Some(backup);
                Error::io(format!("Failed to roll back pack removal: {error}"))
            })?;
        }
        Ok(())
    }
}

impl Drop for PackRemoval {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

impl Drop for PackReplacement {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

fn validate_storage_ref(pack_ref: &str, version: Option<&str>) -> Result<()> {
    RefValidator::validate_pack_ref(pack_ref)?;
    if let Some(version) = version {
        if version.is_empty()
            || version == "."
            || version == ".."
            || version.contains(['/', '\\'])
            || version.chars().any(char::is_whitespace)
        {
            return Err(Error::validation("Invalid pack storage version"));
        }
    }
    Ok(())
}

/// Calculate SHA256 checksum of a directory
///
/// This recursively hashes all files in the directory in a deterministic order
/// (sorted by path) to produce a consistent checksum.
///
/// # Arguments
///
/// * `path` - Path to the directory
///
/// # Returns
///
/// Hex-encoded SHA256 checksum
pub fn calculate_directory_checksum<P: AsRef<Path>>(path: P) -> Result<String> {
    let path = path.as_ref();

    let root_metadata = fs::symlink_metadata(path).map_err(|error| {
        Error::io(format!(
            "Failed to inspect directory {}: {error}",
            path.display()
        ))
    })?;
    if root_metadata.file_type().is_symlink() {
        return Err(Error::validation(
            "Pack directory checksum rejects symlinks",
        ));
    }
    if !path.exists() {
        return Err(Error::io(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }

    if !path.is_dir() {
        return Err(Error::validation(format!(
            "Path is not a directory: {}",
            path.display()
        )));
    }

    let mut hasher = Sha256::new();
    let mut files: Vec<PathBuf> = Vec::new();

    // Collect all files in sorted order for deterministic hashing
    for entry in WalkDir::new(path)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git")
    {
        let entry = entry.map_err(|e| Error::io(format!("Failed to walk directory: {}", e)))?;
        if entry.file_type().is_symlink() {
            return Err(Error::validation(format!(
                "Pack directory contains symlink: {}",
                entry.path().display()
            )));
        }
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort_by(|left, right| {
        left.strip_prefix(path)
            .unwrap_or(left)
            .cmp(right.strip_prefix(path).unwrap_or(right))
    });

    // Frame each field so paths and contents cannot produce concatenation collisions.
    for file_path in files {
        // Include relative path in hash for structure integrity
        let rel_path = file_path
            .strip_prefix(path)
            .map_err(|e| Error::io(format!("Failed to strip prefix: {}", e)))?;

        let rel_path = rel_path
            .components()
            .map(|component| match component {
                Component::Normal(component) => component.to_str().ok_or_else(|| {
                    Error::validation(format!(
                        "Pack path is not valid UTF-8: {}",
                        file_path.display()
                    ))
                }),
                _ => Err(Error::validation(format!(
                    "Pack path is not a canonical relative path: {}",
                    file_path.display()
                ))),
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        let path_bytes = rel_path.as_bytes();
        hasher.update(b"attune-pack-file-v1");
        hasher.update((path_bytes.len() as u64).to_be_bytes());
        hasher.update(path_bytes);

        // Hash file contents
        let mut file = fs::File::open(&file_path).map_err(|e| {
            Error::io(format!(
                "Failed to open file {}: {}",
                file_path.display(),
                e
            ))
        })?;

        let content_len = file
            .metadata()
            .map_err(|e| {
                Error::io(format!(
                    "Failed to inspect file {}: {}",
                    file_path.display(),
                    e
                ))
            })?
            .len();
        hasher.update(content_len.to_be_bytes());

        let mut bytes_read = 0_u64;
        let mut buffer = [0u8; 8192];
        loop {
            let n = file.read(&mut buffer).map_err(|e| {
                Error::io(format!(
                    "Failed to read file {}: {}",
                    file_path.display(),
                    e
                ))
            })?;
            if n == 0 {
                break;
            }
            bytes_read += n as u64;
            hasher.update(&buffer[..n]);
        }
        if bytes_read != content_len {
            return Err(Error::io(format!(
                "File changed while calculating checksum: {}",
                file_path.display()
            )));
        }
    }

    let result = hasher.finalize();
    Ok(result.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Calculate SHA256 checksum of a single file
///
/// # Arguments
///
/// * `path` - Path to the file
///
/// # Returns
///
/// Hex-encoded SHA256 checksum
pub fn calculate_file_checksum<P: AsRef<Path>>(path: P) -> Result<String> {
    let path = path.as_ref();

    if !path.exists() {
        return Err(Error::io(format!(
            "File does not exist: {}",
            path.display()
        )));
    }

    if !path.is_file() {
        return Err(Error::validation(format!(
            "Path is not a file: {}",
            path.display()
        )));
    }

    let mut hasher = Sha256::new();
    let mut file = fs::File::open(path)
        .map_err(|e| Error::io(format!("Failed to open file {}: {}", path.display(), e)))?;

    let mut buffer = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| Error::io(format!("Failed to read file {}: {}", path.display(), e)))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let result = hasher.finalize();
    Ok(result.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Copy a directory recursively
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    let source_metadata = fs::symlink_metadata(src)
        .map_err(|error| Error::io(format!("Failed to inspect source directory: {error}")))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(Error::validation(
            "Pack copy source must be a real directory",
        ));
    }
    fs::create_dir_all(dst).map_err(|e| {
        Error::io(format!(
            "Failed to create destination directory {}: {}",
            dst.display(),
            e
        ))
    })?;

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Pack storage copy recursively processes validated local directories under the configured pack store.
    for entry in fs::read_dir(src).map_err(|e| {
        Error::io(format!(
            "Failed to read source directory {}: {}",
            src.display(),
            e
        ))
    })? {
        let entry =
            entry.map_err(|e| Error::io(format!("Failed to read directory entry: {}", e)))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(&file_name);

        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| Error::io(format!("Failed to inspect pack entry: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(Error::validation(format!(
                "Pack copy rejects symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            copy_dir_all(&path, &dest_path)?;
        } else if metadata.is_file() {
            fs::copy(&path, &dest_path).map_err(|e| {
                Error::io(format!(
                    "Failed to copy file {} to {}: {}",
                    path.display(),
                    dest_path.display(),
                    e
                ))
            })?;
        } else {
            return Err(Error::validation(format!(
                "Pack copy rejects special file: {}",
                path.display()
            )));
        }
    }

    Ok(())
}

/// Verify a pack's checksum matches the expected value
///
/// # Arguments
///
/// * `pack_path` - Path to the pack directory
/// * `expected_checksum` - Expected SHA256 checksum (hex-encoded)
///
/// # Returns
///
/// `Ok(true)` if checksums match, `Ok(false)` if they don't match,
/// or `Err` on I/O errors
pub fn verify_checksum<P: AsRef<Path>>(pack_path: P, expected_checksum: &str) -> Result<bool> {
    let actual = calculate_directory_checksum(pack_path)?;
    Ok(actual.eq_ignore_ascii_case(expected_checksum))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_pack_storage_paths() {
        let storage = PackStorage::new("/opt/attune/packs");

        let path1 = storage.get_pack_path("core", None).unwrap();
        assert_eq!(path1, PathBuf::from("/opt/attune/packs/core"));

        let path2 = storage.get_pack_path("core", Some("1.0.0")).unwrap();
        assert_eq!(path2, PathBuf::from("/opt/attune/packs/core-1.0.0"));
    }

    #[test]
    fn test_calculate_file_checksum() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"Hello, world!").unwrap();
        drop(file);

        let checksum = calculate_file_checksum(&file_path).unwrap();

        // Known SHA256 of "Hello, world!"
        assert_eq!(
            checksum,
            "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
        );
    }

    #[test]
    fn test_calculate_directory_checksum() {
        let temp_dir = TempDir::new().unwrap();

        // Create a simple directory structure
        let subdir = temp_dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();

        let file1 = temp_dir.path().join("file1.txt");
        let mut f = File::create(&file1).unwrap();
        f.write_all(b"content1").unwrap();
        drop(f);

        let file2 = subdir.join("file2.txt");
        let mut f = File::create(&file2).unwrap();
        f.write_all(b"content2").unwrap();
        drop(f);

        let checksum1 = calculate_directory_checksum(temp_dir.path()).unwrap();

        // Calculate again - should be deterministic
        let checksum2 = calculate_directory_checksum(temp_dir.path()).unwrap();

        assert_eq!(checksum1, checksum2);
        assert_eq!(checksum1.len(), 64); // SHA256 is 64 hex characters
    }

    #[test]
    fn directory_checksum_ignores_git_metadata_but_tracks_pack_files() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("pack.yaml"), "ref: demo\n").unwrap();
        let git_dir = temp_dir.path().join(".git");
        fs::create_dir(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let initial = calculate_directory_checksum(temp_dir.path()).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/changed\n").unwrap();
        fs::write(git_dir.join("index"), "metadata").unwrap();
        assert_eq!(
            initial,
            calculate_directory_checksum(temp_dir.path()).unwrap()
        );

        fs::write(temp_dir.path().join("pack.yaml"), "ref: changed\n").unwrap();
        assert_ne!(
            initial,
            calculate_directory_checksum(temp_dir.path()).unwrap()
        );
    }

    #[test]
    fn directory_checksum_frames_paths_and_contents() {
        let first = TempDir::new().unwrap();
        fs::write(first.path().join("a"), b"bc").unwrap();
        fs::write(first.path().join("d"), b"").unwrap();

        let second = TempDir::new().unwrap();
        fs::write(second.path().join("a"), b"b").unwrap();
        fs::write(second.path().join("cd"), b"").unwrap();

        // The old path+content concatenation encoded both trees as "abcd".
        assert_ne!(
            calculate_directory_checksum(first.path()).unwrap(),
            calculate_directory_checksum(second.path()).unwrap()
        );
    }

    #[test]
    fn directory_checksum_uses_canonical_nested_path_test_vector() {
        let directory = TempDir::new().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("file.txt"), b"fixture\n").unwrap();

        assert_eq!(
            calculate_directory_checksum(directory.path()).unwrap(),
            "e9837162383488cb9b187ea585ce8963634d7d04f75abeb0c43d5456de0d6b13"
        );
    }

    #[test]
    fn traversal_ref_cannot_delete_outside_pack_storage() {
        let temp = TempDir::new().unwrap();
        let storage_dir = temp.path().join("packs");
        fs::create_dir(&storage_dir).unwrap();
        let marker = temp.path().join("marker.txt");
        fs::write(&marker, "keep").unwrap();
        let storage = PackStorage::new(&storage_dir);

        for pack_ref in [".", "..", "../outside", "/tmp/outside", "bad.ref"] {
            assert!(
                storage.uninstall_pack(pack_ref, None).is_err(),
                "{pack_ref}"
            );
        }
        assert_eq!(fs::read_to_string(marker).unwrap(), "keep");
    }

    #[test]
    fn failed_activation_scope_restores_previous_pack() {
        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(old.join("pack.yaml"), "old").unwrap();
        fs::write(new.join("pack.yaml"), "new").unwrap();
        storage.install_pack(&old, "demo", None).unwrap();

        {
            let mut replacement = storage.stage_pack(&new, "demo", None).unwrap();
            replacement.activate().unwrap();
            assert_eq!(
                fs::read_to_string(replacement.path().join("pack.yaml")).unwrap(),
                "new"
            );
            // Simulate registration failure by dropping without commit.
        }

        let active = storage.get_pack_path("demo", None).unwrap();
        assert_eq!(fs::read_to_string(active.join("pack.yaml")).unwrap(), "old");
    }

    #[test]
    fn staging_does_not_change_active_pack() {
        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(old.join("pack.yaml"), "old").unwrap();
        fs::write(new.join("pack.yaml"), "new").unwrap();
        storage.install_pack(&old, "demo", None).unwrap();

        let _replacement = storage.stage_pack(&new, "demo", None).unwrap();
        let active = storage.get_pack_path("demo", None).unwrap();
        assert_eq!(fs::read_to_string(active.join("pack.yaml")).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn checksum_and_install_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(temp.path().join("outside"), "secret").unwrap();
        symlink(temp.path().join("outside"), source.join("linked")).unwrap();

        assert!(calculate_directory_checksum(&source).is_err());
        let storage = PackStorage::new(temp.path().join("packs"));
        assert!(storage.stage_pack(&source, "demo", None).is_err());
    }
}
