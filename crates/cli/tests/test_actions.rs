//! Integration tests for CLI action commands
#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, ResponseTemplate,
};

mod common;
use common::*;

#[tokio::test]
async fn test_action_list_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list endpoint
    mock_action_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.echo"))
        .stdout(predicate::str::contains("Echo a message"));
}

#[tokio::test]
async fn test_action_list_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock unauthorized response
    mock_unauthorized(&fixture.mock_server, "/api/v1/actions").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list");

    cmd.assert().failure();
}

#[tokio::test]
async fn test_action_list_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list endpoint
    mock_action_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("action")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""ref""#))
        .stdout(predicate::str::contains(r#"core.echo"#));
}

#[tokio::test]
async fn test_action_list_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list endpoint
    mock_action_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("action")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.echo"))
        .stdout(predicate::str::contains("Echo a message"));
}

#[tokio::test]
async fn test_action_get_by_ref() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action get endpoint
    Mock::given(method("GET"))
        .and(path("/api/v1/actions/core.echo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "ref": "core.echo",
                "pack": 1,
                "pack_ref": "core",
                "label": "Echo Action",
                "description": "Echo a message",
                "entrypoint": "echo.py",
                "runtime": null,
                "param_schema": {
                    "message": {
                        "type": "string",
                        "description": "Message to echo",
                        "required": true
                    }
                },
                "out_schema": null,
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("show")
        .arg("core.echo");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.echo"))
        .stdout(predicate::str::contains("Echo a message"));
}

#[tokio::test]
async fn test_action_get_not_found() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock 404 response
    mock_not_found(&fixture.mock_server, "/api/v1/actions/nonexistent.action").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("show")
        .arg("nonexistent.action");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[tokio::test]
async fn test_action_execute_with_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 42).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=Hello World");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("42").or(predicate::str::contains("scheduled")));
}

#[tokio::test]
async fn test_action_execute_multiple_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 100).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("linux.run_command")
        .arg("--param")
        .arg("cmd=ls -la")
        .arg("--param")
        .arg("timeout=30");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_with_json_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 101).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.webhook")
        .arg("--params-json")
        .arg(r#"{"url": "https://example.com", "method": "POST"}"#);

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_without_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 200).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.no_params_action");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 150).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=test");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("150"))
        .stdout(predicate::str::contains("scheduled"));
}

#[tokio::test]
async fn test_action_execute_wait_for_completion() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 250).await;

    // Mock execution polling - first running, then succeeded
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/250"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 250,
                "action": 1,
                "action_ref": "core.echo",
                "config": {"message": "test"},
                "parent": null,
                "enforcement": null,
                "executor": null,
                "status": "succeeded",
                "result": {"output": "test"},
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=test")
        .arg("--watch");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("succeeded"));
}

#[tokio::test]
async fn test_action_execute_watch_failed_preserves_output_and_exits_nonzero() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    mock_action_execute(&fixture.mock_server, 251).await;
    mock_execution_get(&fixture.mock_server, 251, "cancelled").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--watch");

    cmd.assert()
        .failure()
        .stdout(predicate::str::contains("Execution 251 completed"))
        .stdout(predicate::str::contains("cancelled"))
        .stdout(predicate::str::contains("Hello"))
        .stderr(predicate::str::contains(
            "execution finished with status 'cancelled'",
        ));
}

#[tokio::test]
#[ignore = "Profile switching needs more investigation - CLI integration issue"]
async fn test_action_execute_with_profile() {
    let fixture = TestFixture::new().await;

    // Create multi-profile config
    let config = format!(
        r#"
current_profile: default
default_output_format: table
profiles:
  default:
    api_url: {}
    auth_token: default_token
    refresh_token: default_refresh
  production:
    api_url: {}
    auth_token: prod_token
    refresh_token: prod_refresh
"#,
        fixture.server_url(),
        fixture.server_url()
    );
    fixture.write_config(&config);

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 300).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--profile")
        .arg("production")
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("message=prod_test");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_invalid_param_format() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--param")
        .arg("invalid_format_no_equals");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error").or(predicate::str::contains("=")));
}

#[tokio::test]
async fn test_action_execute_invalid_json_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.echo")
        .arg("--params-json")
        .arg(r#"{"invalid json"#);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error").or(predicate::str::contains("JSON")));
}

#[tokio::test]
async fn test_action_list_by_pack() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action list for a specific pack
    Mock::given(method("GET"))
        .and(path("/api/v1/packs/core/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "ref": "core.echo",
                    "pack_ref": "core",
                    "label": "Echo Action",
                    "description": "Echo a message",
                    "entrypoint": "echo.py",
                    "runtime": null,
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "total_items": 1,
                "total_pages": 1,
                "has_previous": false,
                "has_next": false
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list")
        .arg("--pack")
        .arg("core");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_execute_async_flag() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action execute endpoint
    mock_action_execute(&fixture.mock_server, 400).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("execute")
        .arg("core.long_running");
    // Note: default behavior is async (no --watch), so no --async flag needed

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("scheduled").or(predicate::str::contains("400")));
}

#[tokio::test]
async fn test_action_list_empty_result() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock empty action list
    Mock::given(method("GET"))
        .and(path("/api/v1/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "total_items": 0,
                "total_pages": 0,
                "has_previous": false,
                "has_next": false
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("list");

    cmd.assert().success();
}

#[tokio::test]
async fn test_action_get_shows_parameters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock action get with detailed parameters
    Mock::given(method("GET"))
        .and(path("/api/v1/actions/core.complex"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 5,
                "ref": "core.complex",
                "pack": 1,
                "pack_ref": "core",
                "label": "Complex Action",
                "description": "Complex action with multiple params",
                "entrypoint": "complex.py",
                "runtime": null,
                "param_schema": {
                    "required_string": {
                        "type": "string",
                        "description": "A required string parameter",
                        "required": true
                    },
                    "optional_number": {
                        "type": "integer",
                        "description": "An optional number",
                        "required": false,
                        "default": 42
                    },
                    "boolean_flag": {
                        "type": "boolean",
                        "description": "A boolean flag",
                        "required": false,
                        "default": false
                    }
                },
                "out_schema": null,
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:00Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("action")
        .arg("show")
        .arg("core.complex");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("required_string"))
        .stdout(predicate::str::contains("optional_number"));
}
