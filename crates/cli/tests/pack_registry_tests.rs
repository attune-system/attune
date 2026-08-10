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
                "version": "{}",
                "author": "Test",
                "license": "Apache-2.0",
                "keywords": ["test"],
                "install_sources": [
                    {{
                        "type": "git",
                        "url": "https://github.com/test/{}.git",
                        "ref": "v{}",
                        "checksum": "sha256:abc123"
                    }}
                ]
            }}"#,
                name, name, version, name, version
            )
        })
        .collect();

    let index = format!(
        r#"{{
            "version": "1.0",
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
    let pack_dir = create_test_pack("index-entry-test", "1.2.3", &[]);

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("--output")
        .arg("json")
        .arg("pack")
        .arg("index-entry")
        .arg(pack_dir.path().to_str().unwrap())
        .arg("--git-url")
        .arg("https://github.com/test/pack.git")
        .arg("--git-ref")
        .arg("v1.2.3");

    let output = cmd.assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    // Verify it's valid JSON
    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["ref"], "index-entry-test");
    assert_eq!(json["version"], "1.2.3");
    assert!(json["install_sources"].is_array());
    assert!(json["install_sources"][0]["checksum"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    // Verify metadata
    assert_eq!(json["author"], "Test Author");
    assert_eq!(json["license"], "Apache-2.0");
    assert!(!json["keywords"].as_array().unwrap().is_empty());
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
        .arg("https://releases.example.com/pack-2.0.0.tar.gz");

    let output = cmd.assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    let json: Value = serde_json::from_str(&stdout).unwrap();
    assert!(!json["install_sources"].as_array().unwrap().is_empty());

    let archive_source = &json["install_sources"][0];
    assert_eq!(archive_source["type"], "archive");
    assert_eq!(
        archive_source["url"],
        "https://releases.example.com/pack-2.0.0.tar.gz"
    );
}

#[test]
fn test_pack_index_entry_missing_pack_yaml() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("readme.txt"), "No pack.yaml here").unwrap();

    let (mut cmd, _config_dir) = isolated_cmd();
    cmd.arg("pack")
        .arg("index-entry")
        .arg(temp_dir.path().to_str().unwrap());

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
        .arg("https://github.com/test/existing-pack.git");

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
        .arg(pack_dir.path().to_str().unwrap());

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
    let json: Value = serde_json::from_str(&merged_content).unwrap();
    assert_eq!(json["packs"].as_array().unwrap().len(), 4);
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

    // Should succeed but skip missing file (with warning in stderr)
    cmd.assert()
        .success()
        .stderr(predicate::str::contains("Skipping").or(predicate::str::contains("missing")));
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
