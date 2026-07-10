//! Integration tests for CLI workflow commands

#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;
use common::*;

// ── Mock helpers ────────────────────────────────────────────────────────

async fn mock_workflow_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/workflows"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": "core.install_packs",
                    "pack_ref": "core",
                    "label": "Install Packs",
                    "description": "Install one or more packs",
                    "version": "1.0.0",
                    "tags": ["core", "packs"],
                    "enabled": true,
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                },
                {
                    "id": 2,
                    "ref": "mypack.deploy",
                    "pack_ref": "mypack",
                    "label": "Deploy App",
                    "description": "Deploy an application",
                    "version": "2.0.0",
                    "tags": ["deploy"],
                    "enabled": true,
                    "created": "2024-01-02T00:00:00Z",
                    "updated": "2024-01-02T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_next": false,
                "has_previous": false,
                "total_items": 2,
                "total_pages": 1
            }
        })))
        .mount(server)
        .await;
}

async fn mock_workflow_list_by_pack(server: &MockServer, pack_ref: &str) {
    let p = format!("/api/v1/packs/{}/workflows", pack_ref);
    Mock::given(method("GET"))
        .and(path(p.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": format!("{}.example_workflow", pack_ref),
                    "pack_ref": pack_ref,
                    "label": "Example Workflow",
                    "description": "An example workflow",
                    "version": "1.0.0",
                    "tags": [],
                    "enabled": true,
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_next": false,
                "has_previous": false,
                "total_items": 1,
                "total_pages": 1
            }
        })))
        .mount(server)
        .await;
}

async fn mock_workflow_get(server: &MockServer, workflow_ref: &str) {
    let p = format!("/api/v1/workflows/{}", workflow_ref);
    Mock::given(method("GET"))
        .and(path(p.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "ref": workflow_ref,
                "pack": 1,
                "pack_ref": "mypack",
                "label": "My Workflow",
                "description": "A test workflow",
                "version": "1.0.0",
                "param_schema": {
                    "url": {"type": "string", "required": true},
                    "timeout": {"type": "integer", "default": 30}
                },
                "out_schema": {
                    "status": {"type": "string"}
                },
                "definition": {
                    "version": "1.0.0",
                    "vars": {"result": null},
                    "tasks": [
                        {
                            "name": "step1",
                            "action": "core.echo",
                            "input": {"message": "hello"},
                            "next": [
                                {"when": "{{ succeeded() }}", "do": ["step2"]}
                            ]
                        },
                        {
                            "name": "step2",
                            "action": "core.echo",
                            "input": {"message": "done"}
                        }
                    ]
                },
                "tags": ["test", "demo"],
                "enabled": true,
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(server)
        .await;
}

async fn mock_workflow_delete(server: &MockServer, workflow_ref: &str) {
    let p = format!("/api/v1/workflows/{}", workflow_ref);
    Mock::given(method("DELETE"))
        .and(path(p.as_str()))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
}

async fn mock_workflow_save(server: &MockServer, pack_ref: &str) {
    let p = format!("/api/v1/packs/{}/workflow-files", pack_ref);
    Mock::given(method("POST"))
        .and(path(p.as_str()))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "data": {
                "id": 10,
                "ref": format!("{}.deploy", pack_ref),
                "pack": 1,
                "pack_ref": pack_ref,
                "label": "Deploy App",
                "description": "Deploy the application",
                "version": "1.0.0",
                "param_schema": null,
                "out_schema": null,
                "definition": {"version": "1.0.0", "tasks": []},
                "tags": ["deploy"],
                "enabled": true,
                "created": "2024-01-10T00:00:00Z",
                "updated": "2024-01-10T00:00:00Z"
            }
        })))
        .mount(server)
        .await;
}

async fn mock_workflow_save_conflict(server: &MockServer, pack_ref: &str) {
    let p = format!("/api/v1/packs/{}/workflow-files", pack_ref);
    Mock::given(method("POST"))
        .and(path(p.as_str()))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": "Workflow with ref 'mypack.deploy' already exists"
        })))
        .mount(server)
        .await;
}

async fn mock_workflow_update(server: &MockServer, workflow_ref: &str) {
    let p = format!("/api/v1/workflows/{}/file", workflow_ref);
    Mock::given(method("PUT"))
        .and(path(p.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 10,
                "ref": workflow_ref,
                "pack": 1,
                "pack_ref": "mypack",
                "label": "Deploy App",
                "description": "Deploy the application",
                "version": "1.0.0",
                "param_schema": null,
                "out_schema": null,
                "definition": {"version": "1.0.0", "tasks": []},
                "tags": ["deploy"],
                "enabled": true,
                "created": "2024-01-10T00:00:00Z",
                "updated": "2024-01-10T12:00:00Z"
            }
        })))
        .mount(server)
        .await;
}

// ── Helper to write action + workflow YAML to temp dirs ─────────────────

struct WorkflowFixture {
    _dir: tempfile::TempDir,
    action_yaml_path: String,
}

impl WorkflowFixture {
    fn new(action_ref: &str, workflow_file: &str) -> Self {
        let dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let actions_dir = dir.path().join("actions");
        let workflows_dir = actions_dir.join("workflows");
        fs::create_dir_all(&workflows_dir).unwrap();

        // Write the action YAML
        let action_yaml = format!(
            r#"ref: {}
label: "Deploy App"
description: "Deploy the application"
enabled: true
workflow_file: {}

parameters:
  environment:
    type: string
    required: true
    description: "Target environment"
  version:
    type: string
    default: "latest"

output:
  status:
    type: string

tags:
  - deploy
"#,
            action_ref, workflow_file,
        );

        let action_name = action_ref.rsplit('.').next().unwrap();
        let action_path = actions_dir.join(format!("{}.yaml", action_name));
        fs::write(&action_path, &action_yaml).unwrap();

        // Write the workflow YAML
        let workflow_yaml = r#"version: "1.0.0"

vars:
  deploy_result: null

tasks:
  - name: prepare
    action: core.echo
    input:
      message: "Preparing deployment"
    next:
      - when: "{{ succeeded() }}"
        do:
          - deploy

  - name: deploy
    action: core.echo
    input:
      message: "Deploying"
    next:
      - when: "{{ succeeded() }}"
        do:
          - verify

  - name: verify
    action: core.echo
    input:
      message: "Verifying"

output_map:
  status: "{{ 'success' if workflow.deploy_result else 'unknown' }}"
"#;

        let workflow_path = workflows_dir.join(format!("{}.workflow.yaml", action_name));
        fs::write(&workflow_path, workflow_yaml).unwrap();

        Self {
            action_yaml_path: action_path.to_string_lossy().to_string(),
            _dir: dir,
        }
    }
}

// ── List tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_workflow_list_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.install_packs"))
        .stdout(predicate::str::contains("mypack.deploy"))
        .stdout(predicate::str::contains("2 workflow(s) found"));
}

#[tokio::test]
async fn test_workflow_list_by_pack() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_list_by_pack(&fixture.mock_server, "core").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("list")
        .arg("--pack")
        .arg("core");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.example_workflow"))
        .stdout(predicate::str::contains("1 workflow(s) found"));
}

#[tokio::test]
async fn test_workflow_list_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("workflow")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"core.install_packs\""))
        .stdout(predicate::str::contains("\"mypack.deploy\""));
}

#[tokio::test]
async fn test_workflow_list_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("workflow")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.install_packs"))
        .stdout(predicate::str::contains("mypack.deploy"));
}

#[tokio::test]
async fn test_workflow_list_empty() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    Mock::given(method("GET"))
        .and(path("/api/v1/workflows"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_next": false,
                "has_previous": false,
                "total_items": 0,
                "total_pages": 0
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("No workflows found"));
}

#[tokio::test]
async fn test_workflow_list_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    mock_unauthorized(&fixture.mock_server, "/api/v1/workflows").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("list");

    cmd.assert().failure();
}

// ── Show tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_workflow_show() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_get(&fixture.mock_server, "mypack.my_workflow").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("show")
        .arg("mypack.my_workflow");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("mypack.my_workflow"))
        .stdout(predicate::str::contains("My Workflow"))
        .stdout(predicate::str::contains("1.0.0"))
        .stdout(predicate::str::contains("test, demo"))
        // Tasks table should show task names
        .stdout(predicate::str::contains("step1"))
        .stdout(predicate::str::contains("step2"))
        .stdout(predicate::str::contains("core.echo"));
}

#[tokio::test]
async fn test_workflow_show_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_get(&fixture.mock_server, "mypack.my_workflow").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("workflow")
        .arg("show")
        .arg("mypack.my_workflow");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"mypack.my_workflow\""))
        .stdout(predicate::str::contains("\"My Workflow\""))
        .stdout(predicate::str::contains("\"definition\""));
}

#[tokio::test]
async fn test_workflow_show_not_found() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_not_found(&fixture.mock_server, "/api/v1/workflows/nonexistent.wf").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("show")
        .arg("nonexistent.wf");

    cmd.assert().failure();
}

// ── Delete tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_workflow_delete_with_yes_flag() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_delete(&fixture.mock_server, "mypack.my_workflow").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("delete")
        .arg("mypack.my_workflow")
        .arg("--yes");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("deleted successfully"));
}

#[tokio::test]
async fn test_workflow_delete_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    mock_workflow_delete(&fixture.mock_server, "mypack.my_workflow").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("workflow")
        .arg("delete")
        .arg("mypack.my_workflow")
        .arg("--yes");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"message\""))
        .stdout(predicate::str::contains("deleted"));
}

// ── Upload tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_workflow_upload_success() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let wf_fixture = WorkflowFixture::new("mypack.deploy", "workflows/deploy.workflow.yaml");

    mock_workflow_save(&fixture.mock_server, "mypack").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("upload")
        .arg(&wf_fixture.action_yaml_path);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("uploaded successfully"))
        .stdout(predicate::str::contains("mypack.deploy"));
}

#[tokio::test]
async fn test_workflow_upload_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let wf_fixture = WorkflowFixture::new("mypack.deploy", "workflows/deploy.workflow.yaml");

    mock_workflow_save(&fixture.mock_server, "mypack").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("workflow")
        .arg("upload")
        .arg(&wf_fixture.action_yaml_path);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"mypack.deploy\""))
        .stdout(predicate::str::contains("\"Deploy App\""));
}

#[tokio::test]
async fn test_workflow_upload_conflict_without_force() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let wf_fixture = WorkflowFixture::new("mypack.deploy", "workflows/deploy.workflow.yaml");

    mock_workflow_save_conflict(&fixture.mock_server, "mypack").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("upload")
        .arg(&wf_fixture.action_yaml_path);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("already exists"))
        .stderr(predicate::str::contains("--force"));
}

#[tokio::test]
async fn test_workflow_upload_conflict_with_force() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let wf_fixture = WorkflowFixture::new("mypack.deploy", "workflows/deploy.workflow.yaml");

    mock_workflow_save_conflict(&fixture.mock_server, "mypack").await;
    mock_workflow_update(&fixture.mock_server, "mypack.deploy").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("upload")
        .arg(&wf_fixture.action_yaml_path)
        .arg("--force");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("uploaded successfully"));
}

#[tokio::test]
async fn test_workflow_upload_missing_action_file() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("upload")
        .arg("/nonexistent/path/action.yaml");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[tokio::test]
async fn test_workflow_upload_missing_workflow_file() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Create a temp dir with only the action YAML, no workflow file
    let dir = tempfile::TempDir::new().unwrap();
    let actions_dir = dir.path().join("actions");
    fs::create_dir_all(&actions_dir).unwrap();

    let action_yaml = r#"ref: mypack.deploy
label: "Deploy App"
workflow_file: workflows/deploy.workflow.yaml
"#;
    let action_path = actions_dir.join("deploy.yaml");
    fs::write(&action_path, action_yaml).unwrap();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("upload")
        .arg(action_path.to_string_lossy().as_ref());

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Workflow file not found"));
}

#[tokio::test]
async fn test_workflow_upload_action_without_workflow_file_field() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Create a temp dir with a regular (non-workflow) action YAML
    let dir = tempfile::TempDir::new().unwrap();
    let actions_dir = dir.path().join("actions");
    fs::create_dir_all(&actions_dir).unwrap();

    let action_yaml = r#"ref: mypack.echo
label: "Echo"
description: "A regular action, not a workflow"
runner_type: shell
entry_point: echo.sh
"#;
    let action_path = actions_dir.join("echo.yaml");
    fs::write(&action_path, action_yaml).unwrap();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("upload")
        .arg(action_path.to_string_lossy().as_ref());

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("workflow_file"));
}

#[tokio::test]
async fn test_workflow_upload_invalid_action_yaml() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let dir = tempfile::TempDir::new().unwrap();
    let bad_yaml_path = dir.path().join("bad.yaml");
    fs::write(&bad_yaml_path, "this is not valid yaml: [[[").unwrap();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("workflow")
        .arg("upload")
        .arg(bad_yaml_path.to_string_lossy().as_ref());

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Failed to parse action YAML"));
}

// ── Help text tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_workflow_help() {
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.arg("workflow").arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("upload"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("show"))
        .stdout(predicate::str::contains("delete"));
}

#[tokio::test]
async fn test_workflow_upload_help() {
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.arg("workflow").arg("upload").arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("action"))
        .stdout(predicate::str::contains("workflow_file"))
        .stdout(predicate::str::contains("--force"));
}
