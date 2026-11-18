//! Trusted CP materialization of an exact format-1 release as a launch ZIP.
//! No mutable unpacked tree or host cache pathname is accepted. Callers own
//! pinned acquisition and cancellation-safe blocking work. Selected secrets,
//! tokens and stdin belong exclusively in the encrypted frozen context.
use crate::{
    pack_format,
    workload_archive::{self, ArchivePolicy},
    workload_launch::{LaunchInputKind, Sha256Digest},
    Error, Result,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
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

/// Blocking and bounded. Verify a private copy, then stream its exact regular
/// file inventory without extraction. Strip only the verified pack-ref prefix;
/// preserve payloads and executable modes, including both native platforms.
pub fn release_zip<R: Read>(
    source: R,
    pin: &ReleaseInputPin,
    release_limits: pack_format::Limits,
    policy: &ArchivePolicy,
) -> Result<PreparedPackInput> {
    release_zip_inner(source, pin, release_limits, policy).map_err(|_| invalid())
}

fn release_zip_inner<R: Read>(
    source: R,
    pin: &ReleaseInputPin,
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
    let verified = pack_format::verify(private.path(), release_limits).map_err(|_| invalid())?;
    let manifest = verified.manifest();
    if manifest.pack.r#ref != pin.pack_ref || verified.sha256 != pin.sha256.as_str() {
        return Err(invalid().into());
    }
    let manifest_bytes = serde_jcs::to_vec(manifest).map_err(|_| invalid())?;
    let mut expected = BTreeMap::new();
    for file in &manifest.files {
        if expected
            .insert(
                file.path.clone(),
                (file.size, Sha256Digest::parse(&file.sha256)?, file.mode),
            )
            .is_some()
        {
            return Err(invalid().into());
        }
    }
    if expected
        .insert(
            pack_format::MANIFEST_PATH.into(),
            (
                manifest_bytes.len() as u64,
                Sha256Digest::of_bytes(&manifest_bytes),
                pack_format::FileMode::Data,
            ),
        )
        .is_some()
    {
        return Err(invalid().into());
    }
    let mut output = tempfile::NamedTempFile::new()?;
    private.as_file_mut().rewind()?;
    {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(private.as_file_mut()));
        let bounded = BoundedOutput {
            file: output.as_file_mut(),
            maximum: policy.max_compressed_bytes,
        };
        let mut zip = zip::ZipWriter::new(bounded);
        let prefix = format!("{}/", pin.pack_ref);
        for entry in archive.entries().map_err(|_| invalid())? {
            let mut entry = entry.map_err(|_| invalid())?;
            if !entry.header().entry_type().is_file() {
                return Err(invalid().into());
            }
            let path = entry
                .path()
                .map_err(|_| invalid())?
                .to_str()
                .ok_or_else(invalid)?
                .to_owned();
            let name = path.strip_prefix(&prefix).ok_or_else(invalid)?;
            let (size, sha256, mode) = expected.remove(name).ok_or_else(invalid)?;
            if entry.size() != size {
                return Err(invalid().into());
            }
            let permissions = match mode {
                pack_format::FileMode::Data => 0o644,
                pack_format::FileMode::Executable => 0o755,
            };
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
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
        if !expected.is_empty() {
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
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, ReleaseInputPin) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("pack.yaml"), "ref: owned\nversion: 1.0.0\n").unwrap();
        std::fs::write(source.join("helper"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(source.join("payload"), b"exact bytes\r\n").unwrap();
        let path = root.path().join("release.tar.gz");
        let release = pack_format::build(
            &pack_format::BuildOptions {
                source,
                artifacts: vec![],
                executables: vec!["helper".into()],
            },
            &path,
            pack_format::Limits::default(),
        )
        .unwrap();
        let pin = ReleaseInputPin {
            pack_ref: "owned".into(),
            size_bytes: release.compressed_size,
            sha256: Sha256Digest::parse(release.sha256).unwrap(),
        };
        (root, path, pin)
    }
    fn policy() -> ArchivePolicy {
        ArchivePolicy {
            root: LaunchInputKind::Pack,
            max_compressed_bytes: 1024 * 1024,
            max_expanded_bytes: 1024 * 1024,
            max_files: 100,
        }
    }
    #[test]
    fn exact_release_becomes_fixed_root_zip_with_modes_and_payload_preserved() {
        let (_root, path, pin) = fixture();
        let mut ready = release_zip(
            File::open(&path).unwrap(),
            &pin,
            pack_format::Limits::default(),
            &policy(),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(ready.file.as_file_mut()).unwrap();
        let mut bytes = Vec::new();
        zip.by_name("payload")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"exact bytes\r\n");
        assert_eq!(
            zip.by_name("helper").unwrap().unix_mode().unwrap() & 0o777,
            0o755
        );
        assert!(zip.by_name("owned/payload").is_err());
        assert!(zip.by_name(pack_format::MANIFEST_PATH).is_ok());
    }
    #[test]
    fn wrong_pin_trailing_bytes_and_output_limits_fail_closed() {
        let (_root, path, mut pin) = fixture();
        pin.sha256 = Sha256Digest::of_bytes(b"wrong");
        assert!(release_zip(
            File::open(&path).unwrap(),
            &pin,
            pack_format::Limits::default(),
            &policy()
        )
        .is_err());
        let (_root, path, pin) = fixture();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(0);
        assert!(release_zip(
            bytes.as_slice(),
            &pin,
            pack_format::Limits::default(),
            &policy()
        )
        .is_err());
        let mut tiny = policy();
        tiny.max_compressed_bytes = 32;
        assert!(release_zip(
            File::open(&path).unwrap(),
            &pin,
            pack_format::Limits::default(),
            &tiny
        )
        .is_err());
        let mut tiny = policy();
        tiny.max_files = 1;
        assert!(release_zip(
            File::open(&path).unwrap(),
            &pin,
            pack_format::Limits::default(),
            &tiny
        )
        .is_err());
    }
}
