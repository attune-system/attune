#![cfg(target_os = "linux")]
use attune_common::{
    action_cli::{select_program, verify_selected_program},
    child_process_environment::ChildProcessEnvironment,
    pack_format::{LaunchSpec, Manifest},
};
use serde_json::json;
use std::{ffi::OsString, path::Path};

fn environment(path: &str) -> ChildProcessEnvironment {
    ChildProcessEnvironment::from_parent([(OsString::from("PATH"), OsString::from(path))], &[])
        .unwrap()
}
fn command(program: &str) -> LaunchSpec {
    serde_json::from_value(json!({"type":"command","program":program})).unwrap()
}

#[test]
fn installed_program_uses_only_absolute_baseline_search_paths() {
    let release = tempfile::tempdir().unwrap();
    for program in ["./printf", "printf/", "printf/.", "bin/printf"] {
        assert!(command(program)
            .validate_action_source(
                None,
                attune_common::models::ParameterDelivery::None,
                attune_common::models::ParameterFormat::Json
            )
            .is_err());
    }
    let selected = select_program(
        &command("printf"),
        release.path(),
        None,
        "tools.run",
        &environment(":relative:/usr/bin"),
    )
    .unwrap();
    assert_eq!(
        selected.path,
        Path::new("/usr/bin/printf").canonicalize().unwrap()
    );
    verify_selected_program(&selected).unwrap();
    assert!(select_program(
        &command("printf"),
        release.path(),
        None,
        "tools.run",
        &environment(":relative")
    )
    .is_err());
    assert!(select_program(
        &command("./printf"),
        release.path(),
        None,
        "tools.run",
        &environment("/usr/bin")
    )
    .is_err());
}

#[test]
fn command_cannot_bypass_pack_integrity_or_follow_symlink_into_release() {
    use std::os::unix::fs::symlink;
    let release = tempfile::tempdir().unwrap();
    std::fs::copy("/usr/bin/printf", release.path().join("probe")).unwrap();
    let tools = tempfile::tempdir().unwrap();
    symlink(release.path().join("probe"), tools.path().join("probe")).unwrap();
    assert!(select_program(
        &command(tools.path().join("probe").to_str().unwrap()),
        release.path(),
        None,
        "tools.run",
        &environment("/usr/bin")
    )
    .is_err());
}

#[test]
fn packaged_program_requires_consumer_platform_mode_and_hash() {
    use sha2::{Digest, Sha256};
    let release = tempfile::tempdir().unwrap();
    let path = release.path().join("probe");
    std::fs::copy("/usr/bin/printf", &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let manifest: Manifest = serde_json::from_value(json!({
        "schema":"attune.pack.release/v1","pack":{"ref":"tools","version":"1.0.0"},
        "requires":{"attune":"*","platform_catalog":"v1","runtimes":["native"]},"dependencies":{},
        "files":[{"path":"probe","size":bytes.len(),"sha256":hash,"mode":"0755"}],
        "artifacts":[{"kind":"native","id":"probe","component_refs":["tools.run"],"variants":[{"id":"linux","path":"probe","target":{"os":"linux","libc":"static","arch":if std::env::consts::ARCH=="x86_64" {"amd64"}else{"arm64"}}}]}],
        "evidence":{"sboms":[],"provenance":[]}
    })).unwrap();
    let launch = serde_json::from_value(json!({"type":"native","artifact":"probe"})).unwrap();
    let selected = select_program(
        &launch,
        release.path(),
        Some(&manifest),
        "tools.run",
        &environment("/usr/bin"),
    )
    .unwrap();
    assert_eq!(selected.artifact.as_deref(), Some("probe"));
    assert!(select_program(
        &launch,
        release.path(),
        Some(&manifest),
        "tools.foreign",
        &environment("/usr/bin")
    )
    .is_err());
    let mut invalid = manifest.clone();
    invalid.files[0].sha256 = "0".repeat(64);
    assert!(select_program(
        &launch,
        release.path(),
        Some(&invalid),
        "tools.run",
        &environment("/usr/bin")
    )
    .is_err());
    let mut wrong_platform = manifest.clone();
    if let attune_common::pack_format::Artifact::Native { variants, .. } =
        &mut wrong_platform.artifacts[0]
    {
        variants[0].target.arch = if std::env::consts::ARCH == "x86_64" {
            attune_common::pack_format::NativeArch::Arm64
        } else {
            attune_common::pack_format::NativeArch::Amd64
        };
    }
    assert!(select_program(
        &launch,
        release.path(),
        Some(&wrong_platform),
        "tools.run",
        &environment("/usr/bin")
    )
    .is_err());
    let mut invalid = manifest;
    invalid.files[0].mode = attune_common::pack_format::FileMode::Data;
    assert!(select_program(
        &launch,
        release.path(),
        Some(&invalid),
        "tools.run",
        &environment("/usr/bin")
    )
    .is_err());
    std::fs::write(&path, b"changed executable").unwrap();
    assert!(verify_selected_program(&selected).is_err());
}

#[test]
fn writable_executable_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let release = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let path = tools.path().join("probe");
    std::fs::copy("/usr/bin/printf", &path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(select_program(
        &command(path.to_str().unwrap()),
        release.path(),
        None,
        "tools.run",
        &environment("/usr/bin")
    )
    .is_err());
}
