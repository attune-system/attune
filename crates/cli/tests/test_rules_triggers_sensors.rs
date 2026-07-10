//! Integration tests for CLI rules, triggers, and sensors commands
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

// ============================================================================
// Rule Tests
// ============================================================================

#[tokio::test]
async fn test_rule_list_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock rule list endpoint
    mock_rule_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("rule")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("On Webhook"));
}

#[tokio::test]
async fn test_rule_list_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock unauthorized response
    mock_unauthorized(&fixture.mock_server, "/api/v1/rules").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("rule")
        .arg("list");

    cmd.assert().failure();
}

#[tokio::test]
async fn test_rule_list_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock rule list endpoint
    mock_rule_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("rule")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""ref": "core.on_webhook""#));
}

#[tokio::test]
async fn test_rule_list_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock rule list endpoint
    mock_rule_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("rule")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("ref: core.on_webhook"));
}

#[tokio::test]
async fn test_rule_get_by_ref() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock rule get endpoint
    Mock::given(method("GET"))
        .and(path("/api/v1/rules/core.on_webhook"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "ref": "core.on_webhook",
                "pack": 1,
                "pack_ref": "core",
                "label": "On Webhook",
                "description": "Handle webhook events",
                "trigger": 1,
                "trigger_ref": "core.webhook",
                "action": 1,
                "action_ref": "core.echo",
                "enabled": true,
                "conditions": {},
                "action_params": {},
                "trigger_params": {},
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
        .arg("rule")
        .arg("show")
        .arg("core.on_webhook");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("On Webhook"))
        .stdout(predicate::str::contains("Handle webhook events"));
}

#[tokio::test]
async fn test_rule_get_not_found() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock 404 response
    mock_not_found(&fixture.mock_server, "/api/v1/rules/nonexistent.rule").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("rule")
        .arg("show")
        .arg("nonexistent.rule");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[tokio::test]
async fn test_rule_list_by_pack() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock rule list endpoint with pack filter via query parameter
    mock_rule_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("rule")
        .arg("list")
        .arg("--pack")
        .arg("core");

    cmd.assert().success();
}

// ============================================================================
// Trigger Tests
// ============================================================================

#[tokio::test]
async fn test_trigger_list_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock trigger list endpoint
    mock_trigger_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("trigger")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Webhook Trigger"));
}

#[tokio::test]
async fn test_trigger_list_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock unauthorized response
    mock_unauthorized(&fixture.mock_server, "/api/v1/triggers").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("trigger")
        .arg("list");

    cmd.assert().failure();
}

#[tokio::test]
async fn test_trigger_list_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock trigger list endpoint
    mock_trigger_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("trigger")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""ref": "core.webhook""#));
}

#[tokio::test]
async fn test_trigger_list_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock trigger list endpoint
    mock_trigger_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("trigger")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("ref: core.webhook"));
}

#[tokio::test]
async fn test_trigger_get_by_ref() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock trigger get endpoint
    Mock::given(method("GET"))
        .and(path("/api/v1/triggers/core.webhook"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "ref": "core.webhook",
                "pack": 1,
                "pack_ref": "core",
                "label": "Webhook Trigger",
                "description": "Webhook trigger",
                "enabled": true,
                "param_schema": {},
                "out_schema": {},
                "webhook_enabled": false,
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
        .arg("trigger")
        .arg("show")
        .arg("core.webhook");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Webhook Trigger"))
        .stdout(predicate::str::contains("Webhook trigger"));
}

#[tokio::test]
async fn test_trigger_get_not_found() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock 404 response
    mock_not_found(&fixture.mock_server, "/api/v1/triggers/nonexistent.trigger").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("trigger")
        .arg("show")
        .arg("nonexistent.trigger");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

// ============================================================================
// Sensor Tests
// ============================================================================

#[tokio::test]
async fn test_sensor_list_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock sensor list endpoint
    mock_sensor_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("sensor")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Webhook Sensor"));
}

#[tokio::test]
async fn test_sensor_list_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock unauthorized response
    mock_unauthorized(&fixture.mock_server, "/api/v1/sensors").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("sensor")
        .arg("list");

    cmd.assert().failure();
}

#[tokio::test]
async fn test_sensor_list_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock sensor list endpoint
    mock_sensor_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("sensor")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""ref": "core.webhook_sensor""#));
}

#[tokio::test]
async fn test_sensor_list_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock sensor list endpoint
    mock_sensor_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("sensor")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("ref: core.webhook_sensor"));
}

#[tokio::test]
async fn test_sensor_get_by_ref() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock sensor get endpoint
    Mock::given(method("GET"))
        .and(path("/api/v1/sensors/core.webhook_sensor"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 1,
                "ref": "core.webhook_sensor",
                "pack": 1,
                "pack_ref": "core",
                "label": "Webhook Sensor",
                "description": "Webhook sensor",
                "enabled": true,
                "trigger_types": ["core.webhook"],
                "entry_point": "webhook_sensor.py",
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
        .arg("sensor")
        .arg("show")
        .arg("core.webhook_sensor");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Webhook Sensor"))
        .stdout(predicate::str::contains("Webhook sensor"));
}

#[tokio::test]
async fn test_sensor_get_not_found() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock 404 response
    mock_not_found(&fixture.mock_server, "/api/v1/sensors/nonexistent.sensor").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("sensor")
        .arg("show")
        .arg("nonexistent.sensor");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[tokio::test]
async fn test_sensor_list_by_pack() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock sensor list endpoint with pack filter via query parameter
    mock_sensor_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("sensor")
        .arg("list")
        .arg("--pack")
        .arg("core");

    cmd.assert().success();
}

// ============================================================================
// Cross-feature Tests
// ============================================================================

#[tokio::test]
async fn test_all_list_commands_with_profile() {
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
  staging:
    api_url: {}
    auth_token: staging_token
    refresh_token: staging_refresh
"#,
        fixture.server_url(),
        fixture.server_url()
    );
    fixture.write_config(&config);

    // Mock all list endpoints
    mock_rule_list(&fixture.mock_server).await;
    mock_trigger_list(&fixture.mock_server).await;
    mock_sensor_list(&fixture.mock_server).await;

    // Test rule list with profile
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--profile")
        .arg("staging")
        .arg("rule")
        .arg("list");
    cmd.assert().success();

    // Test trigger list with profile
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--profile")
        .arg("staging")
        .arg("trigger")
        .arg("list");
    cmd.assert().success();

    // Test sensor list with profile
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--profile")
        .arg("staging")
        .arg("sensor")
        .arg("list");
    cmd.assert().success();
}

#[tokio::test]
async fn test_empty_list_results() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock empty lists
    Mock::given(method("GET"))
        .and(path("/api/v1/rules"))
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

    Mock::given(method("GET"))
        .and(path("/api/v1/triggers"))
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

    Mock::given(method("GET"))
        .and(path("/api/v1/sensors"))
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

    // All should succeed with empty results
    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("rule")
        .arg("list");
    cmd.assert().success();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("trigger")
        .arg("list");
    cmd.assert().success();

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("sensor")
        .arg("list");
    cmd.assert().success();
}
