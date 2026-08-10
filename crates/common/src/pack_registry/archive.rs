use crate::error::{Error, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct SafeExtractionLimits {
    pub max_entries: u32,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for SafeExtractionLimits {
    fn default() -> Self {
        Self {
            max_entries: crate::config::PackUploadConfig::DEFAULT_MAX_FILE_COUNT,
            max_entry_bytes: crate::config::PackUploadConfig::DEFAULT_MAX_PER_ENTRY_SIZE_BYTES,
            max_total_bytes: crate::config::PackUploadConfig::DEFAULT_MAX_EXTRACTED_SIZE_BYTES,
        }
    }
}

pub fn extract_archive(
    archive_path: &Path,
    dest: &Path,
    limits: SafeExtractionLimits,
) -> Result<()> {
    let name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let file = File::open(archive_path) // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- This safe-archive helper intentionally opens its caller-selected local archive before validating every extracted entry.
        .map_err(|error| Error::io(format!("Failed to open archive: {error}")))?;

    if name.ends_with(".zip") {
        extract_zip(file, dest, limits)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".gz") {
        extract_tar(flate2::read::GzDecoder::new(file), dest, limits)
    } else if name.ends_with(".tar") {
        extract_tar(file, dest, limits)
    } else {
        Err(Error::validation(
            "Unsupported archive format; expected ZIP, TAR, or TGZ",
        ))
    }
}

pub fn extract_tar<R: Read>(reader: R, dest: &Path, limits: SafeExtractionLimits) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    archive.set_overwrite(false);
    archive.set_unpack_xattrs(false);
    archive.set_preserve_permissions(false);
    archive.set_preserve_mtime(false);
    extract_tar_archive(&mut archive, dest, limits)
}

pub fn extract_tar_archive<R: Read>(
    archive: &mut tar::Archive<R>,
    dest: &Path,
    limits: SafeExtractionLimits,
) -> Result<()> {
    prepare_destination(dest)?;
    let mut state = ExtractionState::new(limits);

    for entry in archive
        .entries()
        .map_err(|error| Error::validation(format!("Invalid TAR archive: {error}")))?
    {
        let mut entry =
            entry.map_err(|error| Error::validation(format!("Invalid TAR entry: {error}")))?;
        state.next_entry()?;
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(Error::validation(format!(
                "Archive contains forbidden link or special entry type: {kind:?}"
            )));
        }
        let path = entry
            .path()
            .map_err(|error| Error::validation(format!("Invalid archive path: {error}")))?;
        let relative = validate_relative_path(&path)?;
        if kind.is_dir() {
            create_safe_directories(dest, &relative)?;
            continue;
        }
        let target = safe_target(dest, &relative)?;
        state.check_declared_size(
            entry
                .header()
                .size()
                .map_err(|error| Error::validation(format!("Invalid entry size: {error}")))?,
        )?;
        create_safe_parent(dest, &relative)?;
        let written = write_bounded(
            &mut entry,
            &target,
            limits.max_entry_bytes.min(state.remaining_bytes()),
        )?;
        state.add_bytes(written)?;
    }
    Ok(())
}

pub fn extract_zip<R: Read + Seek>(
    reader: R,
    dest: &Path,
    limits: SafeExtractionLimits,
) -> Result<()> {
    prepare_destination(dest)?;
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| Error::validation(format!("Invalid ZIP archive: {error}")))?;
    let mut state = ExtractionState::new(limits);

    for index in 0..archive.len() {
        state.next_entry()?;
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::validation(format!("Invalid ZIP entry: {error}")))?;
        let mode_type = entry.unix_mode().unwrap_or(0) & 0o170000;
        if mode_type != 0 && mode_type != 0o040000 && mode_type != 0o100000 {
            return Err(Error::validation(
                "ZIP archive contains a symlink or special file",
            ));
        }
        let relative = validate_relative_path(Path::new(entry.name()))?; // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The archive entry is parsed only as input to strict relative-component validation before destination joining.
        if entry.is_dir() || mode_type == 0o040000 {
            create_safe_directories(dest, &relative)?;
            continue;
        }
        let target = safe_target(dest, &relative)?;
        state.check_declared_size(entry.size())?;
        create_safe_parent(dest, &relative)?;
        let written = write_bounded(
            &mut entry,
            &target,
            limits.max_entry_bytes.min(state.remaining_bytes()),
        )?;
        state.add_bytes(written)?;
    }
    Ok(())
}

fn prepare_destination(dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)
        .map_err(|error| Error::io(format!("Failed to create extraction directory: {error}")))?;
    let metadata = fs::symlink_metadata(dest)
        .map_err(|error| Error::io(format!("Failed to inspect extraction directory: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::validation(
            "Archive destination must be a real directory",
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(Error::validation(
            "Archive entry path must be relative and non-empty",
        ));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::validation(format!(
                    "Unsafe archive entry path: {}",
                    path.display()
                )));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(Error::validation("Archive entry path must be non-empty"));
    }
    Ok(clean)
}

fn safe_target(dest: &Path, relative: &Path) -> Result<PathBuf> {
    let target = dest.join(relative);
    if target.exists() || fs::symlink_metadata(&target).is_ok() {
        return Err(Error::validation(format!(
            "Archive entry already exists: {}",
            relative.display()
        )));
    }
    Ok(target)
}

fn create_safe_parent(dest: &Path, relative: &Path) -> Result<()> {
    if let Some(parent) = relative.parent() {
        create_safe_directories(dest, parent)?;
    }
    Ok(())
}

fn create_safe_directories(dest: &Path, relative: &Path) -> Result<()> {
    let mut current = dest.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::validation(format!(
                    "Archive path has a symlink parent: {}",
                    current.display()
                )));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(Error::validation(format!(
                    "Archive path parent is not a directory: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    Error::io(format!("Failed to create archive directory: {error}"))
                })?;
            }
            Err(error) => {
                return Err(Error::io(format!(
                    "Failed to inspect archive path: {error}"
                )))
            }
        }
    }
    Ok(())
}

fn write_bounded(reader: &mut impl Read, target: &Path, max_bytes: u64) -> Result<u64> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| {
            Error::io(format!(
                "Failed to create extracted file {}: {error}",
                target.display()
            ))
        })?;
    let mut writer = std::io::BufWriter::new(file);
    let mut limited = reader.take(max_bytes.saturating_add(1));
    let written = std::io::copy(&mut limited, &mut writer)
        .map_err(|error| Error::io(format!("Failed to extract {}: {error}", target.display())))?;
    if written > max_bytes {
        drop(writer);
        let _ = fs::remove_file(target);
        return Err(Error::validation(
            "Archive entry exceeds the per-entry extracted size limit",
        ));
    }
    writer
        .flush()
        .map_err(|error| Error::io(format!("Failed to flush extracted file: {error}")))?;
    Ok(written)
}

struct ExtractionState {
    limits: SafeExtractionLimits,
    entries: u32,
    bytes: u64,
}

impl ExtractionState {
    fn new(limits: SafeExtractionLimits) -> Self {
        Self {
            limits,
            entries: 0,
            bytes: 0,
        }
    }
    fn next_entry(&mut self) -> Result<()> {
        self.entries = self.entries.saturating_add(1);
        if self.entries > self.limits.max_entries {
            return Err(Error::validation("Archive contains too many entries"));
        }
        Ok(())
    }
    fn check_declared_size(&self, size: u64) -> Result<()> {
        if size > self.limits.max_entry_bytes {
            return Err(Error::validation(
                "Archive entry exceeds the per-entry extracted size limit",
            ));
        }
        if self.bytes.saturating_add(size) > self.limits.max_total_bytes {
            return Err(Error::validation(
                "Archive exceeds the total extracted size limit",
            ));
        }
        Ok(())
    }
    fn add_bytes(&mut self, size: u64) -> Result<()> {
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > self.limits.max_total_bytes {
            return Err(Error::validation(
                "Archive exceeds the total extracted size limit",
            ));
        }
        Ok(())
    }

    fn remaining_bytes(&self) -> u64 {
        self.limits.max_total_bytes.saturating_sub(self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tar_with_file(path: &str, data: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, data).unwrap();
        builder.into_inner().unwrap()
    }

    fn zip_with_file(path: &str, data: &[u8]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn tar_limits_extracted_bytes_and_entries() {
        let bytes = tar_with_file("large", b"12345");
        let dest = tempfile::tempdir().unwrap();
        let limits = SafeExtractionLimits {
            max_entries: 1,
            max_entry_bytes: 4,
            max_total_bytes: 4,
        };
        assert!(extract_tar(Cursor::new(bytes), dest.path(), limits).is_err());
        assert!(!dest.path().join("large").exists());
    }

    #[test]
    fn zip_rejects_parent_paths_and_bombs() {
        let traversal = zip_with_file("../outside", b"bad");
        let dest = tempfile::tempdir().unwrap();
        assert!(extract_zip(
            Cursor::new(traversal),
            dest.path(),
            SafeExtractionLimits::default()
        )
        .is_err());
        assert!(!dest.path().parent().unwrap().join("outside").exists());

        let bomb = zip_with_file("large", &vec![0; 1024]);
        let limits = SafeExtractionLimits {
            max_entries: 2,
            max_entry_bytes: 2048,
            max_total_bytes: 512,
        };
        assert!(extract_zip(Cursor::new(bomb), dest.path(), limits).is_err());
    }

    #[test]
    fn zip_rejects_symlinks() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("link", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"target").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        let central = bytes
            .windows(4)
            .position(|window| window == b"\x50\x4b\x01\x02")
            .unwrap();
        bytes[central + 5] = 3; // Unix creator OS.
        bytes[central + 38..central + 42].copy_from_slice(&((0o120777_u32) << 16).to_le_bytes());
        let dest = tempfile::tempdir().unwrap();

        assert!(extract_zip(
            Cursor::new(bytes),
            dest.path(),
            SafeExtractionLimits::default()
        )
        .is_err());
    }

    #[test]
    fn tar_rejects_links() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_mode(0o777);
        builder.append_link(&mut header, "link", "target").unwrap();
        let bytes = builder.into_inner().unwrap();
        let dest = tempfile::tempdir().unwrap();
        assert!(extract_tar(
            Cursor::new(bytes),
            dest.path(),
            SafeExtractionLimits::default()
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn extraction_rejects_preexisting_symlink_parent() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let outside = root.path().join("outside");
        fs::create_dir(&destination).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, destination.join("nested")).unwrap();
        let bytes = tar_with_file("nested/file", b"bad");

        assert!(extract_tar(
            Cursor::new(bytes),
            &destination,
            SafeExtractionLimits::default()
        )
        .is_err());
        assert!(!outside.join("file").exists());
    }
}
