//! Central-only canonical packaging of E's real G interoperability fixture.
use attune_common::pack_registry::PackStorage;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err(
            "Usage: interoperability_acceptance_archive SOURCE_PACK NEW_OWNED_STORE".into(),
        );
    }
    let source = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    if output.exists() {
        return Err("Never adopt an existing acceptance archive store".into());
    }
    let metadata: serde_json::Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(source.join("pack.yaml"))?)?;
    let reference = metadata["ref"].as_str().ok_or("Missing fixture ref")?;
    let version = metadata["version"]
        .as_str()
        .ok_or("Missing fixture version")?;
    if !reference.starts_with("ginterop_") {
        return Err("Expected the owned interoperability fixture".into());
    }
    for path in [
        "actions/probe.yaml",
        "actions/input_probe.yaml",
        "actions/input_workflow.yaml",
        "actions/emit.yaml",
        "actions/batch_probe.yaml",
        "actions/external_probe.yaml",
        "actions/workflows/input_workflow.workflow.yaml",
        "actions/interoperability_worker.py",
        "actions/interoperability_batch_worker.py",
        "actions/interoperability_emitter.py",
        "queues/inbox.yaml",
        "queues/input_inbox.yaml",
        "rules/input_rule.yaml",
        "triggers/input_tick.yaml",
        "permission_sets/runtime.yaml",
        "permission_sets/runtime_alt.yaml",
        "permission_sets/external_runtime.yaml",
        "permission_sets/emitter.yaml",
        "types/probe_receipt.yaml",
        "types/batch_payload.yaml",
        "types/batch_context.yaml",
        "types/batch_receipt_item.yaml",
    ] {
        if !std::fs::symlink_metadata(source.join(path))?
            .file_type()
            .is_file()
        {
            return Err("Missing full-matrix regular fixture component".into());
        }
    }
    let release = PackStorage::new(output).publish_release(&source, reference, version)?;
    println!(
        "{}",
        serde_json::json!({"digest": release.digest, "archive_path": release.archive_path, "pack_path": release.pack_path, "version": version})
    );
    Ok(())
}
