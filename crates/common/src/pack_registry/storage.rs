//! Pack Storage Management
//!
//! This module provides utilities for managing pack storage, including:
//! - Checksum calculation (SHA256)
//! - Pack directory management
//! - Storage path resolution
//! - Pack content verification

use crate::error::{Error, Result};
use crate::schema::RefValidator;
use flate2::{write::GzEncoder, Compression, GzBuilder};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackReleaseFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackReleaseManifest {
    pub format_version: u32,
    pub pack_ref: String,
    pub version: String,
    pub files: Vec<PackReleaseFile>,
}

#[derive(Debug, Clone)]
pub struct PublishedPackRelease {
    pub digest: String,
    pub archive_path: PathBuf,
    pub pack_path: PathBuf,
    pub manifest_path: PathBuf,
    pub archive_size: u64,
    pub manifest: PackReleaseManifest,
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

    /// Publish a write-once, content-addressed pack release.
    ///
    /// The archive and manifest are deterministic. A repeated publication of
    /// the same bytes returns the existing release after verifying its digest.
    pub fn publish_release<P: AsRef<Path>>(
        &self,
        source: P,
        pack_ref: &str,
        version: &str,
    ) -> Result<PublishedPackRelease> {
        validate_storage_ref(pack_ref, Some(version))?;
        self.ensure_base_dir()?;

        let releases_dir = self.base_dir.join(".releases");
        let sha256_dir = releases_dir.join("sha256");
        ensure_real_directory(&releases_dir)?;
        ensure_real_directory(&sha256_dir)?;

        let staging = releases_dir.join(format!(".{}.staging", uuid::Uuid::new_v4()));
        let staged_pack = staging.join("pack");
        fs::create_dir(&staging).map_err(|error| {
            Error::io(format!(
                "Failed to create release staging directory: {error}"
            ))
        })?;
        if let Err(error) = copy_release_tree(source.as_ref(), &staged_pack) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let manifest = match build_release_manifest(&staged_pack, pack_ref, version) {
            Ok(manifest) => manifest,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let archive_path = staging.join("pack.tar.gz");
        if let Err(error) = write_deterministic_archive(&archive_path, &staged_pack, &manifest) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        if build_release_manifest(&staged_pack, pack_ref, version)? != manifest {
            let _ = fs::remove_dir_all(&staging);
            return Err(Error::validation(
                "Pack files changed while building the release archive",
            ));
        }
        let digest = calculate_file_checksum(&archive_path)?;
        let archive_size = fs::metadata(&archive_path)
            .map_err(|error| Error::io(format!("Failed to inspect release archive: {error}")))?
            .len();
        let manifest_path = staging.join("manifest.json");
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        fs::write(&manifest_path, manifest_bytes)
            .map_err(|error| Error::io(format!("Failed to write release manifest: {error}")))?;

        let destination = sha256_dir.join(&digest);
        if destination.exists() {
            let _ = fs::remove_dir_all(&staging);
            return verify_published_release(&destination, &digest, pack_ref, version);
        }

        if let Err(error) = fs::rename(&staging, &destination) {
            if destination.exists() {
                let _ = fs::remove_dir_all(&staging);
                return verify_published_release(&destination, &digest, pack_ref, version);
            }
            let _ = fs::remove_dir_all(&staging);
            return Err(Error::io(format!(
                "Failed to publish pack release: {error}"
            )));
        }
        if let Err(error) = make_tree_read_only(&destination) {
            let _ = remove_read_only_tree(&destination);
            return Err(error);
        }

        Ok(PublishedPackRelease {
            digest,
            archive_path: destination.join("pack.tar.gz"),
            pack_path: destination.join("pack"),
            manifest_path: destination.join("manifest.json"),
            archive_size,
            manifest,
        })
    }

    /// Remove an immutable release only when the stored path matches its
    /// content-addressed location under this pack store.
    pub fn remove_release_tree(&self, digest: &str, content_path: &str) -> Result<bool> {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::validation("Invalid pack release digest"));
        }

        let releases_dir = self.base_dir.join(".releases");
        let releases_root = releases_dir.join("sha256");
        let release_dir = releases_root.join(digest);
        if Path::new(content_path) != release_dir.join("pack") {
            return Err(Error::validation(
                "Pack release content path does not match its digest",
            ));
        }

        for path in [&self.base_dir, &releases_dir, &releases_root] {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => {
                    return Err(Error::io(format!(
                        "Failed to inspect pack release root {}: {error}",
                        path.display()
                    )))
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(Error::validation(format!(
                    "Pack release root must be a real directory: {}",
                    path.display()
                )));
            }
        }

        let metadata = match fs::symlink_metadata(&release_dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(Error::io(format!(
                    "Failed to inspect pack release {}: {error}",
                    release_dir.display()
                )))
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::validation(
                "Pack release tree must be a real directory",
            ));
        }

        remove_read_only_tree(&release_dir).map_err(|error| {
            Error::io(format!(
                "Failed to remove pack release {}: {error}",
                release_dir.display()
            ))
        })?;
        Ok(true)
    }

    /// Assign a staged candidate to its install record so internal transport
    /// can locate it without accepting a filesystem path from a worker.
    pub fn bind_candidate_to_install(
        &self,
        candidate: &Path,
        pack_ref: &str,
        pack_install_id: i64,
    ) -> Result<PathBuf> {
        RefValidator::validate_pack_ref(pack_ref)?;
        if pack_install_id <= 0 {
            return Err(Error::validation("Pack install ID must be positive"));
        }
        let candidate_parent = candidate.parent().ok_or_else(|| {
            Error::validation("Pack test candidate must be directly under the pack storage root")
        })?;
        if candidate_parent != self.base_dir || !candidate.is_dir() {
            return Err(Error::validation(
                "Pack test candidate must be directly under the pack storage root",
            ));
        }
        let destination = self.base_dir.join(format!(".pack-test-{pack_install_id}"));
        fs::rename(candidate, &destination)
            .map_err(|error| Error::io(format!("Failed to assign pack test candidate: {error}")))?;
        Ok(destination)
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
    /// Keep the staged candidate for an external validation step.
    ///
    /// The caller owns cleanup after this returns. The active destination has
    /// not been changed.
    pub fn into_staging_path(mut self) -> PathBuf {
        self.committed = true;
        self.staging.clone()
    }

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
            exchange_paths(&self.staging, &self.destination)?;
            self.backup = Some(self.staging.clone());
        } else {
            fs::rename(&self.staging, &self.destination)
                .map_err(|error| Error::io(format!("Failed to activate staged pack: {error}")))?;
        }
        self.activated = true;
        Ok(&self.destination)
    }

    /// Publish after the database commit, removing any old projection if the
    /// atomic switch fails. This prevents callers from serving stale content.
    pub fn publish_fail_closed(&mut self) -> Result<&Path> {
        if let Err(publish_error) = self.activate() {
            let cleanup_result = remove_path_if_exists(&self.destination);
            return match cleanup_result {
                Ok(()) => Err(publish_error),
                Err(cleanup_error) => Err(Error::io(format!(
                    "{publish_error}; failed to remove stale pack projection: {cleanup_error}"
                ))),
            };
        }
        Ok(&self.destination)
    }

    pub fn path(&self) -> &Path {
        &self.destination
    }

    pub fn staged_path(&self) -> &Path {
        &self.staging
    }

    pub fn rollback(&mut self) -> Result<()> {
        if self.activated {
            if let Some(backup) = self.backup.take() {
                if let Err(error) = exchange_paths(&self.destination, &backup) {
                    self.backup = Some(backup);
                    return Err(error);
                }
                remove_path_if_exists(&backup)?;
            } else {
                remove_path_if_exists(&self.destination)?;
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

#[cfg(target_os = "linux")]
fn exchange_paths(left: &Path, right: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let left = CString::new(left.as_os_str().as_bytes())
        .map_err(|_| Error::validation("Pack storage path contains a NUL byte"))?;
    let right = CString::new(right.as_os_str().as_bytes())
        .map_err(|_| Error::validation("Pack storage path contains a NUL byte"))?;
    // Both paths are siblings in the pack store, so RENAME_EXCHANGE provides
    // one atomic visibility point without a missing-directory interval.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(Error::io(format!(
            "Failed to atomically exchange pack projection: {}",
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(not(target_os = "linux"))]
fn exchange_paths(_left: &Path, _right: &Path) -> Result<()> {
    Err(Error::io(
        "Atomic pack projection replacement requires Linux renameat2",
    ))
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(Error::io(format!(
                "Failed to inspect pack projection {}: {error}",
                path.display()
            )))
        }
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|error| {
        Error::io(format!(
            "Failed to remove pack projection {}: {error}",
            path.display()
        ))
    })
}

impl PackRemoval {
    /// Disable rollback after the database commit, then remove the backup.
    ///
    /// A cleanup error leaves the backup for a later retry but must never
    /// restore the active projection because the database deletion is durable.
    pub fn finalize_after_commit(&mut self) -> Result<()> {
        self.committed = true;
        if let Some(backup) = &self.backup {
            fs::remove_dir_all(backup).map_err(|error| {
                Error::io(format!(
                    "Failed to remove staged pack backup {}: {error}",
                    backup.display()
                ))
            })?;
        }
        self.backup = None;
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

fn ensure_real_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir(path).map_err(|error| {
            Error::io(format!(
                "Failed to create directory {}: {error}",
                path.display()
            ))
        })?;
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| Error::io(format!("Failed to inspect {}: {error}", path.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::validation(format!(
            "Pack release path must be a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn release_files(path: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(path).sort_by_file_name() {
        let entry = entry.map_err(|error| Error::io(format!("Failed to walk pack: {error}")))?;
        if entry.file_type().is_symlink() {
            return Err(Error::validation(format!(
                "Pack release rejects symlink: {}",
                entry.path().display()
            )));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(path)
            .map_err(|error| Error::io(format!("Failed to resolve pack release path: {error}")))?;
        let relative = canonical_relative_path(relative)?;
        files.push((relative, entry.path().to_path_buf()));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn canonical_relative_path(path: &Path) -> Result<String> {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned).ok_or_else(|| {
                Error::validation(format!("Pack path is not valid UTF-8: {}", path.display()))
            }),
            _ => Err(Error::validation(format!(
                "Pack path is not canonical: {}",
                path.display()
            ))),
        })
        .collect::<Result<Vec<_>>>()
        .map(|components| components.join("/"))
}

fn build_release_manifest(
    pack_path: &Path,
    pack_ref: &str,
    version: &str,
) -> Result<PackReleaseManifest> {
    let files = release_files(pack_path)?
        .into_iter()
        .map(|(path, file_path)| {
            let metadata = fs::metadata(&file_path).map_err(|error| {
                Error::io(format!(
                    "Failed to inspect release file {}: {error}",
                    file_path.display()
                ))
            })?;
            Ok(PackReleaseFile {
                path,
                size: metadata.len(),
                sha256: calculate_file_checksum(&file_path)?,
                executable: file_is_executable(&metadata),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(PackReleaseManifest {
        format_version: 1,
        pack_ref: pack_ref.to_string(),
        version: version.to_string(),
        files,
    })
}

#[cfg(unix)]
fn file_is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn file_is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

fn write_deterministic_archive(
    archive_path: &Path,
    pack_path: &Path,
    manifest: &PackReleaseManifest,
) -> Result<()> {
    let file = fs::File::create(archive_path)
        .map_err(|error| Error::io(format!("Failed to create release archive: {error}")))?;
    let encoder: GzEncoder<fs::File> = GzBuilder::new().mtime(0).write(file, Compression::best());
    let mut archive = tar::Builder::new(encoder);
    archive.mode(tar::HeaderMode::Deterministic);
    for entry in &manifest.files {
        let file_path = pack_path.join(&entry.path);
        let mut file = fs::File::open(&file_path).map_err(|error| {
            Error::io(format!(
                "Failed to open release file {}: {error}",
                file_path.display()
            ))
        })?;
        let mut header = tar::Header::new_gnu();
        header.set_size(entry.size);
        header.set_mode(if entry.executable { 0o755 } else { 0o644 });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        let archive_name = format!("{}/{}", manifest.pack_ref, entry.path);
        archive
            .append_data(&mut header, archive_name, &mut file)
            .map_err(|error| {
                Error::io(format!("Failed to append release archive entry: {error}"))
            })?;
    }
    archive
        .finish()
        .map_err(|error| Error::io(format!("Failed to finish release archive: {error}")))?;
    archive
        .into_inner()
        .and_then(GzEncoder::finish)
        .map_err(|error| Error::io(format!("Failed to finish release compression: {error}")))?;
    Ok(())
}

fn verify_published_release(
    release_dir: &Path,
    digest: &str,
    pack_ref: &str,
    version: &str,
) -> Result<PublishedPackRelease> {
    let archive_path = release_dir.join("pack.tar.gz");
    let actual_digest = calculate_file_checksum(&archive_path)?;
    if actual_digest != digest {
        return Err(Error::validation(format!(
            "Existing pack release {} failed digest verification",
            release_dir.display()
        )));
    }
    let manifest_path = release_dir.join("manifest.json");
    let manifest: PackReleaseManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| Error::io(format!("Failed to read release manifest: {error}")))?,
    )?;
    if manifest.pack_ref != pack_ref || manifest.version != version {
        return Err(Error::validation(
            "Existing pack release manifest has conflicting identity",
        ));
    }
    if build_release_manifest(&release_dir.join("pack"), pack_ref, version)? != manifest {
        return Err(Error::validation(
            "Existing pack release file tree does not match its manifest",
        ));
    }
    let archive_size = fs::metadata(&archive_path)
        .map_err(|error| Error::io(format!("Failed to inspect release archive: {error}")))?
        .len();
    let pack_path = release_dir.join("pack");
    if !pack_path.is_dir() {
        return Err(Error::validation("Existing pack release has no file tree"));
    }
    Ok(PublishedPackRelease {
        digest: digest.to_string(),
        archive_path,
        pack_path,
        manifest_path,
        archive_size,
        manifest,
    })
}

fn copy_release_tree(src: &Path, dst: &Path) -> Result<()> {
    let source_metadata = fs::symlink_metadata(src)
        .map_err(|error| Error::io(format!("Failed to inspect source directory: {error}")))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(Error::validation(
            "Pack release source must be a real directory",
        ));
    }
    fs::create_dir(dst)
        .map_err(|error| Error::io(format!("Failed to create release pack directory: {error}")))?;
    for entry in fs::read_dir(src)
        .map_err(|error| Error::io(format!("Failed to read pack source: {error}")))?
    {
        let entry =
            entry.map_err(|error| Error::io(format!("Failed to read pack entry: {error}")))?;
        if entry.file_name() == ".git" {
            continue;
        }
        let source = entry.path();
        let destination = dst.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| Error::io(format!("Failed to inspect pack entry: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(Error::validation(format!(
                "Pack release rejects symlink: {}",
                source.display()
            )));
        }
        if metadata.is_dir() {
            copy_release_tree(&source, &destination)?;
        } else if metadata.is_file() {
            fs::copy(&source, &destination).map_err(|error| {
                Error::io(format!(
                    "Failed to copy release file {}: {error}",
                    source.display()
                ))
            })?;
        } else {
            return Err(Error::validation(format!(
                "Pack release rejects special file: {}",
                source.display()
            )));
        }
    }
    Ok(())
}

fn make_tree_read_only(path: &Path) -> Result<()> {
    let mut entries = WalkDir::new(path)
        .contents_first(true)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| Error::io(format!("Failed to inspect release tree: {error}")))?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.depth()));
    for entry in entries {
        let metadata = fs::metadata(entry.path())
            .map_err(|error| Error::io(format!("Failed to inspect release path: {error}")))?;
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if metadata.is_dir() || file_is_executable(&metadata) {
                0o555
            } else {
                0o444
            };
            permissions.set_mode(mode);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(true);
        fs::set_permissions(entry.path(), permissions)
            .map_err(|error| Error::io(format!("Failed to protect release path: {error}")))?;
    }
    Ok(())
}

fn remove_read_only_tree(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let entries = WalkDir::new(path)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if entries.iter().any(|entry| entry.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pack release tree contains a symlink",
        ));
    }
    for entry in entries {
        let metadata = fs::symlink_metadata(entry.path())?;
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(entry.path(), permissions)?;
    }
    fs::remove_dir_all(path)
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
    fn immutable_releases_are_deterministic_and_preserved() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("pack.yaml"),
            "ref: demo\nname: Demo\nversion: 1.0.0\n",
        )
        .unwrap();
        fs::write(source.join("run.sh"), "#!/bin/sh\necho first\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(source.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let storage = PackStorage::new(temp.path().join("packs"));

        let first = storage.publish_release(&source, "demo", "1.0.0").unwrap();
        let repeated = storage.publish_release(&source, "demo", "1.0.0").unwrap();
        let independent = PackStorage::new(temp.path().join("other-packs"))
            .publish_release(&source, "demo", "1.0.0")
            .unwrap();

        assert_eq!(first.digest, repeated.digest);
        assert_eq!(first.digest, independent.digest);
        assert_eq!(first.archive_path, repeated.archive_path);
        assert_eq!(
            fs::read(&first.archive_path).unwrap(),
            fs::read(&independent.archive_path).unwrap()
        );
        assert!(first
            .manifest
            .files
            .iter()
            .any(|file| file.path == "run.sh" && file.executable));

        fs::write(
            source.join("pack.yaml"),
            "ref: demo\nname: Demo\nversion: 2.0.0\n",
        )
        .unwrap();
        let second = storage.publish_release(&source, "demo", "2.0.0").unwrap();

        assert_ne!(first.digest, second.digest);
        assert!(first.archive_path.is_file());
        assert!(first.pack_path.join("pack.yaml").is_file());
        assert!(second.archive_path.is_file());
    }

    #[test]
    fn existing_release_blob_must_match_its_digest() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("pack.yaml"), "ref: demo\nversion: 1.0.0\n").unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let release = storage.publish_release(&source, "demo", "1.0.0").unwrap();

        let mut permissions = fs::metadata(&release.archive_path).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(&release.archive_path, permissions).unwrap();
        fs::write(&release.archive_path, b"tampered").unwrap();

        assert!(storage.publish_release(&source, "demo", "1.0.0").is_err());
    }

    #[test]
    fn removes_only_the_release_tree_matching_the_digest_and_content_path() {
        let temp = TempDir::new().unwrap();
        let packs = temp.path().join("packs");
        let digest = "a".repeat(64);
        let release = packs.join(".releases/sha256").join(&digest);
        fs::create_dir_all(release.join("pack")).unwrap();
        fs::write(release.join("pack/pack.yaml"), "ref: demo\n").unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "keep").unwrap();
        let storage = PackStorage::new(&packs);

        assert!(storage
            .remove_release_tree(&digest, outside.join("pack").to_str().unwrap())
            .is_err());
        assert!(release.exists());
        assert!(outside.join("keep").exists());

        assert!(storage
            .remove_release_tree(&digest, release.join("pack").to_str().unwrap())
            .unwrap());
        assert!(!release.exists());
        assert!(outside.join("keep").exists());
    }

    #[cfg(unix)]
    #[test]
    fn release_cleanup_rejects_symlinked_digest_directory() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let packs = temp.path().join("packs");
        let root = packs.join(".releases/sha256");
        fs::create_dir_all(&root).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "keep").unwrap();
        let digest = "b".repeat(64);
        symlink(&outside, root.join(&digest)).unwrap();

        let storage = PackStorage::new(&packs);
        assert!(storage
            .remove_release_tree(&digest, root.join(&digest).join("pack").to_str().unwrap(),)
            .is_err());
        assert!(outside.join("keep").exists());
    }

    #[test]
    fn binds_candidate_to_install_scoped_path() {
        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path());
        storage.ensure_base_dir().unwrap();
        let candidate = temp.path().join(".demo.random.staging");
        fs::create_dir(&candidate).unwrap();

        let bound = storage
            .bind_candidate_to_install(&candidate, "demo", 42)
            .unwrap();

        assert_eq!(bound, temp.path().join(".pack-test-42"));
        assert!(bound.is_dir());
        assert!(!candidate.exists());
    }

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
    fn staged_uninstall_restores_pack_when_database_commit_fails() {
        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("pack.yaml"), "ref: demo\n").unwrap();
        storage.install_pack(&source, "demo", None).unwrap();

        {
            let _removal = storage.stage_uninstall("demo", None).unwrap();
            assert!(!storage.is_installed("demo", None));
            // Simulate the database transaction returning without a commit.
        }

        assert!(storage.is_installed("demo", None));
        assert_eq!(
            fs::read_to_string(
                storage
                    .get_pack_path("demo", None)
                    .unwrap()
                    .join("pack.yaml")
            )
            .unwrap(),
            "ref: demo\n"
        );
    }

    #[test]
    fn cleanup_failure_after_database_commit_does_not_restore_pack() {
        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("pack.yaml"), "ref: demo\n").unwrap();
        storage.install_pack(&source, "demo", None).unwrap();

        let backup = {
            let mut removal = storage.stage_uninstall("demo", None).unwrap();
            let backup = removal.backup.clone().unwrap();
            fs::remove_dir_all(&backup).unwrap();
            fs::write(&backup, "cleanup failure fixture").unwrap();

            assert!(removal.finalize_after_commit().is_err());
            backup
        };

        assert!(!storage.is_installed("demo", None));
        assert!(backup.exists(), "failed cleanup must remain retryable");
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

    #[cfg(target_os = "linux")]
    #[test]
    fn activation_atomically_switches_complete_directory_trees() {
        use std::os::fd::AsRawFd;

        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(old.join("version"), "old").unwrap();
        fs::write(old.join("component"), "old component").unwrap();
        fs::write(new.join("version"), "new").unwrap();
        fs::write(new.join("component"), "new component").unwrap();
        storage.install_pack(&old, "demo", None).unwrap();
        let active = storage.get_pack_path("demo", None).unwrap();
        let open_reader = fs::File::open(&active).unwrap();

        let mut replacement = storage.stage_pack(&new, "demo", None).unwrap();
        replacement.activate().unwrap();

        let reader_path = PathBuf::from(format!("/proc/self/fd/{}", open_reader.as_raw_fd()));
        assert_eq!(
            fs::read_to_string(reader_path.join("version")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(reader_path.join("component")).unwrap(),
            "old component"
        );
        assert_eq!(fs::read_to_string(active.join("version")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(active.join("component")).unwrap(),
            "new component"
        );
        replacement.commit().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_publication_removes_stale_projection() {
        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(old.join("pack.yaml"), "old").unwrap();
        fs::write(new.join("pack.yaml"), "new").unwrap();
        storage.install_pack(&old, "demo", None).unwrap();

        let mut replacement = storage.stage_pack(&new, "demo", None).unwrap();
        fs::remove_dir_all(replacement.staged_path()).unwrap();
        let active = storage.get_pack_path("demo", None).unwrap();

        assert!(replacement.publish_fail_closed().is_err());
        assert!(
            !active.exists(),
            "the old projection must not remain readable"
        );
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

    #[test]
    fn candidate_staging_survives_replacement_drop_without_touching_active_pack() {
        let temp = TempDir::new().unwrap();
        let storage = PackStorage::new(temp.path().join("packs"));
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(old.join("pack.yaml"), "old").unwrap();
        fs::write(new.join("pack.yaml"), "new").unwrap();
        storage.install_pack(&old, "demo", None).unwrap();

        let candidate = storage
            .stage_pack(&new, "demo", None)
            .unwrap()
            .into_staging_path();
        let active = storage.get_pack_path("demo", None).unwrap();

        assert!(candidate.exists());
        assert_eq!(
            fs::read_to_string(candidate.join("pack.yaml")).unwrap(),
            "new"
        );
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
