//! Canonical packaging only. Managed installation remains an ordinary API upload.
use attune_common::pack_registry::PackStorage;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: native_producer_acceptance_archive SOURCE_PACK NEW_STORE".into());
    }
    let source = PathBuf::from(&args[0]);
    let store = PathBuf::from(&args[1]);
    if store.exists() {
        return Err("Never adopt an existing native producer archive store".into());
    }
    for path in ["pack.yaml", "actions/producer.yaml", "actions/producer.py"] {
        if !std::fs::symlink_metadata(source.join(path))?
            .file_type()
            .is_file()
        {
            return Err("Expected the actual regular native producer fixture files".into());
        }
    }
    let metadata: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(source.join("pack.yaml"))?)?;
    if metadata["ref"] != "f_native_settlement" {
        return Err("Expected F's actual managed producer pack".into());
    }
    let version = metadata["version"]
        .as_str()
        .ok_or("Missing producer version")?;
    let release =
        PackStorage::new(store).publish_release(&source, "f_native_settlement", version)?;
    println!(
        "{}",
        serde_json::json!({"ref": "f_native_settlement", "version": version, "digest": release.digest, "archive_path": release.archive_path})
    );
    Ok(())
}
