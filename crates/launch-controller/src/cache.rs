//! Controller-owned, fixed-volume input cache. No engine archive API or caller
//! paths are accepted. Bodies are canonical ZIP and extraction never follows
//! links. All publication is serialized across controller processes sharing the
//! volume; workload containers see only a selected `body` subdirectory.
use attune_common::workload_launch::{LaunchManifest, PinnedLaunchInput, Sha256Digest};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
use thiserror::Error;
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("cache I/O failed")]
    Io(#[from] io::Error),
    #[error("invalid, escaping or unsupported ZIP/tree entry")]
    UnsafeEntry,
    #[error("input or stage exceeds controller/admitted quota")]
    Quota,
    #[error("input body does not match admitted immutable pin")]
    PinMismatch,
    #[error("invalid ZIP body")]
    Zip(#[from] zip::result::ZipError),
    #[error("private attempt directory already exists or is not owned")]
    AttemptConflict,
}
pub type Result<T> = std::result::Result<T, CacheError>;

#[derive(Clone, Copy)]
pub struct CacheLimits {
    pub compressed_bytes: u64,
    pub expanded_bytes: u64,
    pub files: u64,
}

/// The root is configured once by the operator, never selected by a request.
/// Production runs this custodian as root; pack processes run as uid 1000.
pub struct CacheStore {
    root: PathBuf,
    limits: CacheLimits,
}
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        // Closing also unlocks on uncertain cleanup; locks never use a pack fd.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl CacheStore {
    pub fn open(root: PathBuf, limits: CacheLimits) -> Result<Self> {
        if !root.is_absolute()
            || limits.compressed_bytes == 0
            || limits.expanded_bytes == 0
            || limits.files == 0
        {
            return Err(CacheError::Quota);
        }
        fs::create_dir_all(&root)?;
        let metadata = fs::symlink_metadata(&root)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(CacheError::UnsafeEntry);
        }
        for child in ["inputs", "private", "pending"] {
            let path = root.join(child);
            if !path.exists() {
                fs::create_dir(&path)?;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_dir()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o022 != 0
            {
                return Err(CacheError::UnsafeEntry);
            }
        }
        Ok(Self { root, limits })
    }
    fn lock(&self) -> Result<Lock> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.root.join("cache.lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Lock(file))
    }
    /// Call on a blocking worker. The body must already have been downloaded
    /// under a current CP input grant, with a bounded transport body reader.
    pub fn install_input(&self, pin: &PinnedLaunchInput, body: &mut File) -> Result<PathBuf> {
        if pin.size_bytes > self.limits.compressed_bytes
            || pin.max_expanded_bytes > self.limits.expanded_bytes
            || pin.max_files > self.limits.files
        {
            return Err(CacheError::Quota);
        }
        verify_body(body, pin)?;
        let _lock = self.lock()?;
        let relative = crate::engine::input_subpath(pin.kind, &pin.sha256);
        let destination = self.root.join(relative);
        let parent = destination.parent().ok_or(CacheError::UnsafeEntry)?;
        if parent.exists() {
            if !fs::symlink_metadata(parent)?.is_dir()
                || fs::read(parent.join("receipt"))? != receipt(pin)?
            {
                return Err(CacheError::PinMismatch);
            }
            // The parent is custodian-owned and never mounted in a workload.
            return Ok(destination);
        }
        let pending = tempfile::Builder::new()
            .prefix("input-")
            .tempdir_in(self.root.join("pending"))?;
        let output = pending.path().join("body");
        fs::create_dir(&output)?;
        extract_zip(
            body,
            &output,
            pin.max_expanded_bytes,
            pin.max_files,
            pin.kind.container_root(),
        )?;
        seal_tree(&output, pin.kind.container_root())?;
        fs::write(pending.path().join("receipt"), receipt(pin)?)?;
        fs::create_dir_all(parent.parent().ok_or(CacheError::UnsafeEntry)?)?;
        // Global cache lock owns validation through publication. No workload
        // sees this temporary directory or the cache parent.
        fs::rename(pending.path(), parent)?;
        Ok(destination)
    }
    /// Each nonce is single-use. Restarts must reconcile the existing launch,
    /// not reuse or reset a writable stage. No recursive delete by caller path.
    pub fn prepare_private(&self, manifest: &LaunchManifest) -> Result<()> {
        let _lock = self.lock()?;
        let path = self
            .root
            .join(format!("private/{}", manifest.engine_name_nonce.simple()));
        fs::create_dir(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                CacheError::AttemptConflict
            } else {
                error.into()
            }
        })?;
        if manifest.stage.is_some() {
            let stage = path.join("runtime");
            fs::create_dir(&stage)?;
            make_workload_owned(&stage)?;
        }
        if manifest.pack_access == attune_common::workload_launch::PackAccess::CandidateCopy {
            let pin = manifest
                .inputs
                .iter()
                .find(|pin| pin.kind == attune_common::workload_launch::LaunchInputKind::Pack)
                .ok_or(CacheError::PinMismatch)?;
            let source = self
                .root
                .join(crate::engine::input_subpath(pin.kind, &pin.sha256));
            let output = path.join("pack");
            copy_private(&source, &output)?;
        }
        Ok(())
    }
    /// Admission budget, not a live filesystem quota. Concurrent/stale writers
    /// can consume disk; the service also limits active launches and always
    /// retains exact ownership for post-removal cleanup.
    pub fn ensure_capacity(
        &self,
        manifest: &LaunchManifest,
        minimum_free_bytes: u64,
    ) -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let _lock = self.lock()?;
        let mut required = self.limits.compressed_bytes;
        for input in &manifest.inputs {
            required = required
                .checked_add(input.max_expanded_bytes)
                .and_then(|value| value.checked_add(input.size_bytes))
                .ok_or(CacheError::Quota)?;
        }
        if let Some(stage) = &manifest.stage {
            required = required
                .checked_add(stage.quota_bytes)
                .ok_or(CacheError::Quota)?;
        }
        if manifest.pack_access == attune_common::workload_launch::PackAccess::CandidateCopy {
            required = required
                .checked_add(
                    manifest
                        .inputs
                        .iter()
                        .find(|input| {
                            input.kind == attune_common::workload_launch::LaunchInputKind::Pack
                        })
                        .ok_or(CacheError::PinMismatch)?
                        .max_expanded_bytes,
                )
                .ok_or(CacheError::Quota)?;
        }
        let name = std::ffi::CString::new(self.root.as_os_str().as_bytes())
            .map_err(|_| CacheError::UnsafeEntry)?;
        let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(name.as_ptr(), status.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let status = unsafe { status.assume_init() };
        let available = (status.f_bavail as u128) * (status.f_frsize as u128);
        if available < required as u128 + minimum_free_bytes as u128 {
            return Err(CacheError::Quota);
        }
        Ok(())
    }
    /// Record an actually received Engine Remove acknowledgement before CP ACK.
    /// This receipt is never synthesized from inspect 404 or a timeout.
    pub fn record_removal(
        &self,
        manifest: &LaunchManifest,
        resource: &attune_common::workload_launch::EngineResourceEvidence,
    ) -> Result<()> {
        let digest = manifest
            .admitted_digest()
            .map_err(|_| CacheError::PinMismatch)?;
        if resource.engine_id != manifest.engine_id
            || resource.engine_name_nonce != manifest.engine_name_nonce
            || resource.admitted_digest != digest
        {
            return Err(CacheError::PinMismatch);
        }
        let _lock = self.lock()?;
        let directory = self.root.join("removed");
        fs::create_dir_all(&directory)?;
        let destination = directory.join(manifest.engine_name_nonce.simple().to_string());
        let bytes = serde_json::to_vec(resource).map_err(|_| CacheError::PinMismatch)?;
        if destination.exists() {
            if fs::read(destination)? != bytes {
                return Err(CacheError::PinMismatch);
            }
            return Ok(());
        }
        let mut pending = tempfile::NamedTempFile::new_in(&directory)?;
        use std::io::Write;
        pending.write_all(&bytes)?;
        pending.as_file().sync_all()?;
        pending
            .persist_noclobber(&destination)
            .map_err(|error| CacheError::Io(error.error))?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }
    pub fn removal_receipt(
        &self,
        manifest: &LaunchManifest,
    ) -> Result<Option<attune_common::workload_launch::EngineResourceEvidence>> {
        let _lock = self.lock()?;
        let path = self
            .root
            .join("removed")
            .join(manifest.engine_name_nonce.simple().to_string());
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if file.metadata()?.len() > 8192 {
            return Err(CacheError::PinMismatch);
        }
        let receipt: attune_common::workload_launch::EngineResourceEvidence =
            serde_json::from_reader(file).map_err(|_| CacheError::PinMismatch)?;
        if receipt.admitted_digest
            != manifest
                .admitted_digest()
                .map_err(|_| CacheError::PinMismatch)?
            || receipt.engine_id != manifest.engine_id
            || receipt.engine_name_nonce != manifest.engine_name_nonce
        {
            return Err(CacheError::PinMismatch);
        }
        Ok(Some(receipt))
    }
    pub fn cleanup_settled_private(
        &self,
        manifest: &LaunchManifest,
        status: &attune_common::workload_launch::LaunchStatus,
    ) -> Result<()> {
        use attune_common::workload_launch::{LaunchPublicationState, LaunchState};
        if status.validate_shape().is_err()
            || status.state != LaunchState::Stopped
            || status.request.launch_id != manifest.launch_id
            || status.request.admitted_digest
                != manifest
                    .admitted_digest()
                    .map_err(|_| CacheError::PinMismatch)?
            || matches!(
                status.publication,
                LaunchPublicationState::Pending | LaunchPublicationState::Uploading
            )
        {
            return Err(CacheError::PinMismatch);
        }
        let _lock = self.lock()?;
        let path = self
            .root
            .join(format!("private/{}", manifest.engine_name_nonce.simple()));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
            Ok(_) => return Err(CacheError::AttemptConflict),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
    /// Caller must first obtain CP Stopped + successful setup and Wait+Remove
    /// evidence. No container holding a writable mount may remain at this call.
    pub fn archive_settled_stage(
        &self,
        manifest: &LaunchManifest,
        status: &attune_common::workload_launch::LaunchStatus,
    ) -> Result<File> {
        let stage = manifest.stage.as_ref().ok_or(CacheError::PinMismatch)?;
        if !status.permits_stage_publication()
            || status.request.launch_id != manifest.launch_id
            || status.request.admitted_digest
                != manifest
                    .admitted_digest()
                    .map_err(|_| CacheError::PinMismatch)?
        {
            return Err(CacheError::PinMismatch);
        }
        let _lock = self.lock()?;
        let path = self.root.join(crate::engine::stage_subpath(manifest));
        let entries = inspect_tree(
            &path,
            stage.quota_bytes.min(self.limits.expanded_bytes),
            stage.quota_files.min(self.limits.files),
            attune_common::workload_launch::RUNTIME_ROOT,
        )?;
        let mut output = BoundedFile {
            file: tempfile::tempfile_in(self.root.join("pending"))?,
            limit: self.limits.compressed_bytes,
        };
        {
            let mut zip = ZipWriter::new(&mut output);
            for entry in entries {
                let name = entry.relative.to_str().ok_or(CacheError::UnsafeEntry)?;
                let options = SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(if entry.executable { 0o755 } else { 0o644 });
                if let Some(target) = entry.link {
                    zip.add_symlink(
                        name,
                        target.to_str().ok_or(CacheError::UnsafeEntry)?,
                        options,
                    )?;
                } else if entry.directory {
                    zip.add_directory(format!("{name}/"), options)?;
                } else {
                    zip.start_file(name, options)?;
                    let mut file = OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(path.join(&entry.relative))?;
                    io::copy(&mut file, &mut zip)?;
                }
            }
            zip.finish()?;
        }
        use std::io::{Seek, SeekFrom};
        output.file.seek(SeekFrom::Start(0))?;
        Ok(output.file)
    }
}
struct BoundedFile {
    file: File,
    limit: u64,
}
impl io::Write for BoundedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        use io::Seek;
        let position = self.file.stream_position()?;
        if position.saturating_add(bytes.len() as u64) > self.limit {
            return Err(io::Error::other("archive output exceeds bound"));
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl io::Seek for BoundedFile {
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        let target = match position {
            io::SeekFrom::Start(value) => value as i128,
            io::SeekFrom::Current(value) => self.file.stream_position()? as i128 + value as i128,
            io::SeekFrom::End(value) => self.file.metadata()?.len() as i128 + value as i128,
        };
        if target < 0 || target > self.limit as i128 {
            return Err(io::Error::other("archive output seek exceeds bound"));
        }
        self.file.seek(io::SeekFrom::Start(target as u64))
    }
}
fn receipt(pin: &PinnedLaunchInput) -> Result<Vec<u8>> {
    // Content is reusable across object IDs/pins; limits are checked at every
    // admission, and the receipt records actual pin limits to prevent weaker
    // validation from masquerading as a stronger ceiling.
    Ok(format!(
        "{}\n{}\n{}\n{}\n",
        pin.sha256.as_str(),
        pin.size_bytes,
        pin.max_expanded_bytes,
        pin.max_files
    )
    .into_bytes())
}
fn verify_body(body: &mut File, pin: &PinnedLaunchInput) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    body.seek(SeekFrom::Start(0))?;
    if body.metadata()?.len() != pin.size_bytes {
        return Err(CacheError::PinMismatch);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = body.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let digest = Sha256Digest::parse(
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .map_err(|_| CacheError::PinMismatch)?;
    if digest != pin.sha256 {
        return Err(CacheError::PinMismatch);
    }
    body.seek(SeekFrom::Start(0))?;
    Ok(())
}
fn archive_policy(
    container_root: &str,
    compressed: u64,
    bytes: u64,
    files: u64,
) -> Result<attune_common::workload_archive::ArchivePolicy> {
    use attune_common::workload_launch::{LaunchInputKind, PACK_ROOT, RUNNER_ROOT, RUNTIME_ROOT};
    let root = match container_root {
        PACK_ROOT => LaunchInputKind::Pack,
        RUNTIME_ROOT => LaunchInputKind::Runtime,
        RUNNER_ROOT => LaunchInputKind::Runner,
        _ => return Err(CacheError::UnsafeEntry),
    };
    Ok(attune_common::workload_archive::ArchivePolicy {
        root,
        max_compressed_bytes: compressed,
        max_expanded_bytes: bytes,
        max_files: files,
    })
}
fn extract_zip(
    body: &mut File,
    root: &Path,
    bytes_limit: u64,
    files_limit: u64,
    container_root: &str,
) -> Result<()> {
    use attune_common::workload_archive::{validate_zip, ArchiveEntryKind};
    let policy = archive_policy(
        container_root,
        body.metadata()?.len(),
        bytes_limit,
        files_limit,
    )?;
    let validated = validate_zip(body, &policy).map_err(|_| CacheError::UnsafeEntry)?;
    let mut archive = ZipArchive::new(body)?;
    let mut links = Vec::new();
    for (index, entry) in validated.entries.into_iter().enumerate() {
        let relative = PathBuf::from(entry.name);
        ensure_parents(root, &relative)?;
        let output = root.join(&relative);
        match entry.kind {
            ArchiveEntryKind::Directory => {
                if !output.exists() {
                    fs::create_dir(&output)?;
                }
            }
            ArchiveEntryKind::Symlink { target } => links.push((relative, target)),
            ArchiveEntryKind::File { executable } => {
                let mut source = archive.by_index(index)?;
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&output)?;
                let actual = io::copy(
                    &mut source.by_ref().take(entry.size_bytes.saturating_add(1)),
                    &mut file,
                )?;
                if actual != entry.size_bytes {
                    return Err(CacheError::Quota);
                }
                fs::set_permissions(
                    output,
                    fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
                )?;
            }
        }
    }
    for (relative, target) in links {
        ensure_parents(root, &relative)?;
        std::os::unix::fs::symlink(target, root.join(relative))?;
    }
    inspect_tree(root, bytes_limit, files_limit, container_root)?;
    Ok(())
}
struct TreeEntry {
    relative: PathBuf,
    directory: bool,
    link: Option<PathBuf>,
    executable: bool,
}
fn inspect_tree(
    root: &Path,
    bytes_limit: u64,
    files_limit: u64,
    container_root: &str,
) -> Result<Vec<TreeEntry>> {
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(CacheError::UnsafeEntry);
    }
    let mut stack = vec![PathBuf::new()];
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    while let Some(parent) = stack.pop() {
        for child in fs::read_dir(root.join(&parent))? {
            let child = child?;
            let relative = parent.join(child.file_name());
            let metadata = fs::symlink_metadata(child.path())?;
            let file_type = metadata.file_type();
            let link = if file_type.is_symlink() {
                let target = fs::read_link(child.path())?;
                Some(target)
            } else if file_type.is_dir() {
                stack.push(relative.clone());
                None
            } else if file_type.is_file() && metadata.nlink() == 1 {
                None
            } else {
                return Err(CacheError::UnsafeEntry);
            };
            bytes = bytes
                .checked_add(if file_type.is_file() || link.is_some() {
                    metadata.len()
                } else {
                    0
                })
                .ok_or(CacheError::Quota)?;
            if bytes > bytes_limit || entries.len() as u64 >= files_limit {
                return Err(CacheError::Quota);
            }
            entries.push(TreeEntry {
                relative,
                directory: file_type.is_dir(),
                link,
                executable: metadata.mode() & 0o111 != 0,
            });
        }
    }
    use attune_common::workload_archive::{validate_tree, ArchiveEntry, ArchiveEntryKind};
    let logical = entries
        .iter()
        .map(|entry| {
            Ok(ArchiveEntry {
                name: entry
                    .relative
                    .to_str()
                    .ok_or(CacheError::UnsafeEntry)?
                    .to_string(),
                size_bytes: if let Some(target) = &entry.link {
                    target.as_os_str().len() as u64
                } else if entry.directory {
                    0
                } else {
                    fs::symlink_metadata(root.join(&entry.relative))?.len()
                },
                kind: if let Some(target) = &entry.link {
                    ArchiveEntryKind::Symlink {
                        target: target.to_str().ok_or(CacheError::UnsafeEntry)?.to_string(),
                    }
                } else if entry.directory {
                    ArchiveEntryKind::Directory
                } else {
                    ArchiveEntryKind::File {
                        executable: entry.executable,
                    }
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    validate_tree(
        &logical,
        &archive_policy(container_root, u64::MAX, bytes_limit, files_limit)?,
    )
    .map_err(|_| CacheError::UnsafeEntry)?;
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(entries)
}
fn seal_tree(root: &Path, container_root: &str) -> Result<()> {
    let entries = inspect_tree(root, u64::MAX, u64::MAX, container_root)?;
    for entry in entries {
        if entry.link.is_none() {
            fs::set_permissions(
                root.join(entry.relative),
                fs::Permissions::from_mode(if entry.directory || entry.executable {
                    0o555
                } else {
                    0o444
                }),
            )?;
        }
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o555))?;
    Ok(())
}
fn make_workload_owned(path: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let name =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| CacheError::UnsafeEntry)?;
    if unsafe { libc::lchown(name.as_ptr(), 1000, 1000) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
fn copy_private(source: &Path, output: &Path) -> Result<()> {
    let entries = inspect_tree(
        source,
        u64::MAX,
        u64::MAX,
        attune_common::workload_launch::PACK_ROOT,
    )?;
    fs::create_dir(output)?;
    for entry in entries {
        let path = output.join(&entry.relative);
        if entry.directory {
            fs::create_dir_all(&path)?;
        } else if let Some(link) = entry.link {
            fs::create_dir_all(path.parent().ok_or(CacheError::UnsafeEntry)?)?;
            std::os::unix::fs::symlink(link, &path)?;
        } else {
            fs::create_dir_all(path.parent().ok_or(CacheError::UnsafeEntry)?)?;
            fs::copy(source.join(&entry.relative), &path)?;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if entry.executable { 0o755 } else { 0o644 }),
            )?;
        }
        make_workload_owned(&path)?;
    }
    make_workload_owned(output)?;
    Ok(())
}

fn ensure_parents(root: &Path, relative: &Path) -> Result<()> {
    let mut path = root.to_path_buf();
    for component in relative
        .parent()
        .ok_or(CacheError::UnsafeEntry)?
        .components()
    {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(CacheError::UnsafeEntry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&path)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::{LaunchInputKind, PinnedLaunchInput};
    use std::io::{Seek, SeekFrom, Write};

    fn zip_body(files: &[(&str, &[u8])], links: &[(&str, &str)]) -> File {
        let mut body = tempfile::tempfile().unwrap();
        {
            let mut zip = ZipWriter::new(&mut body);
            for (name, value) in files {
                zip.start_file(*name, SimpleFileOptions::default()).unwrap();
                zip.write_all(value).unwrap();
            }
            for (name, target) in links {
                zip.add_symlink(*name, *target, SimpleFileOptions::default())
                    .unwrap();
            }
            zip.finish().unwrap();
        }
        body.seek(SeekFrom::Start(0)).unwrap();
        body
    }
    fn extract(body: &mut File, root: &Path) -> Result<()> {
        extract_zip(
            body,
            root,
            1024,
            32,
            attune_common::workload_launch::PACK_ROOT,
        )
    }

    #[test]
    fn malicious_archives_cannot_escape_or_follow_link_parents() {
        for name in [
            "../escape",
            "/absolute",
            "a/../../escape",
            "a\\escape",
            "./escape",
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut body = zip_body(&[(name, b"owned")], &[]);
            assert!(extract(&mut body, root.path()).is_err(), "accepted {name}");
        }
        for links in [
            vec![("link", "../escape")],
            vec![("link", "/etc/passwd")],
            vec![("x", "safe"), ("x/y", "safe")],
            vec![("a", "b"), ("b", "a")],
            vec![("dir/jump", ".."), ("dir/deep", "jump/../outside")],
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut body = zip_body(&[], &links);
            assert!(
                extract(&mut body, root.path()).is_err(),
                "accepted {links:?}"
            );
        }
    }
    #[test]
    fn extraction_bounds_bytes_files_and_rejects_normalized_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let mut body = zip_body(&[("a", b"12345")], &[]);
        assert!(extract_zip(
            &mut body,
            root.path(),
            4,
            10,
            attune_common::workload_launch::PACK_ROOT
        )
        .is_err());
        let root = tempfile::tempdir().unwrap();
        body.seek(SeekFrom::Start(0)).unwrap();
        assert!(extract_zip(
            &mut body,
            root.path(),
            10,
            0,
            attune_common::workload_launch::PACK_ROOT
        )
        .is_err());
        let root = tempfile::tempdir().unwrap();
        let mut body = zip_body(&[("dir/file", b"1"), ("dir//file", b"2")], &[]);
        assert!(extract(&mut body, root.path()).is_err());
    }
    #[test]
    fn stage_tree_rejects_hardlinks_special_files_and_link_cycles() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a"), b"owned").unwrap();
        fs::hard_link(root.path().join("a"), root.path().join("b")).unwrap();
        assert!(inspect_tree(
            root.path(),
            100,
            10,
            attune_common::workload_launch::PACK_ROOT
        )
        .is_err());
        fs::remove_file(root.path().join("b")).unwrap();
        std::os::unix::net::UnixListener::bind(root.path().join("socket")).unwrap();
        assert!(inspect_tree(
            root.path(),
            100,
            10,
            attune_common::workload_launch::PACK_ROOT
        )
        .is_err());
        fs::remove_file(root.path().join("socket")).unwrap();
        std::os::unix::fs::symlink("cycle", root.path().join("cycle")).unwrap();
        assert!(inspect_tree(
            root.path(),
            100,
            10,
            attune_common::workload_launch::PACK_ROOT
        )
        .is_err());
    }
    #[test]
    fn input_pin_is_verified_before_extraction_or_publication() {
        let root = tempfile::tempdir().unwrap();
        let store = CacheStore::open(
            root.path().to_path_buf(),
            CacheLimits {
                compressed_bytes: 4096,
                expanded_bytes: 1024,
                files: 32,
            },
        )
        .unwrap();
        let mut body = zip_body(&[("safe", b"owned")], &[]);
        let pin = PinnedLaunchInput {
            pin_id: 1,
            object_id: 2,
            object_version: "owned-version".into(),
            kind: LaunchInputKind::Pack,
            size_bytes: body.metadata().unwrap().len(),
            sha256: Sha256Digest::of_bytes(b"wrong-body"),
            max_expanded_bytes: 1024,
            max_files: 32,
        };
        assert!(matches!(
            store.install_input(&pin, &mut body),
            Err(CacheError::PinMismatch)
        ));
        assert_eq!(fs::read_dir(root.path().join("inputs")).unwrap().count(), 0);
        assert_eq!(
            fs::read_dir(root.path().join("pending")).unwrap().count(),
            0
        );
    }
    #[test]
    fn safe_internal_links_survive_archive_extraction_without_rewriting() {
        let root = tempfile::tempdir().unwrap();
        let mut body = zip_body(&[("lib/tool", b"owned")], &[("bin/tool", "../lib/tool")]);
        extract(&mut body, root.path()).unwrap();
        assert_eq!(
            fs::read_link(root.path().join("bin/tool")).unwrap(),
            Path::new("../lib/tool")
        );
        assert_eq!(fs::read(root.path().join("bin/tool")).unwrap(), b"owned");
    }
}
