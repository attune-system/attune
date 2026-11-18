//! Blocking executable selection over an already verified release and baseline.
//! Async callers must use spawn_blocking; no ambient PATH or executable download.
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::child_process_environment::ChildProcessEnvironment;
use crate::pack_format::{Artifact, FileMode, LaunchSpec, Manifest, NativeArch, NativeOs};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedProgram {
    pub path: PathBuf,
    pub sha256: String,
    /// Static invocation spelling preserves multi-call binary dispatch after canonicalization.
    pub invocation_name: String,
    pub artifact: Option<String>,
    pub variant: Option<String>,
}

impl SelectedProgram {
    /// Account for the longer of exec path and fixed argv[0] spelling.
    pub fn argument_budget_program(&self) -> Result<&str> {
        let path = self.path.to_str().ok_or_else(unavailable)?;
        Ok(if path.len() >= self.invocation_name.len() {
            path
        } else {
            &self.invocation_name
        })
    }
}

fn unavailable() -> Error {
    Error::validation("action.launch.program_unavailable")
}

pub fn select_program(
    launch: &LaunchSpec,
    release_root: &Path,
    manifest: Option<&Manifest>,
    action_ref: &str,
    environment: &ChildProcessEnvironment,
) -> Result<SelectedProgram> {
    let root = release_root.canonicalize().map_err(|_| unavailable())?;
    let (path, artifact, variant, expected) = match launch {
        LaunchSpec::Command { program, .. } => {
            let requested = Path::new(program);
            let candidate = if requested.is_absolute() {
                requested.to_owned()
            } else {
                if program.contains('/') || matches!(program.as_str(), "." | "..") {
                    return Err(unavailable());
                }
                environment
                    .get("PATH")
                    .into_iter()
                    .flat_map(std::env::split_paths)
                    .filter(|directory| directory.is_absolute())
                    .map(|directory| directory.join(program))
                    .find(|candidate| executable(candidate).is_ok())
                    .ok_or_else(unavailable)?
            };
            let selected = candidate.canonicalize().map_err(|_| unavailable())?;
            if selected.starts_with(&root) {
                return Err(unavailable());
            }
            executable(&selected)?;
            (selected, None, None, None)
        }
        LaunchSpec::Native { artifact, .. } => {
            let manifest = manifest.ok_or_else(unavailable)?;
            let variants = manifest
                .artifacts
                .iter()
                .find_map(|entry| match entry {
                    Artifact::Native {
                        id,
                        variants,
                        component_refs,
                    } if id == artifact && component_refs.iter().any(|r| r == action_ref) => {
                        Some(variants)
                    }
                    _ => None,
                })
                .ok_or_else(unavailable)?;
            let arch = match std::env::consts::ARCH {
                "x86_64" => NativeArch::Amd64,
                "aarch64" => NativeArch::Arm64,
                _ => return Err(unavailable()),
            };
            if std::env::consts::OS != "linux" {
                return Err(unavailable());
            }
            let mut compatible = variants.iter().filter(|variant| {
                variant.target.os == NativeOs::Linux && variant.target.arch == arch
            });
            let selected = compatible.next().ok_or_else(unavailable)?;
            if compatible.next().is_some() {
                return Err(unavailable());
            }
            if !Path::new(&selected.path)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            {
                return Err(unavailable());
            }
            let entry = manifest
                .files
                .iter()
                .find(|file| file.path == selected.path && file.mode == FileMode::Executable)
                .ok_or_else(unavailable)?;
            let joined = root.join(&selected.path);
            let actual = joined.canonicalize().map_err(|_| unavailable())?;
            if actual != joined || !actual.starts_with(&root) {
                return Err(unavailable());
            }
            executable(&actual)?;
            (
                actual,
                Some(artifact.clone()),
                Some(selected.id.clone()),
                Some(entry.sha256.clone()),
            )
        }
        _ => return Err(unavailable()),
    };
    let sha256 = hash_program(&path)?;
    if expected
        .as_ref()
        .is_some_and(|expected| expected != &sha256)
    {
        return Err(unavailable());
    }
    let invocation_name = match launch {
        LaunchSpec::Command { program, .. } => program.clone(),
        _ => path.to_str().ok_or_else(unavailable)?.to_owned(),
    };
    Ok(SelectedProgram {
        invocation_name,
        path,
        sha256,
        artifact,
        variant,
    })
}

pub fn verify_selected_program(selected: &SelectedProgram) -> Result<()> {
    executable(&selected.path)?;
    if selected.path.canonicalize().map_err(|_| unavailable())? != selected.path
        || hash_program(&selected.path)? != selected.sha256
    {
        return Err(unavailable());
    }
    Ok(())
}

fn executable(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path).map_err(|_| unavailable())?;
    if !metadata.is_file() {
        return Err(unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(unavailable());
        }
    }
    Ok(())
}

fn hash_program(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|_| unavailable())?;
    let maximum = crate::pack_format::Limits::default().file_bytes;
    if file.metadata().map_err(|_| unavailable())?.len() > maximum {
        return Err(unavailable());
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|_| unavailable())?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(unavailable)?;
        if total > maximum {
            return Err(unavailable());
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Select typed native metadata from a primary runtime-v1 release inventory.
/// The embedded component manifest is authenticated DATA, not an alternative
/// archive reader. Program bytes remain selected by the immutable Pack pin and
/// are verified again by the credential-free Runner in the admitted image.
fn select_native_metadata(
    launch: &LaunchSpec,
    release: &crate::pack_registry::storage::PackReleaseManifest,
    metadata: &[u8],
    action_ref: &str,
    platform: crate::workload_launch::LaunchPlatform,
) -> Result<SelectedProgram> {
    use crate::pack_format::{NativeArch, NativeOs, MANIFEST_PATH};
    use crate::workload_launch::{Sha256Digest, PACK_ROOT};
    let LaunchSpec::Native { artifact, .. } = launch else {
        return Err(unavailable());
    };
    if release.format_version != 1
        || metadata.len() as u64 > crate::pack_format::Limits::default().manifest_bytes
    {
        return Err(unavailable());
    }
    let mut declared = release
        .files
        .iter()
        .filter(|file| file.path == MANIFEST_PATH);
    let declared = declared
        .next()
        .filter(|file| {
            !file.executable
                && file.size == metadata.len() as u64
                && Sha256Digest::of_bytes(metadata).as_str() == file.sha256
        })
        .ok_or_else(unavailable)?;
    // Both primary inventory and component metadata must be unambiguous.
    if release
        .files
        .iter()
        .filter(|file| file.path == declared.path)
        .count()
        != 1
    {
        return Err(unavailable());
    }
    let manifest: Manifest = serde_json::from_slice(metadata).map_err(|_| unavailable())?;
    let limits = crate::pack_format::Limits::default();
    if manifest.files.len() as u64 > limits.entries
        || manifest.artifacts.len() as u64 > limits.artifacts
        || manifest.pack.r#ref != release.pack_ref
        || manifest.pack.version != release.version
    {
        return Err(unavailable());
    }
    let mut artifacts = manifest
        .artifacts
        .iter()
        .filter(|entry| entry.id() == artifact);
    let Some(Artifact::Native {
        component_refs,
        variants,
        ..
    }) = artifacts.next()
    else {
        return Err(unavailable());
    };
    if artifacts.next().is_some()
        || variants.len() as u64 > limits.variants_per_artifact
        || !component_refs
            .iter()
            .any(|reference| reference == action_ref)
    {
        return Err(unavailable());
    }
    let arch = match platform {
        crate::workload_launch::LaunchPlatform::LinuxAmd64 => NativeArch::Amd64,
        crate::workload_launch::LaunchPlatform::LinuxArm64 => NativeArch::Arm64,
    };
    let mut variants = variants
        .iter()
        .filter(|variant| variant.target.os == NativeOs::Linux && variant.target.arch == arch);
    let variant = variants.next().ok_or_else(unavailable)?;
    if variants.next().is_some() {
        return Err(unavailable());
    }
    let relative = crate::artifact_transport::ValidatedRelativePath::new(&variant.path)
        .map_err(|_| unavailable())?;
    let mut files = manifest
        .files
        .iter()
        .filter(|file| file.path == variant.path);
    let file = files
        .next()
        .filter(|file| file.mode == FileMode::Executable)
        .ok_or_else(unavailable)?;
    if files.next().is_some() {
        return Err(unavailable());
    }
    let mut primary = release
        .files
        .iter()
        .filter(|entry| entry.path == variant.path);
    let primary_file = primary
        .next()
        .filter(|entry| entry.executable && entry.size == file.size && entry.sha256 == file.sha256)
        .ok_or_else(unavailable)?;
    Sha256Digest::parse(&primary_file.sha256).map_err(|_| unavailable())?;
    if primary.next().is_some() {
        return Err(unavailable());
    }
    let path = PathBuf::from(format!("{PACK_ROOT}/{}", relative.as_str()));
    Ok(SelectedProgram {
        invocation_name: path.to_str().ok_or_else(unavailable)?.to_owned(),
        path,
        sha256: file.sha256.clone(),
        artifact: Some(artifact.clone()),
        variant: Some(variant.id.clone()),
    })
}

/// Read only the fixed component metadata from an already pinned canonical
/// guest ZIP. Primary runtime-v1 remains the sole source archive contract.
pub fn select_released_native<R: Read + std::io::Seek>(
    launch: &LaunchSpec,
    release: &crate::pack_registry::storage::PackReleaseManifest,
    pinned_zip: R,
    action_ref: &str,
    platform: crate::workload_launch::LaunchPlatform,
) -> Result<SelectedProgram> {
    let mut archive = zip::ZipArchive::new(pinned_zip).map_err(|_| unavailable())?;
    let metadata = archive
        .by_name(crate::pack_format::MANIFEST_PATH)
        .map_err(|_| unavailable())?;
    let maximum = crate::pack_format::Limits::default().manifest_bytes;
    if !metadata.is_file() || metadata.size() > maximum {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    metadata
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    select_native_metadata(launch, release, &bytes, action_ref, platform)
}

#[cfg(test)]
mod released_native_tests {
    use super::*;
    use crate::{
        pack_registry::storage::{PackReleaseFile, PackReleaseManifest},
        workload_launch::{LaunchPlatform, Sha256Digest},
    };
    use serde_json::{json, Value};
    use std::io::{Cursor, Write};

    fn component() -> Value {
        json!({
            "schema":"attune.pack.release/v1", "pack":{"ref":"owned","version":"1.2.3"},
            "requires":{"attune":"*","platform_catalog":"v1","runtimes":["native"]},
            "dependencies":{}, "evidence":{"sboms":[],"provenance":[]},
            "files":[
                {"path":"bin/amd64","size":4,"sha256":Sha256Digest::of_bytes(b"amd!").as_str(),"mode":"0755"},
                {"path":"bin/arm64","size":4,"sha256":Sha256Digest::of_bytes(b"arm!").as_str(),"mode":"0755"}
            ],
            "artifacts":[{"kind":"native","id":"app","component_refs":["owned.run"],"variants":[
                {"id":"linux-amd64-static","path":"bin/amd64","target":{"os":"linux","arch":"amd64","libc":"static"}},
                {"id":"linux-arm64-static","path":"bin/arm64","target":{"os":"linux","arch":"arm64","libc":"static"}}
            ]}]
        })
    }
    fn release(metadata: &Value) -> (PackReleaseManifest, Cursor<Vec<u8>>) {
        let bytes = serde_json::to_vec(metadata).unwrap();
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                crate::pack_format::MANIFEST_PATH,
                zip::write::SimpleFileOptions::default().unix_permissions(0o644),
            )
            .unwrap();
        archive.write_all(&bytes).unwrap();
        for (path, bytes) in [("bin/amd64", b"amd!"), ("bin/arm64", b"arm!")] {
            archive
                .start_file(
                    path,
                    zip::write::SimpleFileOptions::default().unix_permissions(0o755),
                )
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        let archive = archive.finish().unwrap().into_inner();
        (
            PackReleaseManifest {
                format_version: 1,
                pack_ref: "owned".into(),
                version: "1.2.3".into(),
                files: vec![
                    PackReleaseFile {
                        path: crate::pack_format::MANIFEST_PATH.into(),
                        size: bytes.len() as u64,
                        sha256: Sha256Digest::of_bytes(&bytes).as_str().into(),
                        executable: false,
                    },
                    PackReleaseFile {
                        path: "bin/amd64".into(),
                        size: 4,
                        sha256: Sha256Digest::of_bytes(b"amd!").as_str().into(),
                        executable: true,
                    },
                    PackReleaseFile {
                        path: "bin/arm64".into(),
                        size: 4,
                        sha256: Sha256Digest::of_bytes(b"arm!").as_str().into(),
                        executable: true,
                    },
                ],
            },
            Cursor::new(archive),
        )
    }
    fn launch() -> LaunchSpec {
        LaunchSpec::Native {
            artifact: "app".into(),
            arguments: Vec::new(),
        }
    }
    #[test]
    fn primary_inventory_selects_admitted_platform_without_host_lookup() {
        for (platform, path, bytes) in [
            (
                LaunchPlatform::LinuxAmd64,
                "/opt/attune/pack/bin/amd64",
                b"amd!",
            ),
            (
                LaunchPlatform::LinuxArm64,
                "/opt/attune/pack/bin/arm64",
                b"arm!",
            ),
        ] {
            let (primary, archive) = release(&component());
            let selected =
                select_released_native(&launch(), &primary, archive, "owned.run", platform)
                    .unwrap();
            assert_eq!(selected.path, PathBuf::from(path));
            assert_eq!(selected.invocation_name, path);
            assert_eq!(selected.sha256, Sha256Digest::of_bytes(bytes).as_str());
            assert_eq!(selected.artifact.as_deref(), Some("app"));
        }
    }
    #[test]
    fn metadata_requires_exact_primary_size_hash_pack_and_version() {
        for change in 0..4 {
            let (mut primary, archive) = release(&component());
            match change {
                0 => primary.files[0].sha256 = Sha256Digest::of_bytes(b"different").as_str().into(),
                1 => primary.files[0].size += 1,
                2 => primary.pack_ref = "other".into(),
                _ => primary.version = "1.2.4".into(),
            }
            assert!(select_released_native(
                &launch(),
                &primary,
                archive,
                "owned.run",
                LaunchPlatform::LinuxAmd64
            )
            .is_err());
        }
    }
    #[test]
    fn variant_budget_is_enforced_before_platform_selection() {
        let mut metadata = component();
        let mut variants = vec![metadata["artifacts"][0]["variants"][0].clone()];
        for index in 0..crate::pack_format::Limits::default().variants_per_artifact {
            let mut other = metadata["artifacts"][0]["variants"][1].clone();
            other["id"] = json!(format!("unselected-{index}"));
            variants.push(other);
        }
        metadata["artifacts"][0]["variants"] = json!(variants);
        let (primary, archive) = release(&metadata);
        assert!(select_released_native(
            &launch(),
            &primary,
            archive,
            "owned.run",
            LaunchPlatform::LinuxAmd64
        )
        .is_err());
    }

    #[test]
    fn selected_component_variant_and_executable_inventory_cannot_be_substituted() {
        for change in 0..6 {
            let mut metadata = component();
            match change {
                0 => metadata["artifacts"][0]["component_refs"] = json!(["owned.other"]),
                1 => metadata["artifacts"][0]["variants"][0]["path"] = json!("../escape"),
                2 => metadata["artifacts"][0]["variants"][0]["target"]["arch"] = json!("arm64"),
                3 => {
                    metadata["files"][0]["sha256"] =
                        json!(Sha256Digest::of_bytes(b"other").as_str())
                }
                4 => metadata["files"][0]["mode"] = json!("0644"),
                _ => {
                    let duplicate = metadata["artifacts"][0]["variants"][0].clone();
                    metadata["artifacts"][0]["variants"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
            }
            // Reauthenticate the changed metadata, so rejection exercises the
            // component/platform/inventory binding rather than only its hash.
            let (primary, archive) = release(&metadata);
            assert!(select_released_native(
                &launch(),
                &primary,
                archive,
                "owned.run",
                LaunchPlatform::LinuxAmd64
            )
            .is_err());
        }
    }
}
