//! Offline format-1 releases. These blocking operations use private temporary
//! storage and bounded streaming buffers; async callers must use spawn_blocking.
mod archive;
mod jar;
mod launch;
mod paths;
mod source;
mod types;

pub use types::*;

use anyhow::{ensure, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const MANIFEST_PATH: &str = ".attune/release-manifest.json";
pub const MEDIA_TYPE: &str = "application/vnd.attune.pack.v1+tar+gzip";
pub const ENCODING_PROFILE: &str = "ustar-miniz_oxide-0.9.1-level9-unicode15.1-v1";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Format-level budgets. Cache concurrency and capacity belong to the host.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub compressed_bytes: u64,
    pub extracted_bytes: u64,
    pub file_bytes: u64,
    pub entries: u64,
    pub manifest_bytes: u64,
    pub artifacts: u64,
    pub variants_per_artifact: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            compressed_bytes: 512 << 20,
            extracted_bytes: 1 << 30,
            file_bytes: 256 << 20,
            entries: 50_000,
            manifest_bytes: 16 << 20,
            artifacts: 1_024,
            variants_per_artifact: 16,
        }
    }
}

pub(super) fn limit(name: &str, observed: u64, maximum: u64) -> Result<()> {
    ensure!(
        observed <= maximum,
        "release_limit_exceeded: {name}: {observed} > {maximum}"
    );
    Ok(())
}

#[derive(Debug, Clone)]
pub enum ArtifactInput {
    Native {
        id: String,
        target: NativeTarget,
        source: PathBuf,
    },
    Jar {
        id: String,
        source: PathBuf,
    },
}

impl std::str::FromStr for ArtifactInput {
    type Err = anyhow::Error;
    fn from_str(mapping: &str) -> Result<Self> {
        let (key, file) = mapping.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("artifact mapping must be ID[linux/ARCH/static]=FILE or ID=JAR")
        })?;
        ensure!(!file.is_empty(), "artifact source path is empty");
        if let Some((id, selector)) = key.split_once('[') {
            paths::id(id)?;
            let selector = selector
                .strip_suffix(']')
                .ok_or_else(|| anyhow::anyhow!("missing ] in artifact target"))?;
            let parts: Vec<_> = selector.split('/').collect();
            ensure!(
                parts.len() == 3 && parts[0] == "linux" && parts[2] == "static",
                "expected linux/amd64/static or linux/arm64/static"
            );
            let arch = match parts[1] {
                "amd64" => NativeArch::Amd64,
                "arm64" => NativeArch::Arm64,
                _ => anyhow::bail!("unsupported native architecture"),
            };
            Ok(Self::Native {
                id: id.into(),
                target: NativeTarget {
                    os: NativeOs::Linux,
                    arch,
                    libc: NativeLibc::Static,
                },
                source: file.into(),
            })
        } else {
            paths::id(key)?;
            Ok(Self::Jar {
                id: key.into(),
                source: file.into(),
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub source: PathBuf,
    pub artifacts: Vec<ArtifactInput>,
    pub executables: Vec<String>,
}

/// The manifest is accessible only after archive, inventory, and launch checks.
#[derive(Debug, Serialize)]
pub struct VerifiedRelease {
    manifest: Manifest,
    pub sha256: String,
    pub compressed_size: u64,
    pub extracted_size: u64,
    pub largest_file: u64,
    pub entry_count: u64,
}

impl VerifiedRelease {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

/// Build atomically without replacing an existing destination.
pub fn build(
    options: &BuildOptions,
    destination: &Path,
    limits: Limits,
) -> Result<VerifiedRelease> {
    let snapshot = tempfile::tempdir()?;
    let manifest = source::collect(options, snapshot.path(), limits)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    archive::write(snapshot.path(), &manifest, output.as_file_mut(), limits)?;
    let verified = verify(output.path(), limits)?;
    output.persist_noclobber(destination)?;
    Ok(verified)
}

/// Check exact bytes offline. Never executes pack content or contacts a server.
pub fn verify(archive: &Path, limits: Limits) -> Result<VerifiedRelease> {
    archive::verify(archive, limits)
}

/// Inspection performs full verification, not an untrusted manifest peek.
pub fn inspect(archive: &Path, limits: Limits) -> Result<VerifiedRelease> {
    verify(archive, limits)
}

#[cfg(test)]
mod tests;
