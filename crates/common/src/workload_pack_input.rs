//! Trusted manager conversion of the primary runtime-v1 release to a launch ZIP.
//! No mutable unpacked tree or host cache pathname is accepted. Callers own
//! pinned acquisition and cancellation-safe blocking work. Selected secrets,
//! tokens and stdin belong exclusively in the encrypted frozen context.
use crate::{
    pack_format,
    pack_registry::storage::PackReleaseManifest,
    workload_archive::{self, ArchivePolicy},
    workload_launch::{LaunchInputKind, Sha256Digest},
    Error, Result,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
};

#[derive(Clone)]
pub struct ReleaseInputPin {
    pub pack_ref: String,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
}
pub struct PreparedPackInput {
    pub(crate) file: tempfile::NamedTempFile,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: Sha256Digest,
    pub(crate) expanded_bytes: u64,
    pub(crate) entries: u64,
    pub(crate) source_pin: ReleaseInputPin,
}
impl PreparedPackInput {
    pub fn source_pin(&self) -> &ReleaseInputPin {
        &self.source_pin
    }
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    pub fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
    pub fn expanded_bytes(&self) -> u64 {
        self.expanded_bytes
    }
    pub fn entries(&self) -> u64 {
        self.entries
    }
    pub fn into_file(self) -> tempfile::NamedTempFile {
        self.file
    }
}
fn invalid() -> Error {
    Error::validation("Pinned release cannot be materialized within launch policy")
}
fn digest(hash: Sha256) -> Result<Sha256Digest> {
    Sha256Digest::parse(
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

struct BoundedOutput<'a> {
    file: &'a mut File,
    maximum: u64,
}
impl Write for BoundedOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .file
            .stream_position()?
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.maximum)
        {
            return Err(std::io::Error::other("launch ZIP exceeds byte limit"));
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for BoundedOutput<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let result = self.file.seek(position)?;
        if result > self.maximum {
            return Err(std::io::Error::other("launch ZIP seek exceeds limit"));
        }
        Ok(result)
    }
}

/// Read accounting includes TAR metadata that its entry iterator consumes
/// internally; hitting a limit never masquerades as an authenticated EOF.
struct BoundedRead<R> {
    inner: R,
    remaining: u64,
}
impl<R: Read> Read for BoundedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut extra = [0];
            return if self.inner.read(&mut extra)? == 0 {
                Ok(0)
            } else {
                Err(std::io::Error::other("runtime archive exceeds bounds"))
            };
        }
        let count = buf
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let read = self.inner.read(&mut buf[..count])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

/// Blocking and bounded. Verify a private copy, then stream its exact regular
/// file inventory without extraction. Strip only the verified pack-ref prefix;
/// preserve payloads and executable modes, including both native platforms.
/// The caller obtains this manifest and pin from the SAME retained immutable
/// release record (including its version/source authority), never from a runner
/// submission. This pure conversion does not authorize execution/publication.
pub fn release_zip<R: Read>(
    source: R,
    pin: &ReleaseInputPin,
    retained_manifest: &PackReleaseManifest,
    release_limits: pack_format::Limits,
    policy: &ArchivePolicy,
) -> Result<PreparedPackInput> {
    release_zip_inner(source, pin, retained_manifest, release_limits, policy).map_err(|_| invalid())
}

fn release_zip_inner<R: Read>(
    source: R,
    pin: &ReleaseInputPin,
    retained_manifest: &PackReleaseManifest,
    release_limits: pack_format::Limits,
    policy: &ArchivePolicy,
) -> anyhow::Result<PreparedPackInput> {
    if policy.root != LaunchInputKind::Pack
        || pin.size_bytes == 0
        || pin.size_bytes > release_limits.compressed_bytes
    {
        return Err(invalid().into());
    }
    let mut private = tempfile::NamedTempFile::new()?;
    let mut source = source.take(pin.size_bytes.checked_add(1).ok_or_else(invalid)?);
    let mut hash = Sha256::new();
    let mut copied = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .filter(|size| *size <= pin.size_bytes)
            .ok_or_else(invalid)?;
        hash.update(&buffer[..read]);
        private.write_all(&buffer[..read])?;
    }
    if copied != pin.size_bytes || digest(hash)? != pin.sha256 {
        return Err(invalid().into());
    }
    private.flush()?;
    // This is the runtime format emitted by PackStorage::publish_release, not
    // the offline CLI format. Never sniff or retry another archive reader.
    let manifest = retained_manifest;
    if manifest.format_version != 1
        || manifest.pack_ref != pin.pack_ref
        || manifest.version.is_empty()
        || manifest.files.len() as u64 > release_limits.entries
        || serde_json::to_vec(manifest)?.len() as u64 > release_limits.manifest_bytes
    {
        return Err(invalid().into());
    }
    crate::schema::RefValidator::validate_pack_ref(&manifest.pack_ref)?;
    let mut expected = BTreeMap::new();
    let mut logical = Vec::with_capacity(manifest.files.len());
    let mut expanded = 0u64;
    let mut previous: Option<&str> = None;
    for file in &manifest.files {
        expanded = expanded.checked_add(file.size).ok_or_else(invalid)?;
        if file.size > release_limits.file_bytes
            || expanded > release_limits.extracted_bytes
            || previous.is_some_and(|name| name >= file.path.as_str())
        {
            return Err(invalid().into());
        }
        previous = Some(&file.path);
        let hash = Sha256Digest::parse(&file.sha256)?;
        if expected
            .insert(file.path.clone(), (file.size, hash, file.executable))
            .is_some()
        {
            return Err(invalid().into());
        }
        logical.push(workload_archive::ArchiveEntry {
            name: file.path.clone(),
            size_bytes: file.size,
            kind: workload_archive::ArchiveEntryKind::File {
                executable: file.executable,
            },
        });
    }
    workload_archive::validate_tree(&logical, policy)?;
    // Account for TAR headers, GNU long-name records and data padding as well
    // as payload bytes. Canonical runtime archives contain files only.
    let archive_budget = release_limits
        .extracted_bytes
        .checked_add(release_limits.manifest_bytes)
        .and_then(|n| {
            release_limits
                .entries
                .checked_mul(2048)
                .and_then(|headers| n.checked_add(headers))
        })
        .and_then(|n| n.checked_add(1024))
        .ok_or_else(invalid)?;
    let mut output = tempfile::NamedTempFile::new()?;
    private.as_file_mut().rewind()?;
    {
        let compressed = BufReader::new(private.as_file_mut());
        let decoder = flate2::bufread::GzDecoder::new(compressed);
        let mut archive = tar::Archive::new(BoundedRead {
            inner: decoder,
            remaining: archive_budget,
        });
        let bounded = BoundedOutput {
            file: output.as_file_mut(),
            maximum: policy.max_compressed_bytes,
        };
        let mut zip = zip::ZipWriter::new(bounded);
        let prefix = format!("{}/", pin.pack_ref);
        let mut long_name: Option<String> = None;
        // Raw iteration prevents tar's convenience layer from allocating an
        // attacker-sized GNU/PAX metadata body before our path budget check.
        for entry in archive.entries().map_err(|_| invalid())?.raw(true) {
            let mut entry = entry.map_err(|_| invalid())?;
            if entry.header().as_gnu().is_none() {
                return Err(invalid().into());
            }
            if entry.header().entry_type().is_gnu_longname() {
                let maximum = 4096u64
                    .checked_add(pin.pack_ref.len() as u64)
                    .and_then(|n| n.checked_add(2))
                    .ok_or_else(invalid)?;
                if long_name.is_some()
                    || entry.size() == 0
                    || entry.size() > maximum
                    || entry.path_bytes().as_ref() != b"././@LongLink"
                    || entry.header().mode()? != 0o644
                    || entry.header().uid()? != 0
                    || entry.header().gid()? != 0
                    || entry.header().mtime()? != 0
                {
                    return Err(invalid().into());
                }
                let size = entry.size();
                let mut bytes = Vec::with_capacity(size as usize);
                entry.read_to_end(&mut bytes)?;
                if bytes.len() as u64 != size || bytes.pop() != Some(0) || bytes.contains(&0) {
                    return Err(invalid().into());
                }
                long_name = Some(String::from_utf8(bytes).map_err(|_| invalid())?);
                continue;
            }
            if !entry.header().entry_type().is_file() || entry.header().link_name_bytes().is_some()
            {
                return Err(invalid().into());
            }
            let path = match long_name.take() {
                Some(name) => name,
                None => entry
                    .path()
                    .map_err(|_| invalid())?
                    .to_str()
                    .ok_or_else(invalid)?
                    .to_owned(),
            };
            let name = path.strip_prefix(&prefix).ok_or_else(invalid)?;
            let (size, sha256, mode) = expected.remove(name).ok_or_else(invalid)?;
            if entry.size() != size
                || entry.header().mode()? != if mode { 0o755 } else { 0o644 }
                || entry.header().uid()? != 0
                || entry.header().gid()? != 0
                || entry.header().mtime()? != 0
            {
                return Err(invalid().into());
            }
            let permissions = if mode { 0o755 } else { 0o644 };
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .last_modified_time(zip::DateTime::default())
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(permissions),
            )
            .map_err(|_| invalid())?;
            let mut hash = Sha256::new();
            let mut count = 0u64;
            loop {
                let read = entry.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                count = count
                    .checked_add(read as u64)
                    .filter(|value| *value <= size)
                    .ok_or_else(invalid)?;
                hash.update(&buffer[..read]);
                zip.write_all(&buffer[..read]).map_err(|_| invalid())?;
            }
            if count != size || digest(hash)? != sha256 {
                return Err(invalid().into());
            }
        }
        if !expected.is_empty() || long_name.is_some() {
            return Err(invalid().into());
        }
        // Finish this single gzip member through CRC verification. Only the
        // remainder of TAR's two zero blocks is allowed after the entry list.
        let mut decoded = archive.into_inner();
        let mut tail = [0; 1025];
        let mut count = 0;
        loop {
            if count == tail.len() {
                return Err(invalid().into());
            }
            let read = decoded.read(&mut tail[count..])?;
            if read == 0 {
                break;
            }
            if tail[count..count + read].iter().any(|b| *b != 0) {
                return Err(invalid().into());
            }
            count += read;
            if count > 1024 {
                return Err(invalid().into());
            }
        }
        let mut compressed = decoded.inner.into_inner();
        if !compressed.fill_buf()?.is_empty() {
            return Err(invalid().into());
        }
        zip.finish().map_err(|_| invalid())?;
    }
    let checked = workload_archive::validate_zip(output.as_file_mut(), policy)?;
    output.as_file_mut().rewind()?;
    let mut hash = Sha256::new();
    loop {
        let read = output.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    output.as_file().sync_all()?;
    let size_bytes = output.as_file().metadata()?.len();
    output.as_file_mut().rewind()?;
    Ok(PreparedPackInput {
        file: output,
        size_bytes,
        sha256: digest(hash)?,
        expanded_bytes: checked.expanded_bytes,
        entries: checked.files,
        source_pin: pin.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack_registry::{storage::PackReleaseFile, PackStorage};
    use std::{
        fs,
        io::{Cursor, Write},
        path::{Path, PathBuf},
    };
    fn fixture() -> (
        tempfile::TempDir,
        PathBuf,
        ReleaseInputPin,
        PackReleaseManifest,
    ) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir_all(source.join("actions/bin/echo")).unwrap();
        fs::write(source.join("pack.yaml"), "ref: owned\nversion: 1.0.0\n").unwrap();
        fs::write(
            source.join("actions/echo.yaml"),
            "ref: owned.echo\nrunner_type: native\nentry_point: bin/echo\n",
        )
        .unwrap();
        for variant in ["linux-amd64", "linux-arm64"] {
            let path = source.join("actions/bin/echo").join(variant);
            fs::write(&path, format!("owned static {variant} bytes\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        fs::write(source.join("payload"), b"exact bytes\r\n").unwrap();
        fs::write(source.join("p".repeat(160)), b"long public name bytes").unwrap();
        let published = PackStorage::new(root.path().join("publisher"))
            .publish_release(&source, "owned", "1.0.0")
            .unwrap();
        let pin = ReleaseInputPin {
            pack_ref: "owned".into(),
            size_bytes: published.archive_size,
            sha256: Sha256Digest::parse(published.digest).unwrap(),
        };
        (root, published.archive_path, pin, published.manifest)
    }
    fn policy() -> ArchivePolicy {
        ArchivePolicy {
            root: LaunchInputKind::Pack,
            max_compressed_bytes: 1024 * 1024,
            max_expanded_bytes: 1024 * 1024,
            max_files: 100,
        }
    }
    fn convert(
        path: &Path,
        pin: &ReleaseInputPin,
        manifest: &PackReleaseManifest,
    ) -> Result<PreparedPackInput> {
        release_zip(
            File::open(path).unwrap(),
            pin,
            manifest,
            pack_format::Limits::default(),
            &policy(),
        )
    }
    fn pin_bytes(bytes: &[u8]) -> ReleaseInputPin {
        ReleaseInputPin {
            pack_ref: "owned".into(),
            size_bytes: bytes.len() as u64,
            sha256: Sha256Digest::of_bytes(bytes),
        }
    }
    #[test]
    fn primary_runtime_release_preserves_core_directory_variants_and_exact_payload() {
        let (_root, path, pin, manifest) = fixture();
        let mut ready = convert(&path, &pin, &manifest).unwrap();
        let mut zip = zip::ZipArchive::new(ready.file.as_file_mut()).unwrap();
        let mut data = Vec::new();
        zip.by_name("payload")
            .unwrap()
            .read_to_end(&mut data)
            .unwrap();
        assert_eq!(data, b"exact bytes\r\n");
        for variant in ["linux-amd64", "linux-arm64"] {
            let name = format!("actions/bin/echo/{variant}");
            let mut file = zip.by_name(&name).unwrap();
            let mut bytes = String::new();
            file.read_to_string(&mut bytes).unwrap();
            assert_eq!(bytes, format!("owned static {variant} bytes\n"));
            #[cfg(unix)]
            assert_eq!(file.unix_mode().unwrap() & 0o777, 0o755);
        }
        assert_eq!(zip.len(), manifest.files.len());
        assert!(zip.by_name("owned/payload").is_err());
        assert!(
            zip.by_name(pack_format::MANIFEST_PATH).is_err(),
            "converter must not invent an offline-format manifest"
        );
    }
    #[test]
    fn captured_manifest_and_source_pin_must_match_every_byte_and_intent() {
        let (_root, path, pin, manifest) = fixture();
        let mut wrong = pin.clone();
        wrong.sha256 = Sha256Digest::of_bytes(b"wrong");
        assert!(convert(&path, &wrong, &manifest).is_err());
        let mut extra = fs::read(&path).unwrap();
        extra.push(0);
        assert!(release_zip(
            extra.as_slice(),
            &pin,
            &manifest,
            pack_format::Limits::default(),
            &policy()
        )
        .is_err());
        for case in 0..7 {
            let mut wrong = manifest.clone();
            match case {
                0 => wrong.format_version = 2,
                1 => wrong.pack_ref = "other".into(),
                2 => {
                    wrong.files.pop();
                }
                3 => wrong.files[0].sha256 = "a".repeat(64),
                4 => wrong.files[0].size += 1,
                5 => wrong.files[0].executable = !wrong.files[0].executable,
                _ => {
                    let file = wrong.files[0].clone();
                    wrong.files.insert(0, file);
                }
            }
            assert!(
                convert(&path, &pin, &wrong).is_err(),
                "manifest mutation{case}"
            );
        }
    }
    #[test]
    fn authenticated_trailing_bytes_second_members_bad_crc_and_hidden_tar_data_fail() {
        let (_root, path, _pin, manifest) = fixture();
        let original = fs::read(path).unwrap();
        let mut trailing = original.clone();
        trailing.push(0);
        let mut concatenated = original.clone();
        concatenated.extend_from_slice(&original);
        let mut bad_crc = original.clone();
        let index = bad_crc.len() - 8;
        bad_crc[index] ^= 0xff;
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(original.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        let mut cases = vec![trailing, concatenated, bad_crc];
        for tail in [b"hidden-tar-record".as_slice(), vec![0; 2048].as_slice()] {
            let mut encoded =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoded.write_all(&decoded).unwrap();
            encoded.write_all(tail).unwrap();
            cases.push(encoded.finish().unwrap());
        }
        for bytes in cases {
            assert!(release_zip(
                bytes.as_slice(),
                &pin_bytes(&bytes),
                &manifest,
                pack_format::Limits::default(),
                &policy()
            )
            .is_err());
        }
    }
    fn hostile_archive(name: &str, kind: tar::EntryType, mode: u32, data: &[u8]) -> Vec<u8> {
        let encoded = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(encoded);
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_entry_type(kind);
        // Independent malicious producer bypasses Builder's path sanitizer.
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        if kind.is_symlink() || kind.is_hard_link() {
            header.set_link_name("../../outside").unwrap();
        }
        header.set_cksum();
        archive.append(&header, Cursor::new(data)).unwrap();
        archive.finish().unwrap();
        archive.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn traversal_aliases_special_files_and_wrong_modes_fail_with_authenticated_source() {
        let manifest = PackReleaseManifest {
            format_version: 1,
            pack_ref: "owned".into(),
            version: "1.0.0".into(),
            files: vec![PackReleaseFile {
                path: "payload".into(),
                size: 5,
                sha256: Sha256Digest::of_bytes(b"owned").as_str().into(),
                executable: false,
            }],
        };
        for (name, kind, mode) in [
            ("owned/../outside", tar::EntryType::Regular, 0o644),
            ("other/payload", tar::EntryType::Regular, 0o644),
            ("owned/payload", tar::EntryType::Symlink, 0o644),
            ("owned/payload", tar::EntryType::Link, 0o644),
            ("owned/payload", tar::EntryType::Fifo, 0o644),
            ("owned/payload", tar::EntryType::Regular, 0o4755),
            ("owned/payload", tar::EntryType::Regular, 0o755),
        ] {
            let bytes = hostile_archive(name, kind, mode, b"owned");
            assert!(release_zip(
                bytes.as_slice(),
                &pin_bytes(&bytes),
                &manifest,
                pack_format::Limits::default(),
                &policy()
            )
            .is_err());
        }
    }
    #[test]
    fn archive_output_and_expansion_budgets_fail_closed() {
        let (_root, path, pin, manifest) = fixture();
        for case in 0..5 {
            let mut limits = pack_format::Limits::default();
            let mut policy = policy();
            match case {
                0 => limits.compressed_bytes = pin.size_bytes - 1,
                1 => limits.file_bytes = 1,
                2 => limits.manifest_bytes = 1,
                3 => policy.max_files = 1,
                _ => policy.max_compressed_bytes = 32,
            }
            assert!(
                release_zip(File::open(&path).unwrap(), &pin, &manifest, limits, &policy).is_err()
            );
        }
    }
    #[test]
    fn oversized_or_unmatched_gnu_metadata_fails_before_allocation_or_publication() {
        let (_root, _path, _pin, manifest) = fixture();
        for size in [10_000u64, 1u64 << 40] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::GNULongName);
            header.set_path("././@LongLink").unwrap();
            header.set_size(size);
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            let mut encoded =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoded.write_all(header.as_bytes()).unwrap();
            encoded.write_all(&[0; 1024]).unwrap();
            let bytes = encoded.finish().unwrap();
            assert!(release_zip(
                bytes.as_slice(),
                &pin_bytes(&bytes),
                &manifest,
                pack_format::Limits::default(),
                &policy()
            )
            .is_err());
        }
    }
    #[test]
    fn offline_cli_archive_is_not_an_alternate_runtime_reader() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("pack.yaml"), "ref: owned\nversion: 1.0.0\n").unwrap();
        let published = PackStorage::new(root.path().join("runtime"))
            .publish_release(&source, "owned", "1.0.0")
            .unwrap();
        let archive = root.path().join("offline.tar.gz");
        let offline = pack_format::build(
            &pack_format::BuildOptions {
                source,
                artifacts: vec![],
                executables: vec![],
            },
            &archive,
            pack_format::Limits::default(),
        )
        .unwrap();
        let pin = ReleaseInputPin {
            pack_ref: "owned".into(),
            size_bytes: offline.compressed_size,
            sha256: Sha256Digest::parse(offline.sha256).unwrap(),
        };
        assert!(convert(&archive, &pin, &published.manifest).is_err());
    }
}
