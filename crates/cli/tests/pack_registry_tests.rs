//! CLI integration tests for pack registry commands
#![allow(deprecated)]

//!
//! This module tests:
//! - `attune pack install` command with all sources
//! - `attune pack checksum` command
//! - `attune pack index-entry` command
//! - `attune pack index-update` command
//! - `attune pack index-merge` command
//! - Error handling and output formatting

use assert_cmd::Command;
use attune_common::pack_registry::{InstallSource, PackIndex, PackIndexEntry};
use predicates::prelude::*;
use serde_json::Value;
use std::fs;

use tempfile::TempDir;

/// Helper to create a test pack directory with pack.yaml
fn create_test_pack(name: &str, version: &str, deps: &[&str]) -> TempDir {
    let temp_dir = TempDir::new().unwrap();

    let deps_yaml = if deps.is_empty() {
        "dependencies: []".to_string()
    } else {
        let dep_list = deps
            .iter()
            .map(|d| format!("  - {}", d))
            .collect::<Vec<_>>()
            .join("\n");
        format!("dependencies:\n{}", dep_list)
    };

    let pack_yaml = format!(
        r#"
ref: {}
name: Test Pack {}
version: {}
description: Test pack for CLI integration tests
author: Test Author
email: test@example.com
license: Apache-2.0
homepage: https://example.com
repository: https://github.com/example/pack
keywords:
  - test
  - cli
{}
python: "3.8"
actions:
  test_action:
    entry_point: test.py
    runner_type: python-script
    description: Test action
sensors:
  test_sensor:
    entry_point: sensor.py
    runner_type: python-script
triggers:
  test_trigger:
    description: Test trigger
"#,
        name, name, version, deps_yaml
    );

    fs::write(temp_dir.path().join("pack.yaml"), pack_yaml).unwrap();
    fs::write(temp_dir.path().join("test.py"), "print('test action')").unwrap();
    fs::write(temp_dir.path().join("sensor.py"), "print('test sensor')").unwrap();

    temp_dir
}

fn create_component_summary_test_pack() -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("pack.yaml"),
        r#"ref: component-test
label: Component Test
version: 1.0.0
description: Component extraction test
author: Test Author
license: Apache-2.0
actions:
  inline_action:
    description: Filesystem inventory must replace this
sensors:
  inline_sensor:
    description: Filesystem inventory must replace this
triggers:
  inline_trigger:
    description: Filesystem inventory must replace this
rules:
  inline_rule:
    description: Filesystem inventory must replace this
workflows:
  inline_workflow:
    description: Filesystem inventory must replace this
"#,
    )
    .unwrap();

    for directory in ["actions", "sensors", "triggers", "rules", "workflows"] {
        fs::create_dir(temp_dir.path().join(directory)).unwrap();
    }
    fs::write(
        temp_dir.path().join("actions/zebra.yaml"),
        "name: zebra\nlabel: Last alphabetically\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("actions/alpha.yml"),
        "ref: component-test.alpha\ndescription: First alphabetically\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("actions/deploy.yaml"),
        "ref: component-test.deploy\ndescription: Deploy the service\nworkflow_file: workflows/deploy.workflow.yaml\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("sensors/monitor.yaml"),
        "name: monitor\ndescription: Monitor events\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("triggers/changed.yaml"),
        "description: Something changed\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("rules/dispatch.yaml"),
        "ref: component-test.dispatch\ndescription: Dispatch an action\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("workflows/nightly.yaml"),
        "ref: component-test.nightly\nlabel: Nightly workflow\n",
    )
    .unwrap();
    temp_dir
}

fn create_documented_manifest_pack() -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("pack.yaml"),
        r#"ref: example
label: Example
name: Legacy Example Name
description: What the pack automates
version: "1.0.0"
author: Example Team
email: team@example.com
runtime_deps: [python, python]
tags: [integration, example]
keywords: [legacy-keyword]
dependencies:
  attune_version: ">=0.1.0"
  python_version: ">=3.11"
  nodejs_version: ">=20"
  packs: [core, core]
meta:
  category: integration
  license: Apache-2.0
  keywords: [meta-keyword]
  documentation_url: https://docs.example.com/attune-pack
  repository_url: https://github.com/example-packs/example
  use_case: Automate Example resources
  tested_attune_versions: [0.1.0]
  support_tier: community
"#,
    )
    .unwrap();
    temp_dir
}

fn assert_sha256(checksum: &str) {
    let hash = checksum.strip_prefix("sha256:").unwrap();
    assert_eq!(hash.len(), 64);
    assert!(hash
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
}

/// Helper to create a registry index file
fn create_test_index(packs: &[(&str, &str)]) -> TempDir {
    let temp_dir = TempDir::new().unwrap();

    let pack_entries: Vec<String> = packs
        .iter()
        .map(|(name, version)| {
            format!(
                r#"{{
                "ref": "{}",
                "label": "Test Pack {}",
                "description": "Test pack",
                "version": "{}",
                "author": "Test",
                "license": "Apache-2.0",
                "keywords": ["test"],
                "runtime_deps": [],
                "install_sources": [
                    {{
                        "type": "git",
                        "url": "https://github.com/test/{}.git",
                        "ref": "v{}",
                        "checksum": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    }}
                ],
                "contents": {{
                    "actions": [],
                    "sensors": [],
                    "triggers": [],
                    "rules": [],
                    "workflows": []
                }}
            }}"#,
                name, name, version, name, version
            )
        })
        .collect();

    let index = format!(
        r#"{{
            "registry_name": "Test Pack Index",
            "registry_url": "https://example.com/pack-index",
            "version": "1.0",
            "last_updated": "2026-08-15T00:00:00Z",
            "packs": [
                {}
            ]
        }}"#,
        pack_entries.join(",\n")
    );

    fs::write(temp_dir.path().join("index.json"), index).unwrap();

    temp_dir
}

/// Create an isolated CLI command that never touches the user's real config.
///
/// Returns `(Command, TempDir)` — the `TempDir` must be kept alive for the
/// duration of the test so the config directory isn't deleted prematurely.
fn isolated_cmd() -> (Command, TempDir) {
    let config_dir = TempDir::new().expect("Failed to create temp config dir");

    // Write a minimal default config so the CLI doesn't try to create one
    let attune_dir = config_dir.path().join("attune");
    fs::create_dir_all(&attune_dir).expect("Failed to create attune config dir");
    fs::write(
        attune_dir.join("config.yaml"),
        "profile: default\nformat: table\nprofiles:\n  default:\n    api_url: http://localhost:8080\n",
    )
    .expect("Failed to write test config");

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", config_dir.path())
        .env("HOME", config_dir.path());
    (cmd, config_dir)
}

#[test]
fn test_pack_checksum_directory() {
    let pack_dir = create_test_pack("checksum-test", "1.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("table")
        .arg("pack")
        .arg("checksum")
        .arg(pack_dir.path().to_str().unwrap());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("sha256:"));
}

#[test]
fn test_pack_checksum_json_output() {
    let pack_dir = create_test_pack("checksum-json", "1.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("json")
        .arg("pack")
        .arg("checksum")
        .arg(pack_dir.path().to_str().unwrap());

    let output = cmd.assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    // Verify it's valid JSON
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert!(json["checksum"].is_string());
    assert!(json["checksum"].as_str().unwrap().starts_with("sha256:"));
}

#[test]
fn test_pack_checksum_nonexistent_path() {
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack").arg("checksum").arg("/nonexistent/path");

    cmd.assert().failure().stderr(
        predicate::str::contains("not found").or(predicate::str::contains("does not exist")),
    );
}

#[test]
fn test_pack_index_entry_generates_valid_json() {
    for format_args in [Vec::<&str>::new(), vec!["--format", "json"]] {
        let pack_dir = create_component_summary_test_pack();
        let (mut cmd, _config_dir) = isolated_cmd();
        cmd.arg("pack")
            .arg("index-entry")
            .arg(pack_dir.path())
            .arg("--git-url")
            .arg("https://github.com/test/pack.git")
            .arg("--git-ref")
            .arg("v1.0.0")
            .args(format_args);

        let output = cmd.assert().success();
        let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
        assert!(!stdout.contains("Parsing pack.yaml"));
        assert!(!stdout.contains("Index entry generated successfully"));

        let entry: PackIndexEntry = serde_json::from_str(&stdout).unwrap();
        assert_eq!(entry.pack_ref, "component-test");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(entry.contents.actions[0].name, "alpha");
        assert_eq!(
            entry.contents.actions[0].description,
            "First alphabetically"
        );
        assert_eq!(entry.contents.actions[1].name, "zebra");
        assert_eq!(entry.contents.actions[1].description, "Last alphabetically");
        assert_eq!(entry.contents.sensors[0].name, "monitor");
        assert_eq!(entry.contents.sensors[0].description, "Monitor events");
        assert_eq!(entry.contents.triggers[0].name, "changed");
        assert_eq!(entry.contents.triggers[0].description, "Something changed");
        assert_eq!(entry.contents.rules[0].name, "dispatch");
        assert_eq!(entry.contents.rules[0].description, "Dispatch an action");
        assert_eq!(entry.contents.workflows[0].name, "deploy");
        assert_eq!(
            entry.contents.workflows[0].description,
            "Deploy the service"
        );
        assert_eq!(entry.contents.workflows[1].name, "nightly");
        assert_eq!(entry.contents.workflows[1].description, "Nightly workflow");
        assert_sha256(entry.install_sources[0].checksum());
    }
}

#[test]
fn test_pack_index_commands_require_a_real_source_without_placeholders() {
    let pack_dir = create_test_pack("source-required", "1.0.0", &[]);
    let index_dir = create_test_index(&[]);

    for args in [
        vec![
            "pack".to_owned(),
            "index-entry".to_owned(),
            pack_dir.path().display().to_string(),
        ],
        vec![
            "pack".to_owned(),
            "index-update".to_owned(),
            "--index".to_owned(),
            index_dir.path().join("index.json").display().to_string(),
            pack_dir.path().display().to_string(),
        ],
    ] {
        let (mut cmd, _config_dir) = isolated_cmd();
        let assertion = cmd.args(args).assert().failure();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(stderr.contains("--git-url") && stderr.contains("--archive-url"));
        assert!(!stdout.contains("your-org"));
        assert!(!stderr.contains("your-org"));
    }
}

#[test]
fn test_pack_index_commands_require_an_explicit_git_ref() {
    let pack_dir = create_test_pack("ref-required", "1.0.0", &[]);
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.args(["pack", "index-entry"])
        .arg(pack_dir.path())
        .args(["--git-url", "https://example.com/ref-required.git"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--git-ref"));
}

#[test]
fn test_pack_index_entry_matches_documented_manifest_metadata() {
    let pack_dir = create_documented_manifest_pack();
    let (mut cmd, _config_dir) = isolated_cmd();
    let output = cmd
        .args(["pack", "index-entry"])
        .arg(pack_dir.path())
        .args(["--git-url", "https://github.com/example-packs/example.git"])
        .args(["--git-ref", "0123456789abcdef0123456789abcdef01234567"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let entry: PackIndexEntry = serde_json::from_slice(&output).unwrap();

    assert_eq!(entry.label, "Example");
    assert_eq!(entry.license, "Apache-2.0");
    assert_eq!(entry.keywords, ["example", "integration"]);
    assert_eq!(entry.runtime_deps, ["python"]);
    assert_eq!(
        entry.homepage.as_deref(),
        Some("https://docs.example.com/attune-pack")
    );
    assert_eq!(
        entry.repository.as_deref(),
        Some("https://github.com/example-packs/example")
    );
    assert_eq!(
        entry.use_case.as_deref(),
        Some("Automate Example resources")
    );

    let dependencies = entry.dependencies.unwrap();
    assert_eq!(dependencies.attune_version.as_deref(), Some(">=0.1.0"));
    assert_eq!(dependencies.python_version.as_deref(), Some(">=3.11"));
    assert_eq!(dependencies.nodejs_version.as_deref(), Some(">=20"));
    assert_eq!(dependencies.packs, ["core"]);

    let metadata = entry.meta.unwrap();
    assert_eq!(metadata.tested_attune_versions, ["0.1.0"]);
    assert_eq!(metadata.extra["category"], "integration");
    assert_eq!(metadata.extra["support_tier"], "community");
    assert_eq!(metadata.extra["keywords"][0], "meta-keyword");
    assert!(!metadata.extra.contains_key("default_branch"));
    assert!(!metadata.extra.contains_key("commit"));
    assert!(metadata.stars.is_none());
}

#[test]
fn test_pack_index_entry_normalizes_list_dependencies() {
    let pack_dir = create_test_pack("list-dependencies", "1.0.0", &["core", "core"]);
    let (mut cmd, _config_dir) = isolated_cmd();
    let output = cmd
        .args(["pack", "index-entry"])
        .arg(pack_dir.path())
        .args(["--git-url", "https://example.com/list-dependencies.git"])
        .args(["--git-ref", "0123456789abcdef0123456789abcdef01234567"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let entry: PackIndexEntry = serde_json::from_slice(&output).unwrap();
    assert_eq!(entry.dependencies.unwrap().packs, ["core"]);
}

#[test]
fn test_pack_index_entry_inventories_inline_only_components() {
    let pack_dir = create_test_pack("inline-only", "1.0.0", &[]);
    let (mut cmd, _config_dir) = isolated_cmd();
    let output = cmd
        .args(["pack", "index-entry"])
        .arg(pack_dir.path())
        .args(["--git-url", "https://example.com/inline-only.git"])
        .args(["--git-ref", "0123456789abcdef0123456789abcdef01234567"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let entry: PackIndexEntry = serde_json::from_slice(&output).unwrap();

    assert_eq!(entry.contents.actions[0].name, "test_action");
    assert_eq!(entry.contents.actions[0].description, "Test action");
    assert_eq!(entry.contents.sensors[0].name, "test_sensor");
    assert_eq!(entry.contents.triggers[0].name, "test_trigger");
}

#[test]
fn test_pack_index_entry_rejects_non_string_component_metadata() {
    for (path, manifest_suffix, component_content) in [
        (None, "actions:\n  invalid:\n    description: 123\n", None),
        (
            Some("actions/invalid.yaml"),
            "",
            Some("ref: 123\ndescription: Invalid\n"),
        ),
    ] {
        let pack_dir = TempDir::new().unwrap();
        fs::write(
            pack_dir.path().join("pack.yaml"),
            format!("ref: strict-components\nversion: 1.0.0\n{manifest_suffix}"),
        )
        .unwrap();
        if let Some(path) = path {
            fs::create_dir(pack_dir.path().join("actions")).unwrap();
            fs::write(pack_dir.path().join(path), component_content.unwrap()).unwrap();
        }

        let (mut cmd, _config_dir) = isolated_cmd();
        cmd.args(["pack", "index-entry"])
            .arg(pack_dir.path())
            .args([
                "--archive-url",
                "https://example.com/strict-components.tar.gz",
                "--archive-checksum",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("must be a string"));
    }
}

#[test]
fn test_pack_index_entry_rejects_malformed_normalized_metadata() {
    for manifest_field in [
        "label: null\nname: Fallback must not win\n",
        "tags: [true]\n",
        "dependencies:\n  python_version: false\n",
        "meta:\n  extension: .nan\n",
    ] {
        let pack_dir = TempDir::new().unwrap();
        fs::write(
            pack_dir.path().join("pack.yaml"),
            format!("ref: malformed\nversion: 1.0.0\n{manifest_field}"),
        )
        .unwrap();

        let (mut cmd, _config_dir) = isolated_cmd();
        cmd.args(["pack", "index-entry"])
            .arg(pack_dir.path())
            .args([
                "--archive-url",
                "https://example.com/malformed.tar.gz",
                "--archive-checksum",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ])
            .assert()
            .failure();
    }
}

#[test]
fn test_pack_index_entry_rejects_duplicate_tested_attune_versions() {
    let pack_dir = TempDir::new().unwrap();
    fs::write(
        pack_dir.path().join("pack.yaml"),
        "ref: duplicate-tested\nversion: 1.0.0\nmeta:\n  tested_attune_versions: [0.3.0, 0.3.0]\n",
    )
    .unwrap();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.args(["pack", "index-entry"])
        .arg(pack_dir.path())
        .args([
            "--archive-url",
            "https://example.com/duplicate-tested.tar.gz",
            "--archive-checksum",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "meta.tested_attune_versions values must be unique",
        ));
}

#[test]
fn test_pack_index_entry_rejects_invalid_source_urls() {
    let invalid_urls = [
        "http://example.com/pack.git",
        "https://user:secret@example.com/pack.git",
        "https://example.com/pack.git?token=secret",
        "https://example.com/pack.git#fragment",
    ];

    for option in ["--git-url", "--archive-url"] {
        for url in invalid_urls {
            let pack_dir = create_test_pack("invalid-url", "1.0.0", &[]);
            let (mut cmd, _config_dir) = isolated_cmd();
            cmd.arg("pack")
                .arg("index-entry")
                .arg(pack_dir.path())
                .arg(option)
                .arg(url);
            if option == "--archive-url" {
                cmd.arg("--archive-checksum")
                    .arg("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            } else {
                cmd.args(["--git-ref", "0123456789abcdef0123456789abcdef01234567"]);
            }
            cmd.assert().failure().stderr(
                predicate::str::contains("HTTPS")
                    .or(predicate::str::contains("credentials"))
                    .or(predicate::str::contains("query"))
                    .or(predicate::str::contains("fragments")),
            );
        }
    }
}

#[test]
fn test_pack_index_entry_with_archive_url() {
    let pack_dir = create_test_pack("archive-test", "2.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("json")
        .arg("pack")
        .arg("index-entry")
        .arg(pack_dir.path().to_str().unwrap())
        .arg("--archive-url")
        .arg("https://releases.example.com/pack-2.0.0.tar.gz")
        .arg("--archive-checksum")
        .arg("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    let output = cmd.assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    let entry: PackIndexEntry = serde_json::from_str(&stdout).unwrap();
    let InstallSource::Archive { url, checksum } = &entry.install_sources[0] else {
        panic!("expected archive source");
    };
    assert_eq!(url, "https://releases.example.com/pack-2.0.0.tar.gz");
    assert_eq!(
        checksum,
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[test]
fn test_pack_index_entry_normalizes_source_urls() {
    let pack_dir = create_test_pack("normalized-urls", "1.0.0", &[]);
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-entry")
        .arg(pack_dir.path())
        .arg("--git-url")
        .arg("HTTPS://EXAMPLE.COM:443/pack.git")
        .arg("--git-ref")
        .arg("0123456789abcdef0123456789abcdef01234567")
        .arg("--archive-url")
        .arg("HTTPS://RELEASES.EXAMPLE.COM:443/pack.tar.gz")
        .arg("--archive-checksum")
        .arg("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    let output = cmd.assert().success().get_output().stdout.clone();
    let entry: PackIndexEntry = serde_json::from_slice(&output).unwrap();
    let InstallSource::Git { url: git_url, .. } = &entry.install_sources[0] else {
        panic!("expected Git source");
    };
    let InstallSource::Archive {
        url: archive_url, ..
    } = &entry.install_sources[1]
    else {
        panic!("expected archive source");
    };
    assert_eq!(git_url, "https://example.com/pack.git");
    assert_eq!(archive_url, "https://releases.example.com/pack.tar.gz");
}

#[test]
fn test_pack_index_entry_requires_archive_checksum() {
    let pack_dir = create_test_pack("archive-test", "2.0.0", &[]);
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-entry")
        .arg(pack_dir.path())
        .arg("--archive-url")
        .arg("https://releases.example.com/pack-2.0.0.tar.gz");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--archive-checksum"));
}

#[test]
fn test_pack_index_entry_rejects_invalid_archive_checksum() {
    let pack_dir = create_test_pack("archive-test", "2.0.0", &[]);
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-entry")
        .arg(pack_dir.path())
        .arg("--archive-url")
        .arg("https://releases.example.com/pack-2.0.0.tar.gz")
        .arg("--archive-checksum")
        .arg("sha256:ABC123");

    cmd.assert().failure().stderr(predicate::str::contains(
        "sha256:<64 lowercase hex characters>",
    ));
}

#[test]
fn test_pack_index_entry_missing_pack_yaml() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("readme.txt"), "No pack.yaml here").unwrap();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-entry")
        .arg(temp_dir.path().to_str().unwrap())
        .arg("--git-url")
        .arg("https://example.com/missing.git")
        .arg("--git-ref")
        .arg("0123456789abcdef0123456789abcdef01234567");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("pack.yaml"));
}

#[test]
fn test_pack_index_update_adds_new_entry() {
    let index_dir = create_test_index(&[("existing-pack", "1.0.0")]);
    let index_path = index_dir.path().join("index.json");

    let pack_dir = create_test_pack("new-pack", "1.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-update")
        .arg("--index")
        .arg(index_path.to_str().unwrap())
        .arg(pack_dir.path().to_str().unwrap())
        .arg("--git-url")
        .arg("https://github.com/test/new-pack.git")
        .arg("--git-ref")
        .arg("v1.0.0");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("new-pack"))
        .stdout(predicate::str::contains("1.0.0"));

    // Verify index was updated
    let updated_index = fs::read_to_string(&index_path).unwrap();
    let json: Value = serde_json::from_str(&updated_index).unwrap();
    assert_eq!(json["packs"].as_array().unwrap().len(), 2);
    let entry: PackIndexEntry = serde_json::from_value(json["packs"][1].clone()).unwrap();
    assert_eq!(entry.contents.actions[0].name, "test_action");
    assert_eq!(entry.contents.actions[0].description, "Test action");
    assert!(entry.contents.rules.is_empty());
    assert!(entry.contents.workflows.is_empty());
    assert_sha256(entry.install_sources[0].checksum());
}

#[test]
fn test_pack_index_update_sorts_and_updates_timestamp_on_change() {
    let index_dir = create_test_index(&[("z-existing", "1.0.0")]);
    let index_path = index_dir.path().join("index.json");
    let pack_dir = create_test_pack("a-new", "1.0.0", &[]);
    let before = chrono::Utc::now();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.args(["pack", "index-update", "--index"])
        .arg(&index_path)
        .arg(pack_dir.path())
        .args([
            "--git-url",
            "https://example.com/a-new.git",
            "--git-ref",
            "0123456789abcdef0123456789abcdef01234567",
        ])
        .assert()
        .success();

    let updated: Value = serde_json::from_str(&fs::read_to_string(index_path).unwrap()).unwrap();
    let refs: Vec<_> = updated["packs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pack| pack["ref"].as_str().unwrap())
        .collect();
    assert_eq!(refs, ["a-new", "z-existing"]);
    let timestamp = chrono::DateTime::parse_from_rfc3339(updated["last_updated"].as_str().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(timestamp >= before - chrono::Duration::seconds(1));
    assert!(timestamp <= chrono::Utc::now() + chrono::Duration::seconds(1));
}

#[test]
fn test_pack_index_update_preserves_timestamp_when_content_is_unchanged() {
    let index_dir = create_test_index(&[]);
    let index_path = index_dir.path().join("index.json");
    let pack_dir = create_test_pack("unchanged", "1.0.0", &[]);

    for update in [false, true] {
        if update {
            let mut index: Value =
                serde_json::from_str(&fs::read_to_string(&index_path).unwrap()).unwrap();
            index["last_updated"] = Value::String("2020-01-01T00:00:00Z".to_owned());
            fs::write(&index_path, serde_json::to_string_pretty(&index).unwrap()).unwrap();
        }

        let (mut cmd, _config_dir) = isolated_cmd();
        cmd.args(["pack", "index-update", "--index"])
            .arg(&index_path)
            .arg(pack_dir.path())
            .args([
                "--git-url",
                "https://example.com/unchanged.git",
                "--git-ref",
                "0123456789abcdef0123456789abcdef01234567",
            ]);
        if update {
            cmd.arg("--update");
        }
        cmd.assert().success();
    }

    let updated: Value = serde_json::from_str(&fs::read_to_string(index_path).unwrap()).unwrap();
    assert_eq!(updated["last_updated"], "2020-01-01T00:00:00Z");
}

#[test]
fn test_pack_index_update_validates_before_atomic_replace() {
    let index_dir = create_test_index(&[("existing", "1.0.0")]);
    let index_path = index_dir.path().join("index.json");
    let invalid = fs::read_to_string(&index_path).unwrap().replace(
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "sha256:invalid",
    );
    fs::write(&index_path, &invalid).unwrap();
    let pack_dir = create_test_pack("new-pack", "1.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.args(["pack", "index-update", "--index"])
        .arg(&index_path)
        .arg(pack_dir.path())
        .args([
            "--git-url",
            "https://example.com/new-pack.git",
            "--git-ref",
            "0123456789abcdef0123456789abcdef01234567",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid source checksum"));

    assert_eq!(fs::read_to_string(index_path).unwrap(), invalid);
}

#[test]
fn test_pack_index_update_rejects_schema_closed_fields_without_rewriting() {
    for mutation in ["unknown", "missing"] {
        let index_dir = create_test_index(&[("existing", "1.0.0")]);
        let index_path = index_dir.path().join("index.json");
        let mut invalid: Value =
            serde_json::from_str(&fs::read_to_string(&index_path).unwrap()).unwrap();
        if mutation == "unknown" {
            invalid["packs"][0]["unsupported"] = Value::Bool(true);
        } else {
            invalid["packs"][0]["contents"]
                .as_object_mut()
                .unwrap()
                .remove("sensors");
        }
        let invalid = serde_json::to_string_pretty(&invalid).unwrap();
        fs::write(&index_path, &invalid).unwrap();
        let pack_dir = create_test_pack("new-pack", "1.0.0", &[]);

        let (mut cmd, _config_dir) = isolated_cmd();
        cmd.args(["pack", "index-update", "--index"])
            .arg(&index_path)
            .arg(pack_dir.path())
            .args([
                "--archive-url",
                "https://example.com/new-pack.tar.gz",
                "--archive-checksum",
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ])
            .assert()
            .failure();

        assert_eq!(fs::read_to_string(index_path).unwrap(), invalid);
    }
}

#[test]
fn test_pack_index_update_orders_extension_metadata_keys() {
    let index_dir = create_test_index(&[]);
    let index_path = index_dir.path().join("index.json");
    let pack_dir = TempDir::new().unwrap();
    fs::write(
        pack_dir.path().join("pack.yaml"),
        "ref: ordered-meta\nversion: 1.0.0\nmeta:\n  zeta: last\n  alpha: first\n",
    )
    .unwrap();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.args(["pack", "index-update", "--index"])
        .arg(&index_path)
        .arg(pack_dir.path())
        .args([
            "--archive-url",
            "https://example.com/ordered-meta.tar.gz",
            "--archive-checksum",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ])
        .assert()
        .success();

    let updated = fs::read_to_string(index_path).unwrap();
    assert!(updated.find("\"alpha\"").unwrap() < updated.find("\"zeta\"").unwrap());
}

#[test]
fn test_pack_index_update_uses_supplied_archive_checksum() {
    let index_dir = create_test_index(&[]);
    let index_path = index_dir.path().join("index.json");
    let pack_dir = create_test_pack("archive-update", "1.0.0", &[]);
    let checksum = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-update")
        .arg("--index")
        .arg(&index_path)
        .arg(pack_dir.path())
        .arg("--archive-url")
        .arg("https://releases.example.com/archive-update.tar.gz")
        .arg("--archive-checksum")
        .arg(checksum);
    cmd.assert().success();

    let updated_index: Value =
        serde_json::from_str(&fs::read_to_string(index_path).unwrap()).unwrap();
    let entry: PackIndexEntry = serde_json::from_value(updated_index["packs"][0].clone()).unwrap();
    let InstallSource::Archive {
        checksum: emitted_checksum,
        ..
    } = &entry.install_sources[0]
    else {
        panic!("expected archive source");
    };
    assert_eq!(emitted_checksum, checksum);
}

#[test]
fn test_pack_index_update_requires_archive_checksum() {
    let index_dir = create_test_index(&[]);
    let pack_dir = create_test_pack("archive-update", "1.0.0", &[]);
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-update")
        .arg("--index")
        .arg(index_dir.path().join("index.json"))
        .arg(pack_dir.path())
        .arg("--archive-url")
        .arg("https://releases.example.com/archive-update.tar.gz");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--archive-checksum"));
}

#[test]
fn test_pack_index_update_prevents_duplicate_without_flag() {
    let index_dir = create_test_index(&[("existing-pack", "1.0.0")]);
    let index_path = index_dir.path().join("index.json");

    let pack_dir = create_test_pack("existing-pack", "1.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-update")
        .arg("--index")
        .arg(index_path.to_str().unwrap())
        .arg(pack_dir.path().to_str().unwrap())
        .arg("--git-url")
        .arg("https://github.com/test/existing-pack.git")
        .arg("--git-ref")
        .arg("0123456789abcdef0123456789abcdef01234567");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn test_pack_index_update_with_update_flag() {
    let index_dir = create_test_index(&[("existing-pack", "1.0.0")]);
    let index_path = index_dir.path().join("index.json");

    let pack_dir = create_test_pack("existing-pack", "2.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-update")
        .arg("--index")
        .arg(index_path.to_str().unwrap())
        .arg(pack_dir.path().to_str().unwrap())
        .arg("--git-url")
        .arg("https://github.com/test/existing-pack.git")
        .arg("--git-ref")
        .arg("v2.0.0")
        .arg("--update");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("existing-pack"))
        .stdout(predicate::str::contains("2.0.0"));

    // Verify version was updated
    let updated_index = fs::read_to_string(&index_path).unwrap();
    let json: Value = serde_json::from_str(&updated_index).unwrap();
    let packs = json["packs"].as_array().unwrap();
    assert_eq!(packs.len(), 1);
    assert_eq!(packs[0]["version"], "2.0.0");
}

#[test]
fn test_pack_index_update_invalid_index_file() {
    let temp_dir = TempDir::new().unwrap();
    let bad_index = temp_dir.path().join("bad-index.json");
    fs::write(&bad_index, "not valid json {").unwrap();

    let pack_dir = create_test_pack("test-pack", "1.0.0", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-update")
        .arg("--index")
        .arg(bad_index.to_str().unwrap())
        .arg(pack_dir.path().to_str().unwrap())
        .arg("--git-url")
        .arg("https://example.com/test-pack.git")
        .arg("--git-ref")
        .arg("0123456789abcdef0123456789abcdef01234567");

    cmd.assert().failure();
}

#[test]
fn test_pack_index_merge_combines_indexes() {
    let index1 = create_test_index(&[("pack-a", "1.0.0"), ("pack-b", "1.0.0")]);
    let index2 = create_test_index(&[("pack-c", "1.0.0"), ("pack-d", "1.0.0")]);

    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("table")
        .arg("pack")
        .arg("index-merge")
        .arg("--file")
        .arg(output_path.to_str().unwrap())
        .arg(index1.path().join("index.json").to_str().unwrap())
        .arg(index2.path().join("index.json").to_str().unwrap());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Merged"))
        .stdout(predicate::str::contains("2"));

    // Verify merged file
    let merged_content = fs::read_to_string(&output_path).unwrap();
    let index: PackIndex = serde_json::from_str(&merged_content).unwrap();
    assert_eq!(index.registry_name, "Test Pack Index");
    assert_eq!(index.registry_url, "https://example.com/pack-index");
    assert_eq!(index.version, "1.0");
    assert_eq!(index.last_updated, "2026-08-15T00:00:00Z");
    assert_eq!(index.packs.len(), 4);
    assert_eq!(
        index
            .packs
            .iter()
            .map(|pack| pack.pack_ref.as_str())
            .collect::<Vec<_>>(),
        ["pack-a", "pack-b", "pack-c", "pack-d"]
    );
}

#[test]
fn test_pack_index_merge_deduplicates() {
    let index1 = create_test_index(&[("pack-a", "1.0.0"), ("pack-b", "1.0.0")]);
    let index2 = create_test_index(&[("pack-a", "2.0.0"), ("pack-c", "1.0.0")]);

    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("table")
        .arg("pack")
        .arg("index-merge")
        .arg("--file")
        .arg(output_path.to_str().unwrap())
        .arg(index1.path().join("index.json").to_str().unwrap())
        .arg(index2.path().join("index.json").to_str().unwrap());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Duplicates resolved"));

    // Verify deduplication (should have 3 unique packs: pack-a, pack-b, pack-c)
    let merged_content = fs::read_to_string(&output_path).unwrap();
    let json: Value = serde_json::from_str(&merged_content).unwrap();
    let packs = json["packs"].as_array().unwrap();
    assert_eq!(packs.len(), 3);

    // Verify pack-a has the newer version
    let pack_a = packs.iter().find(|p| p["ref"] == "pack-a").unwrap();
    assert_eq!(pack_a["version"], "2.0.0");
}

#[test]
fn test_pack_index_merge_uses_semantic_version_precedence() {
    let index1 = create_test_index(&[("numeric", "1.9.0"), ("release", "2.0.0-rc.1")]);
    let index2 = create_test_index(&[("numeric", "1.10.0"), ("release", "2.0.0")]);
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.args(["pack", "index-merge", "--file"])
        .arg(&output_path)
        .arg(index1.path().join("index.json"))
        .arg(index2.path().join("index.json"))
        .assert()
        .success();

    let index: PackIndex = serde_json::from_str(&fs::read_to_string(output_path).unwrap()).unwrap();
    assert_eq!(index.packs[0].version, "1.10.0");
    assert_eq!(index.packs[1].version, "2.0.0");
}

#[test]
fn test_pack_index_merge_output_exists_without_force() {
    let index1 = create_test_index(&[("pack-a", "1.0.0")]);

    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");
    fs::write(&output_path, "existing content").unwrap();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-merge")
        .arg("--file")
        .arg(output_path.to_str().unwrap())
        .arg(index1.path().join("index.json").to_str().unwrap());

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("already exists").or(predicate::str::contains("force")));
}

#[test]
fn test_pack_index_merge_with_force_flag() {
    let index1 = create_test_index(&[("pack-a", "1.0.0")]);

    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");
    fs::write(&output_path, "existing content").unwrap();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-merge")
        .arg("--file")
        .arg(output_path.to_str().unwrap())
        .arg(index1.path().join("index.json").to_str().unwrap())
        .arg("--force");

    cmd.assert().success();

    // Verify file was overwritten
    let merged_content = fs::read_to_string(&output_path).unwrap();
    assert_ne!(merged_content, "existing content");
}

#[test]
fn test_pack_index_merge_empty_input_list() {
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-merge")
        .arg("--file")
        .arg(output_path.to_str().unwrap());

    // Should fail due to missing required inputs
    cmd.assert().failure();
}

#[test]
fn test_pack_index_merge_missing_input_file() {
    let index1 = create_test_index(&[("pack-a", "1.0.0")]);
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("table")
        .arg("pack")
        .arg("index-merge")
        .arg("--file")
        .arg(output_path.to_str().unwrap())
        .arg(index1.path().join("index.json").to_str().unwrap())
        .arg("/nonexistent/index.json");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
    assert!(!output_path.exists());
}

#[test]
fn test_pack_index_merge_preserves_existing_output_on_invalid_input() {
    let input_dir = create_test_index(&[("invalid", "1.0.0")]);
    let invalid_path = input_dir.path().join("index.json");
    let invalid = fs::read_to_string(&invalid_path).unwrap().replace(
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "sha256:invalid",
    );
    fs::write(&invalid_path, invalid).unwrap();
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.json");
    fs::write(&output_path, "existing content\n").unwrap();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.args(["pack", "index-merge", "--file"])
        .arg(&output_path)
        .arg(&invalid_path)
        .arg("--force")
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid source checksum"));

    assert_eq!(
        fs::read_to_string(output_path).unwrap(),
        "existing content\n"
    );
}

#[test]
fn test_pack_check_valid_directory_json() {
    let pack_dir = create_test_pack("check-test", "1.2.3", &[]);
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("json")
        .arg("pack")
        .arg("check")
        .arg(pack_dir.path());

    let output = cmd.assert().success().get_output().stdout.clone();
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["valid"], true);
    assert_eq!(report["pack_ref"], "check-test");
    assert_eq!(report["version"], "1.2.3");
    assert_eq!(report["files_checked"], 1);
}

#[test]
fn test_pack_check_invalid_directory_fails_after_report() {
    let pack_dir = TempDir::new().unwrap();
    fs::write(
        pack_dir.path().join("pack.yaml"),
        "ref: Invalid\nversion: nope\n",
    )
    .unwrap();
    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("json")
        .arg("pack")
        .arg("check")
        .arg(pack_dir.path());

    let assertion = cmd.assert().failure().stderr(predicate::str::contains(
        "Pack check failed with 2 error(s)",
    ));
    let report: Value = serde_json::from_slice(&assertion.get_output().stdout).unwrap();
    assert_eq!(report["valid"], false);
    assert_eq!(report["errors"], 2);
    assert_eq!(report["diagnostics"][0]["severity"], "error");
}

#[test]
fn test_pack_commands_help() {
    let commands = vec![
        vec!["pack", "checksum", "--help"],
        vec!["pack", "check", "--help"],
        vec!["pack", "index-entry", "--help"],
        vec!["pack", "index-update", "--help"],
        vec!["pack", "index-merge", "--help"],
        vec!["pack", "index", "--help"],
        vec!["pack", "index", "list", "--help"],
        vec!["pack", "index", "add", "--help"],
        vec!["pack", "index", "update", "--help"],
        vec!["pack", "index", "delete", "--help"],
        vec!["pack", "index", "browse", "--help"],
        vec!["pack", "index", "show", "--help"],
    ];

    for args in commands {
        let (mut cmd, _config_dir) = isolated_cmd();
        for arg in &args {
            cmd.arg(arg);
        }
        cmd.assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }
}
