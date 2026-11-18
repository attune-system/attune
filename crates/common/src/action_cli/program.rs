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
