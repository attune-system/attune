//! Local acceptance artifact assembly. Run only in the central permissioned lane.
use attune_common::pack_registry::PackStorage;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err(
            "Usage: bootstrap_acceptance_archive SOURCE_CORE OWNED_OUTPUT_DIRECTORY".into(),
        );
    }
    let source = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    if output.exists() {
        return Err("Output must not exist; never adopt or overwrite an earlier run".into());
    }
    for (architecture, machine) in [("linux-amd64", 62_u16), ("linux-arm64", 183_u16)] {
        let path = source
            .join("sensors/bin")
            .join(architecture)
            .join("attune-core-timer-sensor");
        let bytes = std::fs::read(&path)?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&path)?.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        if bytes.len() < 20
            || &bytes[..4] != b"\x7fELF"
            || bytes[4] != 2
            || bytes[5] != 1
            || u16::from_le_bytes([bytes[18], bytes[19]]) != machine
            || !executable
        {
            return Err(format!("Missing real executable {architecture} ELF input").into());
        }
    }
    let metadata: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(source.join("pack.yaml"))?)?;
    if metadata["ref"] != "core" {
        return Err("Expected actual core source".into());
    }
    let version = metadata["version"].as_str().ok_or("Missing core version")?;
    // This is the SAME publisher/normalizer as API admission, not the Python
    // upload tarball or an OCI manifest digest.
    let published = PackStorage::new(output).publish_release(&source, "core", version)?;
    println!(
        "{}",
        serde_json::json!({
            "version": version,
            "digest": published.digest,
            "archive_path": published.archive_path,
            "pack_path": published.pack_path,
            "archive_size": published.archive_size,
        })
    );
    Ok(())
}
