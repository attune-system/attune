//! Trusted-manager-owned, fixed-volume input cache. No engine archive API or caller
//! paths are accepted. Bodies are canonical ZIP and extraction never follows
//! links. All publication is serialized across controller processes sharing the
//! volume; workload containers see only a selected `body` subdirectory.
use attune_common::workload_artifact::ArtifactLimits;
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
    #[error("input or stage exceeds configured bounds or available cache space")]
    Quota,
    #[error("input body does not match admitted immutable pin")]
    PinMismatch,
    #[error("invalid ZIP body")]
    Zip(#[from] zip::result::ZipError),
    #[error("private attempt directory already exists or is not owned")]
    AttemptConflict,
    #[error("file artifact publication remains unacknowledged; private staging retained")]
    ArtifactPending,
    #[error("private file artifact staging is missing; not a selected missing file")]
    LostArtifactStaging,
}
pub type Result<T> = std::result::Result<T, CacheError>;
#[derive(Clone, Copy)]
pub struct CacheLimits {
    pub compressed_bytes: u64,
    pub expanded_bytes: u64,
    pub files: u64,
}

/// The root is configured once by the operator, never selected by a request.
/// The trusted manager owns this private root. Workloads receive only selected
/// subdirectories through read-only mounts, never the root or sibling stages.
pub struct CacheStore {
    root: PathBuf,
    limits: CacheLimits,
    workload_owner: WorkloadOwner,
    artifact_limits: ArtifactLimits,
}
#[derive(Clone, Copy)]
struct WorkloadOwner {
    uid: libc::uid_t,
    gid: libc::gid_t,
}
/// Public random binding sentinel; never an input pin or credential. Directory
/// stays owned until the corresponding probe container removal is acknowledged.
pub(crate) struct BindingSentinel {
    pub nonce: uuid::Uuid,
    pub payload: String,
}
/// Public physical-removal evidence. Only the Docker adapter records this
/// after a received Remove ACK; it carries no invocation or publication grant.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRemoval {
    version: u8,
    policy: Sha256Digest,
    pub resource: attune_common::isolated_process::ResourceIdentity,
    pub outcome: attune_common::workload_launch::LaunchExitOutcome,
    pub payload_exited: bool,
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
        // This is a trusted manager's private cache, not a shared-host UID
        // boundary. Kubernetes transfer does not require the manager's UID to
        // equal the guest's UID. Ownership and non-following checks remain
        // mandatory; Docker stage chown still fails if its owner cannot do it.
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
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        for child in ["inputs", "private", "pending", "manifests"] {
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
        Ok(Self {
            root,
            limits,
            workload_owner: WorkloadOwner {
                uid: 1000,
                gid: 1000,
            },
            artifact_limits: ArtifactLimits::default(),
        })
    }
    pub fn with_artifact_limits(mut self, max_total_bytes: u64, max_entries: u64) -> Result<Self> {
        let limits = ArtifactLimits {
            max_total_bytes,
            max_entries,
        };
        attune_common::workload_artifact::validate_inventory(&[], limits)
            .map_err(|_| CacheError::Quota)?;
        self.artifact_limits = limits;
        Ok(self)
    }
    pub fn artifact_limits(&self) -> ArtifactLimits {
        self.artifact_limits
    }
    pub(crate) fn native_stage_limits(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
    ) -> Result<CacheLimits> {
        use attune_common::workload_launch::{LaunchInputKind, LaunchPurpose};
        let stage = spec.stage.as_ref().ok_or(CacheError::PinMismatch)?;
        if spec.purpose != LaunchPurpose::Setup
            || spec
                .inputs
                .iter()
                .any(|p| p.kind == LaunchInputKind::Runtime)
            || stage.quota_bytes == 0
            || stage.quota_files == 0
        {
            return Err(CacheError::PinMismatch);
        }
        Ok(CacheLimits {
            compressed_bytes: self.limits.compressed_bytes.min(1024 * 1024 * 1024),
            expanded_bytes: self
                .limits
                .expanded_bytes
                .min(stage.quota_bytes)
                .min(4 * 1024 * 1024 * 1024),
            files: self.limits.files.min(stage.quota_files).min(100_000),
        })
    }
    fn native_parent_locked(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
    ) -> Result<PathBuf> {
        let digest =
            crate::process_policy::public_digest(spec).map_err(|_| CacheError::PinMismatch)?;
        let parent = self.root.join(format!("private/{}", spec.intent.simple()));
        let meta = fs::symlink_metadata(&parent).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                CacheError::LostArtifactStaging
            } else {
                CacheError::Io(e)
            }
        })?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err(CacheError::AttemptConflict);
        }
        let mut marker = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join("native-policy"))?;
        let meta = marker.metadata()?;
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.len() != 64
        {
            return Err(CacheError::AttemptConflict);
        }
        let mut body = Vec::new();
        Read::by_ref(&mut marker).take(65).read_to_end(&mut body)?;
        if body != digest.as_str().as_bytes() {
            return Err(CacheError::AttemptConflict);
        }
        Ok(parent)
    }
    fn native_removal_locked(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
        parent: &Path,
    ) -> Result<Option<NativeRemoval>> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join("native-removal.json"))
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o7777 != 0o600
            || meta.len() > 65536
        {
            return Err(CacheError::UnsafeEntry);
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(65537)
            .read_to_end(&mut bytes)?;
        let record: NativeRemoval =
            serde_json::from_slice(&bytes).map_err(|_| CacheError::UnsafeEntry)?;
        let digest =
            crate::process_policy::public_digest(spec).map_err(|_| CacheError::PinMismatch)?;
        let attune_common::isolated_process::ResourceIdentity::Docker {
            engine_id,
            daemon_id,
            container_id,
        } = &record.resource
        else {
            return Err(CacheError::AttemptConflict);
        };
        let binding = attune_common::workload_launch::LaunchEngineBinding {
            engine_id: *engine_id,
            daemon_id: daemon_id.clone(),
            platform: spec.platform,
        };
        if record.version != 1
            || record.policy != digest
            || binding.validate_shape().is_err()
            || Sha256Digest::parse(container_id).is_err()
        {
            return Err(CacheError::AttemptConflict);
        }
        // A retry after uncertain directory fsync must re-establish durability
        // before this historical positive ACK can authorize local byte reads.
        file.sync_all()?;
        File::open(parent)?.sync_all()?;
        Ok(Some(record))
    }
    pub(crate) fn native_removal(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
        resource: &attune_common::isolated_process::ResourceIdentity,
    ) -> Result<Option<NativeRemoval>> {
        let _lock = self.lock()?;
        let parent = match self.native_parent_locked(spec) {
            Ok(parent) => parent,
            // No historical proof, NOT provider/data absence. Byte-read paths
            // independently reject missing retained staging.
            Err(CacheError::LostArtifactStaging) => return Ok(None),
            Err(error) => return Err(error),
        };
        let record = self.native_removal_locked(spec, &parent)?;
        if record
            .as_ref()
            .is_some_and(|record| &record.resource != resource)
        {
            return Err(CacheError::AttemptConflict);
        }
        Ok(record)
    }
    /// Final factual cleanup only. The caller has already committed domain
    /// completion and all selected-file/runtime publication receipts. Retain
    /// the small immutable ownership/removal evidence for idempotent recovery.
    pub(crate) fn cleanup_native_private(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
        resource: &attune_common::isolated_process::ResourceIdentity,
    ) -> Result<()> {
        let _lock = self.lock()?;
        let parent = self.native_parent_locked(spec)?;
        let record = self
            .native_removal_locked(spec, &parent)?
            .ok_or(CacheError::ArtifactPending)?;
        if &record.resource != resource {
            return Err(CacheError::AttemptConflict);
        }
        // Exact fixed children only; never follow a workload-created symlink
        // or remove a published input/runtime cache, sibling intent or spool.
        // An unreadable tree/uncertain deletion returns Err and stays retained.
        for name in ["artifacts", "runtime", "pack", "publication.zip"] {
            let path = parent.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
                Ok(_) => fs::remove_file(path)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        File::open(parent)?.sync_all()?;
        Ok(())
    }
    pub(crate) fn record_native_removal(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
        resource: &attune_common::isolated_process::ResourceIdentity,
        outcome: attune_common::workload_launch::LaunchExitOutcome,
        payload_exited: bool,
    ) -> Result<NativeRemoval> {
        use std::io::Write;
        let _lock = self.lock()?;
        let parent = self.native_parent_locked(spec)?;
        let attune_common::isolated_process::ResourceIdentity::Docker {
            engine_id,
            daemon_id,
            container_id,
        } = resource
        else {
            return Err(CacheError::AttemptConflict);
        };
        let binding = attune_common::workload_launch::LaunchEngineBinding {
            engine_id: *engine_id,
            daemon_id: daemon_id.clone(),
            platform: spec.platform,
        };
        if binding.validate_shape().is_err() || Sha256Digest::parse(container_id).is_err() {
            return Err(CacheError::AttemptConflict);
        }
        let record = NativeRemoval {
            version: 1,
            policy: crate::process_policy::public_digest(spec)
                .map_err(|_| CacheError::PinMismatch)?,
            resource: resource.clone(),
            outcome,
            payload_exited,
        };
        if let Some(prior) = self.native_removal_locked(spec, &parent)? {
            if prior.resource != record.resource
                || prior.payload_exited != record.payload_exited
                || prior.outcome.exit_code != record.outcome.exit_code
                || prior.outcome.oom_killed != record.outcome.oom_killed
            {
                return Err(CacheError::AttemptConflict);
            }
            return Ok(prior);
        }
        let body = serde_json::to_vec(&record).map_err(|_| CacheError::UnsafeEntry)?;
        if body.len() > 65536 {
            return Err(CacheError::Quota);
        }
        let mut pending = tempfile::NamedTempFile::new_in(&parent)?;
        pending.write_all(&body)?;
        pending.as_file().sync_all()?;
        pending
            .persist_noclobber(parent.join("native-removal.json"))
            .map_err(|e| e.error)?;
        File::open(&parent)?.sync_all()?;
        // Use the same strict read path before returning even a new receipt.
        self.native_removal_locked(spec, &parent)?
            .ok_or(CacheError::AttemptConflict)
    }
    /// The concrete Docker adapter has persisted exact Wait+Remove evidence.
    /// This is selected-file I/O, not inventory/publication/cleanup authority.
    pub(crate) fn read_native_artifact(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
        relative: &attune_common::artifact_transport::ValidatedRelativePath,
        remaining: u64,
    ) -> Result<Option<File>> {
        let _lock = self.lock()?;
        let parent = self.native_parent_locked(spec)?;
        let removed = self
            .native_removal_locked(spec, &parent)?
            .ok_or(CacheError::ArtifactPending)?;
        if !removed.payload_exited {
            return Err(CacheError::ArtifactPending);
        }
        crate::artifact_file::read_selected(
            &parent.join("artifacts"),
            relative,
            remaining.min(self.artifact_limits.max_total_bytes),
            self.artifact_limits,
        )
    }
    /// Only the Docker adapter calls this after successful Wait+acknowledged Remove.
    /// Retain one exact body across retry/restart; publication remains caller-owned.
    pub(crate) fn archive_native_stage(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
    ) -> Result<File> {
        let limits = self.native_stage_limits(spec)?;
        let _lock = self.lock()?;
        let parent = self.native_parent_locked(spec)?;
        let removed = self
            .native_removal_locked(spec, &parent)?
            .ok_or(CacheError::ArtifactPending)?;
        if !removed.payload_exited || removed.outcome.exit_code != 0 || removed.outcome.oom_killed {
            return Err(CacheError::ArtifactPending);
        }
        let destination = parent.join("publication.zip");
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&destination)
        {
            Ok(mut file) => {
                let metadata = file.metadata()?;
                if !metadata.is_file()
                    || metadata.nlink() != 1
                    || metadata.uid() != unsafe { libc::geteuid() }
                {
                    return Err(CacheError::UnsafeEntry);
                }
                attune_common::workload_archive::validate_zip(
                    &mut file,
                    &archive_policy(
                        attune_common::workload_launch::RUNTIME_ROOT,
                        limits.compressed_bytes,
                        limits.expanded_bytes,
                        limits.files,
                    )?,
                )
                .map_err(|_| CacheError::UnsafeEntry)?;
                Ok(file)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let pending = tempfile::NamedTempFile::new_in(&parent)?;
                archive_fixed_root(
                    &parent.join("runtime"),
                    pending.as_file().try_clone()?,
                    limits,
                )?;
                let file = pending
                    .persist_noclobber(destination)
                    .map_err(|e| e.error)?;
                File::open(parent)?.sync_all()?;
                Ok(file)
            }
            Err(error) => Err(error.into()),
        }
    }
    /// Filesystem logic tests run as the CI runner, which need not be uid 1000.
    /// This internal test seam is absent from production and request DTOs; the
    /// packaged Engine test retains and proves the fixed uid/gid 1000 contract.
    #[cfg(test)]
    pub(crate) fn for_current_test_user(mut self) -> Self {
        self.workload_owner = WorkloadOwner {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
        };
        self
    }
    #[cfg(test)]
    pub(crate) fn stage_owner(&self, manifest: &LaunchManifest) -> Result<(u32, u32)> {
        let metadata =
            fs::symlink_metadata(self.root.join(crate::engine::stage_subpath(manifest)))?;
        Ok((metadata.uid(), metadata.gid()))
    }
    pub(crate) fn prepare_binding(&self) -> Result<BindingSentinel> {
        let _lock = self.lock()?;
        let nonce = uuid::Uuid::new_v4();
        let parent = self.root.join("binding");
        fs::create_dir_all(&parent)?;
        let directory = parent.join(nonce.simple().to_string());
        fs::create_dir(&directory)?;
        let payload = uuid::Uuid::new_v4().to_string();
        let path = directory.join("sentinel");
        fs::write(&path, payload.as_bytes())?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o444))?;
        Ok(BindingSentinel { nonce, payload })
    }
    /// Called only after definitive non-submission/rejection or actual removal.
    pub(crate) fn cleanup_binding(&self, sentinel: &BindingSentinel) -> Result<()> {
        let _lock = self.lock()?;
        let path = self
            .root
            .join("binding")
            .join(sentinel.nonce.simple().to_string());
        if !fs::symlink_metadata(&path)?.is_dir()
            || fs::read(path.join("sentinel"))? != sentinel.payload.as_bytes()
        {
            return Err(CacheError::PinMismatch);
        }
        fs::remove_dir_all(path)?;
        Ok(())
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
        // Check before extraction or copying a second archive body. This is
        // best-effort admission, not a disk reservation or live write quota.
        self.require_available(
            pin.size_bytes
                .checked_add(pin.max_expanded_bytes)
                .ok_or(CacheError::Quota)?,
        )?;
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
        // Keep the exact authenticated ZIP for native Pod transfer; the payload
        // receives only the extracted body, never this manager-owned parent.
        use std::io::{Seek, SeekFrom};
        body.seek(SeekFrom::Start(0))?;
        let mut archive = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o400)
            .open(pending.path().join("input.zip"))?;
        io::copy(body, &mut archive)?;
        archive.sync_all()?;
        fs::write(pending.path().join("receipt"), receipt(pin)?)?;
        fs::create_dir_all(parent.parent().ok_or(CacheError::UnsafeEntry)?)?;
        // Global cache lock owns validation through publication. No workload
        // sees this temporary directory or the cache parent.
        fs::rename(pending.path(), parent)?;
        Ok(destination)
    }
    /// Exact, bounded, digest-verified ZIP, not a caller-selected host path.
    pub fn open_input_archive(&self, pin: &PinnedLaunchInput) -> Result<File> {
        let _lock = self.lock()?;
        let path = self
            .root
            .join(crate::engine::input_subpath(pin.kind, &pin.sha256));
        let parent = path.parent().ok_or(CacheError::UnsafeEntry)?;
        if fs::read(parent.join("receipt"))? != receipt(pin)? {
            return Err(CacheError::PinMismatch);
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join("input.zip"))?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(CacheError::UnsafeEntry);
        }
        verify_body(&mut file, pin)?;
        Ok(file)
    }
    /// Idempotent local directory initialization for one persisted native intent.
    /// The DB owns create authority; this receipt only detects directory reuse.
    pub fn prepare_native_private(
        &self,
        spec: &attune_common::isolated_process::ProcessSpec,
    ) -> Result<()> {
        let digest =
            crate::process_policy::public_digest(spec).map_err(|_| CacheError::PinMismatch)?;
        let _lock = self.lock()?;
        let path = self.root.join(format!("private/{}", spec.intent.simple()));
        let marker = path.join("native-policy");
        if path.exists() {
            if !fs::symlink_metadata(&path)?.is_dir()
                || fs::read(marker)? != digest.as_str().as_bytes()
            {
                return Err(CacheError::AttemptConflict);
            }
            return Ok(());
        }
        let mut required = self
            .limits
            .compressed_bytes
            .checked_add(self.artifact_limits.max_total_bytes)
            .ok_or(CacheError::Quota)?;
        if spec.stage.is_some() {
            required = required
                .checked_add(self.native_stage_limits(spec)?.expanded_bytes)
                .ok_or(CacheError::Quota)?;
        }
        if spec.pack_access == attune_common::workload_launch::PackAccess::CandidateCopy {
            required = required
                .checked_add(
                    spec.inputs
                        .iter()
                        .find(|pin| {
                            pin.kind == attune_common::workload_launch::LaunchInputKind::Pack
                        })
                        .ok_or(CacheError::PinMismatch)?
                        .max_expanded_bytes,
                )
                .ok_or(CacheError::Quota)?;
        }
        self.require_available(required)?;
        fs::create_dir(&path)?;
        for name in ["artifacts"]
            .into_iter()
            .chain(spec.stage.as_ref().map(|_| "runtime"))
        {
            let directory = path.join(name);
            fs::create_dir(&directory)?;
            make_workload_owned(&directory, self.workload_owner)?;
        }
        if spec.pack_access == attune_common::workload_launch::PackAccess::CandidateCopy {
            let pin = spec
                .inputs
                .iter()
                .find(|p| p.kind == attune_common::workload_launch::LaunchInputKind::Pack)
                .ok_or(CacheError::PinMismatch)?;
            copy_private(
                &self
                    .root
                    .join(crate::engine::input_subpath(pin.kind, &pin.sha256)),
                &path.join("pack"),
                self.workload_owner,
            )?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(marker)?;
        use io::Write;
        file.write_all(digest.as_str().as_bytes())?;
        file.sync_all()?;
        File::open(&path)?.sync_all()?;
        File::open(self.root.join("private"))?.sync_all()?;
        Ok(())
    }
    /// Each nonce is single-use. Restarts must reconcile the existing launch,
    /// not reuse or reset a writable stage. No recursive delete by caller path.
    pub fn prepare_private(&self, manifest: &LaunchManifest) -> Result<()> {
        let _lock = self.lock()?;
        // Durable public recovery evidence precedes every Engine create. A
        // Stopped/artifact-only claim intentionally carries no private context.
        self.record_manifest_locked(manifest)?;
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
            make_workload_owned(&stage, self.workload_owner)?;
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
            copy_private(&source, &output, self.workload_owner)?;
        }
        Ok(())
    }
    fn manifest_path(
        &self,
        request: &attune_common::workload_launch::LaunchRequest,
    ) -> Result<PathBuf> {
        if request.launch_id <= 0 {
            return Err(CacheError::PinMismatch);
        }
        Ok(self.root.join("manifests").join(format!(
            "{}-{}.json",
            request.launch_id,
            request.admitted_digest.as_str()
        )))
    }
    fn record_manifest_locked(&self, manifest: &LaunchManifest) -> Result<()> {
        manifest
            .validate_shape()
            .map_err(|_| CacheError::PinMismatch)?;
        let request = attune_common::workload_launch::LaunchRequest {
            launch_id: manifest.launch_id,
            admitted_digest: manifest
                .admitted_digest()
                .map_err(|_| CacheError::PinMismatch)?,
        };
        let destination = self.manifest_path(&request)?;
        let bytes = serde_json::to_vec(manifest).map_err(|_| CacheError::PinMismatch)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(CacheError::Quota);
        }
        match self.read_manifest_locked(&request) {
            Ok(previous) => {
                if serde_json::to_vec(&previous).map_err(|_| CacheError::PinMismatch)? != bytes {
                    return Err(CacheError::PinMismatch);
                }
            }
            Err(CacheError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                let mut pending = tempfile::NamedTempFile::new_in(self.root.join("manifests"))?;
                use std::io::Write;
                pending.write_all(&bytes)?;
                pending.as_file().sync_all()?;
                pending
                    .persist_noclobber(destination)
                    .map_err(|error| error.error)?;
                File::open(self.root.join("manifests"))?.sync_all()?;
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }
    /// Local evidence only. The caller first authenticates the exact CP request
    /// and Stopped/artifact claim; this index never authorizes Run or upload.
    /// Missing/corrupt evidence remains an incomplete recovery obligation.
    pub fn retained_manifest(
        &self,
        request: &attune_common::workload_launch::LaunchRequest,
    ) -> Result<LaunchManifest> {
        let _lock = self.lock()?;
        self.read_manifest_locked(request)
    }
    fn read_manifest_locked(
        &self,
        request: &attune_common::workload_launch::LaunchRequest,
    ) -> Result<LaunchManifest> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.manifest_path(request)?)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.len() > 4 * 1024 * 1024
        {
            return Err(CacheError::UnsafeEntry);
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(CacheError::Quota);
        }
        let manifest: LaunchManifest =
            serde_json::from_slice(&bytes).map_err(|_| CacheError::PinMismatch)?;
        manifest
            .validate_shape()
            .map_err(|_| CacheError::PinMismatch)?;
        if manifest.launch_id != request.launch_id
            || manifest
                .admitted_digest()
                .map_err(|_| CacheError::PinMismatch)?
                != request.admitted_digest
            || serde_json::to_vec(&manifest).map_err(|_| CacheError::PinMismatch)? != bytes
        {
            return Err(CacheError::PinMismatch);
        }
        Ok(manifest)
    }
    /// File-backed artifact transport is separate from runtime publication.
    /// The fixed private subpath is not activated by the Engine builder until
    /// the CP artifact grant/state contract is integrated; never discard it as
    /// ordinary scratch or infer a publication ACK from process exit.
    pub fn prepare_artifact_staging(&self, manifest: &LaunchManifest) -> Result<()> {
        use attune_common::workload_launch::LaunchPurpose;
        if !matches!(
            manifest.purpose,
            LaunchPurpose::Action | LaunchPurpose::Sensor | LaunchPurpose::CandidateTest
        ) {
            return Err(CacheError::PinMismatch);
        }
        let _lock = self.lock()?;
        let parent = self
            .root
            .join(format!("private/{}", manifest.engine_name_nonce.simple()));
        let metadata = fs::symlink_metadata(&parent)?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(CacheError::UnsafeEntry);
        }
        let directory = parent.join("artifacts");
        fs::create_dir(&directory)?;
        make_workload_owned(&directory, self.workload_owner)?;
        let marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join("artifact.pending"))?;
        marker.sync_all()?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
    /// The relative path must come from an exact CP-issued file-version grant,
    /// never a frontend path/selector. Caller must hold retained artifact
    /// publication authority; Stopped alone is not file-upload authorization.
    pub fn open_settled_artifact(
        &self,
        manifest: &LaunchManifest,
        status: &attune_common::workload_launch::LaunchStatus,
        relative: &str,
    ) -> Result<File> {
        use attune_common::artifact_transport::ValidatedRelativePath;
        use attune_common::workload_launch::{LaunchPurpose, LaunchState};
        if !matches!(
            manifest.purpose,
            LaunchPurpose::Action | LaunchPurpose::Sensor | LaunchPurpose::CandidateTest
        ) || status.validate_shape().is_err()
            || status.state != LaunchState::Stopped
            || status.request.launch_id != manifest.launch_id
            || status.request.admitted_digest
                != manifest
                    .admitted_digest()
                    .map_err(|_| CacheError::PinMismatch)?
        {
            return Err(CacheError::PinMismatch);
        }
        ValidatedRelativePath::new(relative).map_err(|_| CacheError::UnsafeEntry)?;
        let _lock = self.lock()?;
        let root = self.root.join(format!(
            "private/{}/artifacts",
            manifest.engine_name_nonce.simple()
        ));
        self.validate_artifact_tree(&root, self.artifact_limits)?;
        let mut path = root;
        let components = Path::new(relative).components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            if !matches!(component, std::path::Component::Normal(_)) {
                return Err(CacheError::UnsafeEntry);
            }
            path.push(component.as_os_str());
            let metadata = fs::symlink_metadata(&path)?;
            if index + 1 < components.len() && !metadata.is_dir() {
                return Err(CacheError::UnsafeEntry);
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.len() > self.artifact_limits.max_total_bytes
        {
            return Err(CacheError::UnsafeEntry);
        }
        Ok(file)
    }
    /// Validate the retained root before interpreting a selected file's
    /// NotFound as Missing. A lost root/marker is LostStaging, not an empty
    /// inventory. This is called only after authenticated removal settlement.
    pub fn validate_settled_artifact_staging(
        &self,
        manifest: &LaunchManifest,
        status: &attune_common::workload_launch::LaunchStatus,
        limits: ArtifactLimits,
    ) -> Result<()> {
        use attune_common::workload_launch::{LaunchPurpose, LaunchState};
        if !matches!(
            manifest.purpose,
            LaunchPurpose::Action | LaunchPurpose::Sensor | LaunchPurpose::CandidateTest
        ) || status.validate_shape().is_err()
            || status.state != LaunchState::Stopped
            || status.request.launch_id != manifest.launch_id
            || status.request.admitted_digest
                != manifest
                    .admitted_digest()
                    .map_err(|_| CacheError::PinMismatch)?
            || status.resource.as_ref().is_some_and(|resource| {
                resource.engine_id != manifest.engine_id
                    || resource.engine_name_nonce != manifest.engine_name_nonce
                    || resource.admitted_digest != status.request.admitted_digest
            })
        {
            return Err(CacheError::PinMismatch);
        }
        attune_common::workload_artifact::validate_inventory(&[], limits)
            .map_err(|_| CacheError::Quota)?;
        let _lock = self.lock()?;
        let parent = self
            .root
            .join(format!("private/{}", manifest.engine_name_nonce.simple()));
        let parent_metadata = fs::symlink_metadata(&parent)?;
        let marker = fs::symlink_metadata(parent.join("artifact.pending"))?;
        if !parent_metadata.is_dir()
            || parent_metadata.uid() != unsafe { libc::geteuid() }
            || !marker.is_file()
            || marker.nlink() != 1
            || marker.uid() != unsafe { libc::geteuid() }
            || marker.mode() & 0o077 != 0
            || marker.len() != 0
        {
            return Err(CacheError::UnsafeEntry);
        }
        self.validate_artifact_tree(
            &parent.join("artifacts"),
            ArtifactLimits {
                max_total_bytes: limits
                    .max_total_bytes
                    .min(self.artifact_limits.max_total_bytes),
                max_entries: limits.max_entries.min(self.artifact_limits.max_entries),
            },
        )
    }
    fn validate_artifact_tree(&self, root: &Path, limits: ArtifactLimits) -> Result<()> {
        crate::artifact_file::validate_tree(root, limits)
    }
    /// Only pass a receipt decoded from the authenticated CP artifact protocol,
    /// never a frontend body or a locally synthesized terminal state. Persist
    /// settlement before deleting the owned stage so crash/lost-ACK recovery
    /// cannot recreate publication authority or infer an unknown PUT absent.
    pub fn cleanup_artifact_staging(
        &self,
        manifest: &LaunchManifest,
        status: &attune_common::workload_launch::LaunchStatus,
        receipt: &attune_common::workload_artifact::ArtifactPublicationReceipt,
    ) -> Result<()> {
        use attune_common::workload_artifact::ArtifactPublicationState;
        use attune_common::workload_launch::{LaunchPurpose, LaunchState};
        if !matches!(
            manifest.purpose,
            LaunchPurpose::Action | LaunchPurpose::Sensor | LaunchPurpose::CandidateTest
        ) || status.validate_shape().is_err()
            || status.state != LaunchState::Stopped
            || status.request.launch_id != manifest.launch_id
            || status.request.admitted_digest
                != manifest
                    .admitted_digest()
                    .map_err(|_| CacheError::PinMismatch)?
            || receipt.request != status.request
            || receipt.publication_id <= 0
            || receipt.cleanup_receipt_id <= 0
            || !matches!(
                receipt.state,
                ArtifactPublicationState::Complete | ArtifactPublicationState::Incomplete
            )
        {
            return Err(CacheError::PinMismatch);
        }
        let _lock = self.lock()?;
        let evidence = self.root.join("artifacts-settled");
        fs::create_dir_all(&evidence)?;
        let name = manifest.engine_name_nonce.simple().to_string();
        let destination = evidence.join(&name);
        let bytes = serde_json::to_vec(receipt).map_err(|_| CacheError::PinMismatch)?;
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&destination)
        {
            Ok(mut file) => {
                let metadata = file.metadata()?;
                if !metadata.is_file()
                    || metadata.nlink() != 1
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.len() > 8192
                {
                    return Err(CacheError::UnsafeEntry);
                }
                let mut previous = Vec::new();
                file.read_to_end(&mut previous)?;
                if previous != bytes {
                    return Err(CacheError::PinMismatch);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut pending = tempfile::NamedTempFile::new_in(&evidence)?;
                use std::io::Write;
                pending.write_all(&bytes)?;
                pending.as_file().sync_all()?;
                pending
                    .persist_noclobber(&destination)
                    .map_err(|error| error.error)?;
                File::open(&evidence)?.sync_all()?;
            }
            Err(error) => return Err(error.into()),
        }
        let directory = self.root.join("private").join(name);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() } => {
                fs::remove_dir_all(&directory)?
            }
            Ok(_) => return Err(CacheError::UnsafeEntry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        File::open(self.root.join("private"))?.sync_all()?;
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
        self.require_available(
            required
                .checked_add(minimum_free_bytes)
                .ok_or(CacheError::Quota)?,
        )
    }
    /// Caller holds the cache lock. Concurrent workloads/other writers can
    /// still exhaust disk after this observation; owning loops cap concurrency
    /// and retain failed effects for acknowledged cleanup, never assume quota.
    fn require_available(&self, required: u64) -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(self.root.as_os_str().as_bytes())
            .map_err(|_| CacheError::UnsafeEntry)?;
        let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(name.as_ptr(), status.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let status = unsafe { status.assume_init() };
        let available = (status.f_bavail as u128) * (status.f_frsize as u128);
        if available < required as u128 {
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
        match fs::symlink_metadata(path.join("artifact.pending")) {
            Ok(_) => return Err(CacheError::ArtifactPending),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
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
        let parent = path.parent().ok_or(CacheError::UnsafeEntry)?;
        let destination = parent.join("publication.zip");
        match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.nlink() != 1
                    || metadata.uid() != unsafe { libc::geteuid() }
                {
                    return Err(CacheError::UnsafeEntry);
                }
                let mut file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&destination)?;
                let policy = archive_policy(
                    attune_common::workload_launch::RUNTIME_ROOT,
                    self.limits.compressed_bytes,
                    stage.quota_bytes.min(self.limits.expanded_bytes),
                    stage.quota_files.min(self.limits.files),
                )?;
                attune_common::workload_archive::validate_zip(&mut file, &policy)
                    .map_err(|_| CacheError::UnsafeEntry)?;
                return Ok(file);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let entries = inspect_tree(
            &path,
            stage.quota_bytes.min(self.limits.expanded_bytes),
            stage.quota_files.min(self.limits.files),
            attune_common::workload_launch::RUNTIME_ROOT,
        )?;
        let pending = tempfile::NamedTempFile::new_in(parent)?;
        let mut output = BoundedFile {
            file: pending.as_file().try_clone()?,
            limit: self.limits.compressed_bytes,
        };
        {
            let mut zip = ZipWriter::new(&mut output);
            for entry in entries {
                let name = entry.relative.to_str().ok_or(CacheError::UnsafeEntry)?;
                let options = SimpleFileOptions::default()
                    .last_modified_time(zip::DateTime::default())
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
        output.file.sync_all()?;
        output.file.seek(SeekFrom::Start(0))?;
        // The exact body survives controller restart and a lost CP response.
        // Never regenerate bytes after an output reservation was made.
        let file = pending
            .persist_noclobber(&destination)
            .map_err(|error| CacheError::Io(error.error))?;
        File::open(parent)?.sync_all()?;
        Ok(file)
    }
}
/// Fixed-container transfer helper only. The adapter has already selected the
/// exact stopped resource/root. No caller host path reaches this function.
pub(crate) fn seed_fixed_root(pin: &PinnedLaunchInput, body: &mut File, root: &Path) -> Result<()> {
    verify_body(body, pin)?;
    if !fs::symlink_metadata(root)?.is_dir() || fs::read_dir(root)?.next().is_some() {
        return Err(CacheError::AttemptConflict);
    }
    extract_zip(
        body,
        root,
        pin.max_expanded_bytes,
        pin.max_files,
        pin.kind.container_root(),
    )
}
pub(crate) fn archive_fixed_root(root: &Path, output: File, limits: CacheLimits) -> Result<File> {
    let entries = inspect_tree(
        root,
        limits.expanded_bytes,
        limits.files,
        attune_common::workload_launch::RUNTIME_ROOT,
    )?;
    let mut output = BoundedFile {
        file: output,
        limit: limits.compressed_bytes,
    };
    {
        let mut zip = ZipWriter::new(&mut output);
        for entry in entries {
            let name = entry.relative.to_str().ok_or(CacheError::UnsafeEntry)?;
            let options = SimpleFileOptions::default()
                .last_modified_time(zip::DateTime::default())
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
                    .open(root.join(&entry.relative))?;
                io::copy(&mut file, &mut zip)?;
            }
        }
        zip.finish()?;
    }
    use io::{Seek, SeekFrom};
    output.file.sync_all()?;
    output.file.seek(SeekFrom::Start(0))?;
    let policy = archive_policy(
        attune_common::workload_launch::RUNTIME_ROOT,
        limits.compressed_bytes,
        limits.expanded_bytes,
        limits.files,
    )?;
    attune_common::workload_archive::validate_zip(&mut output.file, &policy)
        .map_err(|_| CacheError::UnsafeEntry)?;
    Ok(output.file)
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
fn make_workload_owned(path: &Path, owner: WorkloadOwner) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let name =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| CacheError::UnsafeEntry)?;
    if unsafe { libc::lchown(name.as_ptr(), owner.uid, owner.gid) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
fn copy_private(source: &Path, output: &Path, owner: WorkloadOwner) -> Result<()> {
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
        make_workload_owned(&path, owner)?;
    }
    make_workload_owned(output, owner)?;
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

    fn native_setup() -> attune_common::isolated_process::ProcessSpec {
        use attune_common::workload_launch::*;
        let old =
            crate::test_support::spec(format!("fixture.invalid/runtime@sha256:{}", "a".repeat(64)));
        attune_common::isolated_process::ProcessSpec {
            intent: uuid::Uuid::new_v4(),
            purpose: LaunchPurpose::Setup,
            target: LaunchTarget::Setup {
                admission: 1,
                attempt: uuid::Uuid::new_v4(),
            },
            image: old.manifest.image,
            platform: LaunchPlatform::LinuxAmd64,
            inputs: vec![],
            pack_access: PackAccess::ReadOnly,
            stage: Some(PrivateStage {
                cache_key: "owned-test-stage".into(),
                quota_bytes: 65536,
                quota_files: 32,
            }),
            timeout_ms: Some(1000),
            invocation: old.invocation,
        }
    }
    fn native_removed(
        store: &CacheStore,
        spec: &attune_common::isolated_process::ProcessSpec,
    ) -> attune_common::isolated_process::ResourceIdentity {
        let resource = attune_common::isolated_process::ResourceIdentity::Docker {
            engine_id: 1,
            daemon_id: "owned-daemon".into(),
            container_id: "a".repeat(64),
        };
        // Pure filesystem fixture: models an already received positive ACK,
        // never claims that a real provider was contacted by this unit test.
        store
            .record_native_removal(
                spec,
                &resource,
                attune_common::workload_launch::LaunchExitOutcome {
                    exit_code: 0,
                    oom_killed: false,
                },
                true,
            )
            .unwrap();
        resource
    }
    #[test]
    fn native_cleanup_requires_exact_removal_and_retains_only_historical_evidence() {
        use attune_common::artifact_transport::ValidatedRelativePath;
        let root = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            compressed_bytes: 65536,
            expanded_bytes: 65536,
            files: 32,
        };
        let store = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        let spec = native_setup();
        store.prepare_native_private(&spec).unwrap();
        let parent = root
            .path()
            .join(format!("private/{}", spec.intent.simple()));
        fs::write(parent.join("runtime/tool"), b"owned runtime").unwrap();
        fs::write(parent.join("artifacts/file"), b"owned artifact").unwrap();
        let resource = attune_common::isolated_process::ResourceIdentity::Docker {
            engine_id: 1,
            daemon_id: "owned-daemon".into(),
            container_id: "a".repeat(64),
        };
        assert!(matches!(
            store.cleanup_native_private(&spec, &resource),
            Err(CacheError::ArtifactPending)
        ));
        assert!(parent.join("runtime/tool").exists());
        let resource = native_removed(&store, &spec);
        // Archive validation/reads must keep the stage until the owning caller
        // explicitly takes the final cleanup path after publication receipts.
        drop(store.archive_native_stage(&spec).unwrap());
        assert!(parent.join("runtime/tool").exists());
        assert!(parent.join("publication.zip").exists());
        let mut wrong = resource.clone();
        if let attune_common::isolated_process::ResourceIdentity::Docker { container_id, .. } =
            &mut wrong
        {
            *container_id = "b".repeat(64);
        }
        assert!(store.cleanup_native_private(&spec, &wrong).is_err());
        assert!(parent.join("artifacts/file").exists());
        store.cleanup_native_private(&spec, &resource).unwrap();
        for name in ["runtime", "artifacts", "publication.zip"] {
            assert!(!parent.join(name).exists());
        }
        assert!(parent.join("native-policy").is_file());
        assert!(parent.join("native-removal.json").is_file());
        drop(store);
        let store = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        store.cleanup_native_private(&spec, &resource).unwrap();
        assert!(store.native_removal(&spec, &resource).unwrap().is_some());
        assert!(store.archive_native_stage(&spec).is_err());
        let mut action = native_setup();
        action.purpose = attune_common::workload_launch::LaunchPurpose::Action;
        action.stage = None;
        store.prepare_native_private(&action).unwrap();
        let action_resource = native_removed(&store, &action);
        store
            .cleanup_native_private(&action, &action_resource)
            .unwrap();
        assert!(matches!(
            store.read_native_artifact(&action, &ValidatedRelativePath::new("missing").unwrap(), 1),
            Err(CacheError::LostArtifactStaging)
        ));
    }
    #[test]
    fn native_cleanup_does_not_follow_links_or_touch_sibling_and_published_trees() {
        let root = tempfile::tempdir().unwrap();
        let store = CacheStore::open(
            root.path().into(),
            CacheLimits {
                compressed_bytes: 65536,
                expanded_bytes: 65536,
                files: 32,
            },
        )
        .unwrap()
        .for_current_test_user();
        let spec = native_setup();
        let sibling = native_setup();
        store.prepare_native_private(&spec).unwrap();
        store.prepare_native_private(&sibling).unwrap();
        let parent = root
            .path()
            .join(format!("private/{}", spec.intent.simple()));
        let sibling_parent = root
            .path()
            .join(format!("private/{}", sibling.intent.simple()));
        fs::write(sibling_parent.join("artifacts/retained"), b"sibling").unwrap();
        let published = root.path().join("inputs/owned-published");
        fs::create_dir(&published).unwrap();
        fs::write(published.join("retained"), b"published").unwrap();
        std::os::unix::fs::symlink(&sibling_parent, parent.join("artifacts/escape")).unwrap();
        fs::remove_dir(parent.join("runtime")).unwrap();
        std::os::unix::fs::symlink(&published, parent.join("runtime")).unwrap();
        let resource = native_removed(&store, &spec);
        store.cleanup_native_private(&spec, &resource).unwrap();
        assert_eq!(
            fs::read(sibling_parent.join("artifacts/retained")).unwrap(),
            b"sibling"
        );
        assert_eq!(fs::read(published.join("retained")).unwrap(), b"published");
        assert!(fs::symlink_metadata(parent.join("runtime")).is_err());
        assert!(fs::symlink_metadata(parent.join("artifacts")).is_err());
    }
    #[test]
    fn native_removal_receipt_is_owned_digest_bound_and_survives_restart_without_invented_absence()
    {
        let root = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            compressed_bytes: 65536,
            expanded_bytes: 65536,
            files: 32,
        };
        let spec = native_setup();
        let store = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        store.prepare_native_private(&spec).unwrap();
        assert!(matches!(
            store.archive_native_stage(&spec),
            Err(CacheError::ArtifactPending)
        ));
        let resource = native_removed(&store, &spec);
        drop(store);
        let store = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        assert!(
            store
                .native_removal(&spec, &resource)
                .unwrap()
                .unwrap()
                .payload_exited
        );
        let mut other = resource.clone();
        if let attune_common::isolated_process::ResourceIdentity::Docker { container_id, .. } =
            &mut other
        {
            *container_id = "b".repeat(64);
        }
        assert!(store.native_removal(&spec, &other).is_err());
        let path = root.path().join(format!(
            "private/{}/native-removal.json",
            spec.intent.simple()
        ));
        let body = fs::read(&path).unwrap();
        fs::hard_link(&path, root.path().join("receipt-alias")).unwrap();
        assert!(store.native_removal(&spec, &resource).is_err());
        fs::remove_file(root.path().join("receipt-alias")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.native_removal(&spec, &resource).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut record: serde_json::Value = serde_json::from_slice(&body).unwrap();
        record["policy"] = serde_json::json!("b".repeat(64));
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(store.native_removal(&spec, &resource).is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.path().join("not-a-receipt"), &path).unwrap();
        assert!(store.native_removal(&spec, &resource).is_err());
        fs::remove_file(&path).unwrap();
        assert!(store.native_removal(&spec, &resource).unwrap().is_none());
        assert!(matches!(
            store.archive_native_stage(&spec),
            Err(CacheError::ArtifactPending)
        ));
    }
    #[test]
    fn native_stage_requires_exited_success_not_never_started_failed_or_oom_removal() {
        for (exited, code, oom, allowed) in [
            (false, 0, false, false),
            (true, 1, false, false),
            (true, 0, true, false),
            (true, 0, false, true),
        ] {
            let root = tempfile::tempdir().unwrap();
            let store = CacheStore::open(
                root.path().into(),
                CacheLimits {
                    compressed_bytes: 65536,
                    expanded_bytes: 65536,
                    files: 32,
                },
            )
            .unwrap()
            .for_current_test_user();
            let spec = native_setup();
            store.prepare_native_private(&spec).unwrap();
            fs::write(
                root.path()
                    .join(format!("private/{}/runtime/owned", spec.intent.simple())),
                b"owned",
            )
            .unwrap();
            let resource = attune_common::isolated_process::ResourceIdentity::Docker {
                engine_id: 1,
                daemon_id: "owned-daemon".into(),
                container_id: "a".repeat(64),
            };
            store
                .record_native_removal(
                    &spec,
                    &resource,
                    attune_common::workload_launch::LaunchExitOutcome {
                        exit_code: code,
                        oom_killed: oom,
                    },
                    exited,
                )
                .unwrap();
            assert_eq!(store.archive_native_stage(&spec).is_ok(), allowed);
        }
    }
    #[test]
    fn native_selected_artifact_checks_retained_policy_and_does_not_infer_lost_root_absence() {
        use attune_common::artifact_transport::ValidatedRelativePath;
        let root = tempfile::tempdir().unwrap();
        let store = CacheStore::open(
            root.path().into(),
            CacheLimits {
                compressed_bytes: 65536,
                expanded_bytes: 65536,
                files: 32,
            },
        )
        .unwrap()
        .for_current_test_user();
        let mut spec = native_setup();
        spec.purpose = attune_common::workload_launch::LaunchPurpose::Action;
        spec.stage = None;
        let selected = ValidatedRelativePath::new("empty").unwrap();
        assert!(matches!(
            store.read_native_artifact(&spec, &selected, 0),
            Err(CacheError::LostArtifactStaging)
        ));
        store.prepare_native_private(&spec).unwrap();
        assert!(matches!(
            store.read_native_artifact(&spec, &selected, 0),
            Err(CacheError::ArtifactPending)
        ));
        native_removed(&store, &spec);
        assert!(store
            .read_native_artifact(&spec, &selected, 0)
            .unwrap()
            .is_none());
        let artifacts = root
            .path()
            .join(format!("private/{}/artifacts", spec.intent.simple()));
        fs::write(artifacts.join("empty"), b"").unwrap();
        assert_eq!(
            store
                .read_native_artifact(&spec, &selected, 0)
                .unwrap()
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            0
        );
        let mut changed = spec.clone();
        changed.timeout_ms = Some(2000);
        assert!(store.read_native_artifact(&changed, &selected, 0).is_err());
        fs::remove_file(artifacts.join("empty")).unwrap();
        fs::remove_dir(&artifacts).unwrap();
        assert!(matches!(
            store.read_native_artifact(&spec, &selected, 0),
            Err(CacheError::LostArtifactStaging)
        ));
    }
    #[test]
    fn native_stage_archive_keeps_exact_body_across_restart_and_checks_ownership() {
        let root = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            compressed_bytes: 65536,
            expanded_bytes: 65536,
            files: 32,
        };
        let spec = native_setup();
        let store = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        store.prepare_native_private(&spec).unwrap();
        let runtime = root
            .path()
            .join(format!("private/{}/runtime", spec.intent.simple()));
        fs::create_dir(runtime.join("bin")).unwrap();
        fs::write(
            runtime.join("bin/tool"),
            b"#!/opt/attune/runtime/bin/python\n",
        )
        .unwrap();
        fs::set_permissions(runtime.join("bin/tool"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(
            "/opt/attune/runtime/bin/tool",
            runtime.join("absolute-link"),
        )
        .unwrap();
        assert!(matches!(
            store.archive_native_stage(&spec),
            Err(CacheError::ArtifactPending)
        ));
        native_removed(&store, &spec);
        let mut first = Vec::new();
        store
            .archive_native_stage(&spec)
            .unwrap()
            .read_to_end(&mut first)
            .unwrap();
        assert!(!first.is_empty());
        // Changes cannot regenerate bytes after the first body was retained.
        fs::write(runtime.join("bin/tool"), b"changed after first archive").unwrap();
        drop(store);
        let store = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        let mut replay = Vec::new();
        store
            .archive_native_stage(&spec)
            .unwrap()
            .read_to_end(&mut replay)
            .unwrap();
        assert_eq!(first, replay);
        let mut changed = spec.clone();
        changed.stage.as_mut().unwrap().cache_key = "another-binding".into();
        assert!(store.archive_native_stage(&changed).is_err());
        changed = spec.clone();
        changed.purpose = attune_common::workload_launch::LaunchPurpose::Action;
        assert!(store.archive_native_stage(&changed).is_err());
        let archive = root
            .path()
            .join(format!("private/{}/publication.zip", spec.intent.simple()));
        fs::remove_file(&archive).unwrap();
        std::os::unix::fs::symlink(runtime.join("bin/tool"), &archive).unwrap();
        assert!(store.archive_native_stage(&spec).is_err());
    }
    #[test]
    fn native_stage_tree_rejects_escaping_cycles_hardlinks_and_special_files() {
        for case in 0..5 {
            let root = tempfile::tempdir().unwrap();
            let store = CacheStore::open(
                root.path().into(),
                CacheLimits {
                    compressed_bytes: 65536,
                    expanded_bytes: 65536,
                    files: 32,
                },
            )
            .unwrap()
            .for_current_test_user();
            let spec = native_setup();
            store.prepare_native_private(&spec).unwrap();
            let runtime = root
                .path()
                .join(format!("private/{}/runtime", spec.intent.simple()));
            match case {
                0 => std::os::unix::fs::symlink("/etc/passwd", runtime.join("escape")).unwrap(),
                1 => {
                    std::os::unix::fs::symlink("second", runtime.join("first")).unwrap();
                    std::os::unix::fs::symlink("first", runtime.join("second")).unwrap();
                }
                2 => {
                    fs::write(runtime.join("first"), b"owned").unwrap();
                    fs::hard_link(runtime.join("first"), runtime.join("second")).unwrap();
                }
                3 => {
                    let _socket =
                        std::os::unix::net::UnixListener::bind(runtime.join("socket")).unwrap();
                }
                4 => fs::write(runtime.join("oversized"), vec![0; 65537]).unwrap(),
                _ => unreachable!(),
            }
            native_removed(&store, &spec);
            assert!(store.archive_native_stage(&spec).is_err(), "case {case}");
            assert!(!runtime.parent().unwrap().join("publication.zip").exists());
        }
    }

    #[test]
    fn public_manifest_restart_evidence_is_digest_bound_private_and_never_an_invocation() {
        use attune_common::workload_launch::LaunchRequest;
        let root = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            compressed_bytes: 65536,
            expanded_bytes: 65536,
            files: 32,
        };
        let store = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        let mut spec = crate::test_support::spec(format!("sha256:{}", "a".repeat(64)));
        spec.invocation
            .env
            .insert("SELECTED_SECRET".into(), "owned-private-input".into());
        spec.invocation.argv.push("owned-private-argument".into());
        let request = LaunchRequest {
            launch_id: spec.manifest.launch_id,
            admitted_digest: spec.manifest.admitted_digest().unwrap(),
        };
        store.prepare_private(&spec.manifest).unwrap();
        let path = store.manifest_path(&request).unwrap();
        let saved = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&saved).contains("owned-private"));
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        drop(store);
        let restarted = CacheStore::open(root.path().into(), limits)
            .unwrap()
            .for_current_test_user();
        assert_eq!(
            restarted
                .retained_manifest(&request)
                .unwrap()
                .engine_name_nonce,
            spec.manifest.engine_name_nonce
        );
        let sibling = LaunchRequest {
            launch_id: request.launch_id + 1,
            ..request.clone()
        };
        assert!(restarted.retained_manifest(&sibling).is_err());
        let alias = root.path().join("linked-manifest");
        fs::hard_link(&path, &alias).unwrap();
        assert!(restarted.retained_manifest(&request).is_err());
        fs::remove_file(&alias).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&alias, &saved).unwrap();
        std::os::unix::fs::symlink(&alias, &path).unwrap();
        assert!(restarted.retained_manifest(&request).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, &saved).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut poisoned = spec.manifest.clone();
        poisoned.engine_name_nonce = uuid::Uuid::new_v4();
        fs::write(&path, serde_json::to_vec(&poisoned).unwrap()).unwrap();
        assert!(restarted.retained_manifest(&request).is_err());
        assert_eq!(fs::read(alias).unwrap(), saved);
    }
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
    fn cache_space_admission_precedes_new_input_and_native_private_allocation() {
        let root = tempfile::tempdir().unwrap();
        // An impossible declared budget exercises actual statvfs admission
        // without filling a disk, changing mounts or relying on free-space races.
        let impossible = u64::MAX / 4;
        let store = CacheStore::open(
            root.path().into(),
            CacheLimits {
                compressed_bytes: impossible,
                expanded_bytes: impossible,
                files: 32,
            },
        )
        .unwrap()
        .for_current_test_user();
        let mut body = zip_body(&[("safe", b"owned")], &[]);
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).unwrap();
        let mut pin = PinnedLaunchInput {
            pin_id: 1,
            object_id: 2,
            object_version: "owned-space-admission".into(),
            kind: LaunchInputKind::Pack,
            size_bytes: bytes.len() as u64,
            sha256: Sha256Digest::of_bytes(&bytes),
            max_expanded_bytes: impossible,
            max_files: 32,
        };
        assert!(matches!(
            store.install_input(&pin, &mut body),
            Err(CacheError::Quota)
        ));
        assert_eq!(fs::read_dir(root.path().join("inputs")).unwrap().count(), 0);
        assert_eq!(
            fs::read_dir(root.path().join("pending")).unwrap().count(),
            0
        );
        pin.max_expanded_bytes = 1024;
        assert!(store
            .install_input(&pin, &mut body)
            .unwrap()
            .join("safe")
            .is_file());
        let spec = native_setup();
        assert!(matches!(
            store.prepare_native_private(&spec),
            Err(CacheError::Quota)
        ));
        assert_eq!(
            fs::read_dir(root.path().join("private")).unwrap().count(),
            0
        );
    }
    #[test]
    fn cached_zip_rejects_changed_pin_bounds_without_rewriting_the_original_receipt() {
        let root = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            compressed_bytes: 4096,
            expanded_bytes: 1024,
            files: 32,
        };
        let cached = CacheStore::open(root.path().join("cached"), limits).unwrap();
        let fresh = CacheStore::open(root.path().join("fresh"), limits).unwrap();
        let mut body = zip_body(&[("safe", b"owned")], &[]);
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).unwrap();
        let loose = PinnedLaunchInput {
            pin_id: 1,
            object_id: 2,
            object_version: "owned-bounds".into(),
            kind: LaunchInputKind::Pack,
            size_bytes: bytes.len() as u64,
            sha256: Sha256Digest::of_bytes(&bytes),
            max_expanded_bytes: 1024,
            max_files: 32,
        };
        let destination = cached.install_input(&loose, &mut body).unwrap();
        let receipt_path = destination.parent().unwrap().join("receipt");
        let original = fs::read(&receipt_path).unwrap();
        let tight = PinnedLaunchInput {
            max_expanded_bytes: 5,
            max_files: 1,
            ..loose.clone()
        };
        assert!(matches!(
            cached.install_input(&tight, &mut body),
            Err(CacheError::PinMismatch)
        ));
        assert_eq!(fs::read(receipt_path).unwrap(), original);
        assert_eq!(fs::read(destination.join("safe")).unwrap(), b"owned");
        // This exact ZIP does satisfy the tight bounds. The failure above is
        // the retained pin contract, not a corrupt archive or insufficient quota.
        let exact = fresh.install_input(&tight, &mut body).unwrap();
        assert_eq!(fs::read(exact.join("safe")).unwrap(), b"owned");
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
    #[test]
    fn exact_publication_body_survives_store_restart_and_pending_cannot_be_deleted() {
        use attune_common::workload_launch::*;
        let root = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            compressed_bytes: 65536,
            expanded_bytes: 65536,
            files: 32,
        };
        let store = CacheStore::open(root.path().to_path_buf(), limits)
            .unwrap()
            .for_current_test_user();
        let mut spec = crate::test_support::spec(format!("sha256:{}", "a".repeat(64)));
        spec.manifest.purpose = LaunchPurpose::Setup;
        spec.manifest.target = LaunchTarget::Setup {
            admission: 1,
            attempt: uuid::Uuid::new_v4(),
        };
        spec.manifest.stage = Some(PrivateStage {
            cache_key: "owned-frozen-binding".into(),
            quota_bytes: 65536,
            quota_files: 32,
        });
        store.prepare_private(&spec.manifest).unwrap();
        assert_eq!(
            store.stage_owner(&spec.manifest).unwrap(),
            (unsafe { libc::geteuid() }, unsafe { libc::getegid() })
        );
        let stage = root
            .path()
            .join(crate::engine::stage_subpath(&spec.manifest));
        fs::write(stage.join("tool"), b"owned-runtime").unwrap();
        let digest = spec.manifest.admitted_digest().unwrap();
        let resource = EngineResourceEvidence {
            engine_id: spec.manifest.engine_id,
            resource_id: "a".repeat(64),
            engine_name_nonce: spec.manifest.engine_name_nonce,
            admitted_digest: digest.clone(),
            public_manifest_digest: Sha256Digest::of_bytes(b"public-engine-policy"),
        };
        let mut status = LaunchStatus {
            request: LaunchRequest {
                launch_id: spec.manifest.launch_id,
                admitted_digest: digest,
            },
            state: LaunchState::Stopped,
            runtime_state: LaunchRuntimeState::Exited,
            exit_outcome: Some(LaunchExitOutcome {
                exit_code: 0,
                oom_killed: false,
            }),
            publication: LaunchPublicationState::Pending,
            capture: LaunchCaptureState::Complete,
            capture_deadline: Some(chrono::Utc::now() + chrono::Duration::seconds(60)),
            resource: Some(resource),
            stop_reason: None,
        };
        let mut first = Vec::new();
        store
            .archive_settled_stage(&spec.manifest, &status)
            .unwrap()
            .read_to_end(&mut first)
            .unwrap();
        assert!(store
            .cleanup_settled_private(&spec.manifest, &status)
            .is_err());
        drop(store);
        let restarted = CacheStore::open(root.path().to_path_buf(), limits)
            .unwrap()
            .for_current_test_user();
        let mut again = Vec::new();
        restarted
            .archive_settled_stage(&spec.manifest, &status)
            .unwrap()
            .read_to_end(&mut again)
            .unwrap();
        assert_eq!(first, again);
        assert!(!first.is_empty());
        status.publication = LaunchPublicationState::Published;
        restarted
            .cleanup_settled_private(&spec.manifest, &status)
            .unwrap();
        assert!(!stage.exists());
    }
    #[test]
    fn private_artifacts_require_settlement_and_reject_escape_aliases_and_unacked_cleanup() {
        use attune_common::workload_launch::*;
        let root = tempfile::tempdir().unwrap();
        let store = CacheStore::open(
            root.path().into(),
            CacheLimits {
                compressed_bytes: 1024,
                expanded_bytes: 32,
                files: 4,
            },
        )
        .unwrap()
        .with_artifact_limits(32, 4)
        .unwrap()
        .for_current_test_user();
        let spec = crate::test_support::spec(format!("sha256:{}", "a".repeat(64)));
        store.prepare_private(&spec.manifest).unwrap();
        store.prepare_artifact_staging(&spec.manifest).unwrap();
        let directory = root.path().join(format!(
            "private/{}/artifacts",
            spec.manifest.engine_name_nonce.simple()
        ));
        fs::write(directory.join("result"), b"owned-file").unwrap();
        // A filesystem fixture, not a CP authorization/removal proof.
        let digest = spec.manifest.admitted_digest().unwrap();
        let mut status = LaunchStatus {
            request: LaunchRequest {
                launch_id: spec.manifest.launch_id,
                admitted_digest: digest.clone(),
            },
            state: LaunchState::Stopped,
            runtime_state: LaunchRuntimeState::Exited,
            exit_outcome: Some(LaunchExitOutcome {
                exit_code: 0,
                oom_killed: false,
            }),
            publication: LaunchPublicationState::NotRequired,
            capture: LaunchCaptureState::Complete,
            capture_deadline: None,
            resource: Some(EngineResourceEvidence {
                engine_id: spec.manifest.engine_id,
                resource_id: "b".repeat(64),
                engine_name_nonce: spec.manifest.engine_name_nonce,
                admitted_digest: digest,
                public_manifest_digest: Sha256Digest::of_bytes(b"fixture"),
            }),
            stop_reason: None,
        };
        let mut bytes = Vec::new();
        store
            .open_settled_artifact(&spec.manifest, &status, "result")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"owned-file");
        store
            .validate_settled_artifact_staging(
                &spec.manifest,
                &status,
                ArtifactLimits {
                    max_total_bytes: 32,
                    max_entries: 4,
                },
            )
            .unwrap();
        assert!(matches!(
            store.validate_settled_artifact_staging(
                &spec.manifest,
                &status,
                ArtifactLimits {
                    max_total_bytes: 8,
                    max_entries: 4
                }
            ),
            Err(CacheError::Quota)
        ));
        fs::write(directory.join("empty"), []).unwrap();
        assert_eq!(
            store
                .open_settled_artifact(&spec.manifest, &status, "empty")
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            0
        );
        assert!(matches!(
            store.cleanup_settled_private(&spec.manifest, &status),
            Err(CacheError::ArtifactPending)
        ));
        assert!(directory.join("result").exists());
        assert!(store
            .open_settled_artifact(&spec.manifest, &status, "../other-attempt")
            .is_err());
        status.state = LaunchState::Created;
        assert!(store
            .open_settled_artifact(&spec.manifest, &status, "result")
            .is_err());
        status.state = LaunchState::Stopped;
        std::os::unix::fs::symlink("result", directory.join("alias")).unwrap();
        assert!(store
            .open_settled_artifact(&spec.manifest, &status, "result")
            .is_err());
        fs::remove_file(directory.join("alias")).unwrap();
        fs::hard_link(directory.join("result"), directory.join("alias")).unwrap();
        assert!(store
            .open_settled_artifact(&spec.manifest, &status, "result")
            .is_err());
        fs::remove_file(directory.join("alias")).unwrap();
        fs::write(directory.join("large"), [0; 33]).unwrap();
        assert!(matches!(
            store.open_settled_artifact(&spec.manifest, &status, "result"),
            Err(CacheError::Quota)
        ));
        let mut receipt = attune_common::workload_artifact::ArtifactPublicationReceipt {
            request: status.request.clone(),
            publication_id: 1,
            state: attune_common::workload_artifact::ArtifactPublicationState::Incomplete,
            cleanup_receipt_id: 0,
        };
        assert!(store
            .cleanup_artifact_staging(&spec.manifest, &status, &receipt)
            .is_err());
        assert!(directory.exists());
        receipt.cleanup_receipt_id = 2;
        store
            .cleanup_artifact_staging(&spec.manifest, &status, &receipt)
            .unwrap();
        assert!(!directory.exists());
        // Same authenticated historical receipt can finish local cleanup again;
        // it grants no upload, allocation or new container creation authority.
        store
            .cleanup_artifact_staging(&spec.manifest, &status, &receipt)
            .unwrap();
    }
}
