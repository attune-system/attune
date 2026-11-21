//! Durable transfer for the private, immutable contents of an installed build.

use super::*;
use crate::blob_store::StoredObject;
use std::collections::BTreeSet;
use std::fs::File;

pub const MAX_PREPARATION_ARCHIVE_BYTES: u64 = 2 * MAX_WORKSPACE_BYTES + 64 * 1024 * 1024;
pub const MAX_PREPARATION_RECEIPT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRuntimeArchive {
    pub key: String,
    pub version: String,
    pub size: u64,
    pub sha256: String,
}

impl From<StoredObject> for PreparedRuntimeArchive {
    fn from(stored: StoredObject) -> Self {
        Self {
            key: stored.key.as_str().into(),
            version: stored.provider_version.as_stored().into(),
            size: stored.size,
            sha256: stored
                .sha256
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        }
    }
}

impl InstalledPreparation {
    pub fn directory(&self) -> Uuid {
        self.directory
    }

    pub fn archive(&self) -> Option<&PreparedRuntimeArchive> {
        self.archive.as_ref()
    }

    pub fn set_archive(&mut self, archive: PreparedRuntimeArchive) -> Result<()> {
        if !self.scope.is_object_backed() || self.archive.is_some() {
            return Err(invalid("Unexpected runtime preparation archive"));
        }
        self.archive = Some(archive);
        Ok(())
    }

    fn directory_path(&self) -> PathBuf {
        self.scope.root.join(self.directory.to_string())
    }

    pub fn is_local(&self) -> bool {
        self.directory_path().is_dir()
    }

    pub async fn build_archive(&self) -> Result<(tempfile::NamedTempFile, u64, String)> {
        let root = self.directory_path();
        let generation = self.generation.clone();
        tokio::task::spawn_blocking(move || {
            let archive = tempfile::NamedTempFile::new()?;
            let mut tar = tar::Builder::new(archive);
            tar.follow_symlinks(false);
            for name in ["pack", &generation] {
                tar.append_dir_all(name, root.join(name))?;
            }
            tar.append_path_with_name(root.join("ready"), "ready")?;
            let file = tar.into_inner()?;
            let bytes = file.as_file().metadata()?.len();
            if bytes > MAX_PREPARATION_ARCHIVE_BYTES {
                return Err(invalid("Prepared runtime archive exceeds the size limit"));
            }
            let sha256 = hash_archive(file.path())?;
            Ok((file, bytes, sha256))
        })
        .await
        .map_err(|_| invalid("Failed to create prepared runtime archive"))?
    }

    pub async fn restore_archive(&self, file: tempfile::NamedTempFile) -> Result<()> {
        let receipt = self.clone();
        tokio::task::spawn_blocking(move || receipt.restore_archive_blocking(file))
            .await
            .map_err(|_| invalid("Failed to restore prepared runtime archive"))?
    }

    fn restore_archive_blocking(&self, file: tempfile::NamedTempFile) -> Result<()> {
        let archive = self
            .archive
            .as_ref()
            .ok_or_else(|| invalid("Missing prepared runtime archive"))?;
        if file.as_file().metadata()?.len() != archive.size
            || archive.size > MAX_PREPARATION_ARCHIVE_BYTES
            || hash_archive(file.path())? != archive.sha256
        {
            return Err(invalid(
                "Prepared runtime archive does not match its receipt",
            ));
        }
        validate_archive(file.reopen()?, Path::new(&self.generation))?;
        let staging = tempfile::Builder::new()
            .prefix("restore-")
            .tempdir_in(&self.scope.root)?;
        let mut unpack = tar::Archive::new(file.reopen()?);
        unpack.set_preserve_permissions(true);
        unpack.unpack(staging.path())?;
        if std::fs::read_to_string(staging.path().join("ready"))? != self.seal.to_string() {
            return Err(invalid(
                "Prepared runtime archive seal does not match its receipt",
            ));
        }
        let destination = self.directory_path();
        match std::fs::rename(staging.path(), &destination) {
            Ok(()) => sync_dir(&self.scope.root)?,
            Err(_) if destination.is_dir() => {
                // Another reader restored the same, manager-owned directory.
                // Both readers still verify its receipt before use.
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

fn hash_archive(path: &Path) -> Result<String> {
    crate::pack_registry::calculate_file_checksum(path)
        .map_err(|_| invalid("Cannot hash prepared runtime archive"))
}

fn validate_archive(file: File, generation: &Path) -> Result<()> {
    let mut tar = tar::Archive::new(file);
    let mut paths = BTreeSet::new();
    let mut non_directories = BTreeSet::new();
    let mut total = 0u64;
    for entry in tar.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        let kind = entry.header().entry_type();
        let ready = path == Path::new("ready");
        if !path.components().all(|c| matches!(c, Component::Normal(_)))
            || !(ready || path.starts_with("pack") || path.starts_with(generation))
            || paths.len() >= MAX_WORKSPACE_FILES * 2 + 3
            || !paths.insert(path.clone())
            || !(kind.is_file() || kind.is_dir() || kind.is_symlink())
            || entry.header().mode()? & 0o7000 != 0
            || (ready && (!kind.is_file() || entry.size() != 36))
            || ((path == Path::new("pack") || path == generation) && !kind.is_dir())
        {
            return Err(invalid("Invalid prepared runtime archive entry"));
        }
        if !kind.is_dir() {
            non_directories.insert(path);
        }
        total = total.saturating_add(entry.size());
        if total > MAX_WORKSPACE_BYTES * 2 + 36 {
            return Err(invalid("Unpacked runtime archive exceeds the size limit"));
        }
    }
    if !paths.contains(Path::new("ready"))
        || !paths.contains(Path::new("pack"))
        || !paths.contains(generation)
        || paths.iter().any(|path| {
            path.ancestors()
                .skip(1)
                .any(|parent| non_directories.contains(parent))
        })
    {
        return Err(invalid("Invalid prepared runtime archive layout"));
    }
    Ok(())
}
