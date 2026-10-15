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
        let kind = entry.header().entry_type();
        let path = entry
            .path()
            .map_err(|_| Error::validation("Invalid archive member path"))?
            .into_owned();
        let member = display_member(&path);
        state
            .next_entry()
            .map_err(|error| archive_error(error, &member))?;
        if !kind.is_file() && !kind.is_dir() {
            let description = match kind.as_byte() {
                b'1' => "hard link",
                b'2' => "symbolic link",
                b'3' => "character device",
                b'4' => "block device",
                b'6' => "FIFO",
                _ => "unsupported special entry",
            };
            return Err(archive_error(Error::validation(format!("Archive contains a forbidden {description}. Remove this member and rebuild the archive.")), &member));
        }
        let declared_size = entry
            .header()
            .size()
            .map_err(|error| Error::validation(format!("Invalid entry size: {error}")))?;
        if kind.is_dir() {
            if declared_size != 0 {
                return Err(Error::validation(
                    "Archive directory entries must not contain data",
                ));
            }
            if is_archive_root_directory(&path) {
                continue;
            }
            let relative = validate_relative_path(&path)?;
            create_safe_directories(dest, &relative)?;
            continue;
        }
        let relative =
            validate_relative_path(&path).map_err(|error| archive_error(error, &member))?;
        let target = safe_target(dest, &relative)?;
        let executable = entry
            .header()
            .mode()
            .map_err(|error| Error::validation(format!("Invalid entry mode: {error}")))?
            & 0o111
            != 0;
        state
            .check_declared_size(declared_size)
            .map_err(|error| archive_error(error, &member))?;
        create_safe_parent(dest, &relative)?;
        let written = write_bounded(
            &mut entry,
            &target,
            limits.max_entry_bytes.min(state.remaining_bytes()),
            executable,
        )
        .map_err(|error| archive_error(error, &member))?;
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
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::validation(format!("Invalid ZIP entry: {error}")))?;
        let mode_type = entry.unix_mode().unwrap_or(0) & 0o170000;
        let member = display_member(Path::new(entry.name()));
        state
            .next_entry()
            .map_err(|error| archive_error(error, &member))?;
        if mode_type != 0 && mode_type != 0o040000 && mode_type != 0o100000 {
            return Err(archive_error(Error::validation("ZIP archive contains a symlink or special file. Remove this member and rebuild the archive."), &member));
        }
        let path = Path::new(entry.name());
        let is_directory = entry.is_dir() || mode_type == 0o040000;
        if is_directory {
            if entry.size() != 0 {
                return Err(Error::validation(
                    "Archive directory entries must not contain data",
                ));
            }
            if is_archive_root_directory(path) {
                continue;
            }
            let relative = validate_relative_path(path)?; // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The archive entry is parsed only as input to strict relative-component validation before destination joining.
            create_safe_directories(dest, &relative)?;
            continue;
        }
        let relative = validate_relative_path(path)?; // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The archive entry is parsed only as input to strict relative-component validation before destination joining.
        let target = safe_target(dest, &relative)?;
        let executable = entry.unix_mode().unwrap_or(0) & 0o111 != 0;
        state
            .check_declared_size(entry.size())
            .map_err(|error| archive_error(error, &member))?;
        create_safe_parent(dest, &relative)?;
        let written = write_bounded(
            &mut entry,
            &target,
            limits.max_entry_bytes.min(state.remaining_bytes()),
            executable,
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

fn is_archive_root_directory(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::CurDir))
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

fn write_bounded(
    reader: &mut impl Read,
    target: &Path,
    max_bytes: u64,
    _executable: bool,
) -> Result<u64> {
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Keep only executable intent, never archive ownership or privileged bits.
        // Use the open handle so permission changes cannot follow a replaced path.
        let mode = if _executable { 0o755 } else { 0o644 };
        writer
            .get_ref()
            .set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|error| {
                Error::io(format!("Failed to set extracted file permissions: {error}"))
            })?;
    }
    Ok(written)
}

struct ExtractionState {
    limits: SafeExtractionLimits,
    entries: u32,
    bytes: u64,
}

fn display_member(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .take(240)
        .flat_map(char::escape_default)
        .collect()
}

fn archive_error(error: Error, member: &str) -> Error {
    let mut error = super::diagnostics::located(error, member, "");
    if let Error::Deployment { details, .. } = &mut error {
        details.stage = "archive_extraction".into();
    }
    error
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
            return Err(Error::validation(format!(
                "Archive contains too many entries: {} exceeds the allowed {}",
                self.entries, self.limits.max_entries
            )));
        }
        Ok(())
    }
    fn check_declared_size(&self, size: u64) -> Result<()> {
        if size > self.limits.max_entry_bytes {
            return Err(Error::validation(format!("Archive entry exceeds the per-entry extracted size limit: {size} bytes exceeds {} bytes", self.limits.max_entry_bytes)));
        }
        if self.bytes.saturating_add(size) > self.limits.max_total_bytes {
            return Err(Error::validation(format!(
                "Archive exceeds the total extracted size limit: {} bytes exceeds {} bytes",
                self.bytes.saturating_add(size),
                self.limits.max_total_bytes
            )));
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

    fn tar_with_root_directory_and_file() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut root = tar::Header::new_gnu();
        root.set_path(".").unwrap();
        root.set_size(0);
        root.set_entry_type(tar::EntryType::Directory);
        root.set_mode(0o755);
        root.set_cksum();
        builder.append(&root, std::io::empty()).unwrap();

        let mut file = tar::Header::new_gnu();
        file.set_path("./pack.yaml").unwrap();
        file.set_size(9);
        file.set_mode(0o644);
        file.set_cksum();
        builder.append(&file, b"ref: demo".as_slice()).unwrap();
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

    #[cfg(unix)]
    #[test]
    fn tar_preserves_only_executable_intent() {
        use std::os::unix::fs::PermissionsExt;

        let mut builder = tar::Builder::new(Vec::new());
        let files = [
            ("sensors/launcher", b"#!/bin/sh\n".as_slice(), 0o755, 0o755),
            (
                "sensors/bin/native",
                b"\x7fELF\0\x01".as_slice(),
                0o711,
                0o755,
            ),
            ("other-execute", b"run".as_slice(), 0o001, 0o755),
            ("privileged-execute", b"run".as_slice(), 0o7777, 0o755),
            ("pack.yaml", b"ref: demo".as_slice(), 0o644, 0o644),
            ("privileged-data", b"data".as_slice(), 0o7666, 0o644),
            ("unreadable-data", b"data".as_slice(), 0o000, 0o644),
        ];
        for (path, data, mode, _) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(mode);
            builder.append_data(&mut header, path, data).unwrap();
        }
        let dest = tempfile::tempdir().unwrap();
        extract_tar(
            Cursor::new(builder.into_inner().unwrap()),
            dest.path(),
            SafeExtractionLimits::default(),
        )
        .unwrap();

        for (path, data, _, expected_mode) in files {
            let target = dest.path().join(path);
            assert_eq!(fs::read(&target).unwrap(), data);
            assert_eq!(
                fs::metadata(target).unwrap().permissions().mode() & 0o7777,
                expected_mode,
                "{path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn zip_preserves_only_executable_intent() {
        use std::os::unix::fs::PermissionsExt;

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let files = [
            ("sensors/launcher", 0o755, 0o755),
            ("sensors/bin/native", 0o711, 0o755),
            ("other-execute", 0o001, 0o755),
            ("privileged-execute", 0o7777, 0o755),
            ("pack.yaml", 0o644, 0o644),
            ("privileged-data", 0o7666, 0o644),
            ("unreadable-data", 0o000, 0o644),
        ];
        for (path, mode, _) in files {
            writer
                .start_file(
                    path,
                    zip::write::SimpleFileOptions::default().unix_permissions(mode),
                )
                .unwrap();
            writer.write_all(b"contents").unwrap();
        }
        let mut bytes = writer.finish().unwrap().into_inner();
        // ZipWriter masks privileged bits, so inject the source modes into
        // central-directory attributes to test sanitization of untrusted ZIPs.
        let headers: Vec<_> = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(offset, window)| (window == b"\x50\x4b\x01\x02").then_some(offset))
            .collect();
        assert_eq!(headers.len(), files.len());
        for (offset, (_, mode, _)) in headers.into_iter().zip(files) {
            bytes[offset + 5] = 3; // Unix creator OS.
            bytes[offset + 38..offset + 42]
                .copy_from_slice(&((0o100000_u32 | mode) << 16).to_le_bytes());
        }
        let dest = tempfile::tempdir().unwrap();
        extract_zip(
            Cursor::new(bytes),
            dest.path(),
            SafeExtractionLimits::default(),
        )
        .unwrap();

        for (path, _, expected_mode) in files {
            let target = dest.path().join(path);
            assert_eq!(fs::read(&target).unwrap(), b"contents");
            assert_eq!(
                fs::metadata(target).unwrap().permissions().mode() & 0o7777,
                expected_mode,
                "{path}"
            );
        }
    }

    #[test]
    fn tar_and_zip_reject_duplicate_files() {
        let mut builder = tar::Builder::new(Vec::new());
        for contents in [b"first".as_slice(), b"second".as_slice()] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            builder.append_data(&mut header, "run", contents).unwrap();
        }
        let dest = tempfile::tempdir().unwrap();
        let error = extract_tar(
            Cursor::new(builder.into_inner().unwrap()),
            dest.path(),
            SafeExtractionLimits::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(dest.path().join("run")).unwrap(), b"first");

        // Construct colliding normalized paths; ZipWriter rejects identical names.
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (path, contents) in [
            ("run", b"first".as_slice()),
            ("./run", b"second".as_slice()),
        ] {
            writer
                .start_file(
                    path,
                    zip::write::SimpleFileOptions::default().unix_permissions(0o755),
                )
                .unwrap();
            writer.write_all(contents).unwrap();
        }
        let dest = tempfile::tempdir().unwrap();
        let error = extract_zip(
            Cursor::new(writer.finish().unwrap().into_inner()),
            dest.path(),
            SafeExtractionLimits::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(dest.path().join("run")).unwrap(), b"first");
    }

    #[cfg(unix)]
    #[test]
    fn tar_ignores_restrictive_and_privileged_directory_modes() {
        use std::os::unix::fs::PermissionsExt;

        for mode in [0o000, 0o7777] {
            let mut builder = tar::Builder::new(Vec::new());
            let mut directory = tar::Header::new_gnu();
            directory.set_entry_type(tar::EntryType::Directory);
            directory.set_size(0);
            directory.set_mode(mode);
            builder
                .append_data(&mut directory, "nested", std::io::empty())
                .unwrap();
            let mut file = tar::Header::new_gnu();
            file.set_size(4);
            file.set_mode(0o755);
            builder
                .append_data(&mut file, "nested/run", b"data".as_slice())
                .unwrap();
            let dest = tempfile::tempdir().unwrap();
            extract_tar(
                Cursor::new(builder.into_inner().unwrap()),
                dest.path(),
                SafeExtractionLimits::default(),
            )
            .unwrap();

            let permissions = fs::metadata(dest.path().join("nested"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(permissions & 0o700, 0o700);
            assert_eq!(permissions & 0o7000, 0);
            assert_eq!(fs::read(dest.path().join("nested/run")).unwrap(), b"data");
        }
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
    fn tar_accepts_archive_root_directory_entry() {
        let bytes = tar_with_root_directory_and_file();
        let dest = tempfile::tempdir().unwrap();

        extract_tar(
            Cursor::new(bytes),
            dest.path(),
            SafeExtractionLimits::default(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(dest.path().join("pack.yaml")).unwrap(),
            "ref: demo"
        );
    }

    #[test]
    fn tar_rejects_directory_entries_with_data() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut directory = tar::Header::new_gnu();
        directory.set_path(".").unwrap();
        directory.set_size(4);
        directory.set_entry_type(tar::EntryType::Directory);
        directory.set_mode(0o755);
        directory.set_cksum();
        builder.append(&directory, b"data".as_slice()).unwrap();
        let bytes = builder.into_inner().unwrap();
        let dest = tempfile::tempdir().unwrap();

        assert!(extract_tar(
            Cursor::new(bytes),
            dest.path(),
            SafeExtractionLimits::default(),
        )
        .is_err());
    }

    #[test]
    fn archive_root_exception_applies_only_to_directories() {
        assert!(is_archive_root_directory(Path::new(".")));
        assert!(validate_relative_path(Path::new(".")).is_err());
        assert!(validate_relative_path(Path::new("")).is_err());
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
