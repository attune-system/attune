//! Integration tests for CLI execution commands
#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;

mod common;
use common::*;

#[tokio::test]
async fn test_execution_list_authenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution list endpoint
    mock_execution_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("succeeded"))
        .stdout(predicate::str::contains("failed"));
}

#[tokio::test]
async fn test_execution_list_unauthenticated() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    // Mock unauthorized response
    mock_unauthorized(&fixture.mock_server, "/api/v1/executions").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("list");

    cmd.assert().failure();
}

#[tokio::test]
async fn test_execution_list_json_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution list endpoint
    mock_execution_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--json")
        .arg("execution")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""status": "succeeded""#))
        .stdout(predicate::str::contains(r#""status": "failed""#));
}

#[tokio::test]
async fn test_execution_list_yaml_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution list endpoint
    mock_execution_list(&fixture.mock_server).await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("--yaml")
        .arg("execution")
        .arg("list");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("status: succeeded"))
        .stdout(predicate::str::contains("status: failed"));
}

#[tokio::test]
async fn test_execution_get_by_id() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution get endpoint
    mock_execution_get(&fixture.mock_server, 123, "succeeded").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("show")
        .arg("123");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("succeeded"));
}

#[tokio::test]
async fn test_execution_get_not_found() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock 404 response
    mock_not_found(&fixture.mock_server, "/api/v1/executions/999").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("show")
        .arg("999");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[tokio::test]
async fn test_execution_list_with_status_filter() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution list with filter
    use serde_json::json;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .and(query_param("status", "succeeded"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "action_ref": "core.echo",
                    "status": "succeeded",
                    "parent": null,
                    "enforcement": null,
                    "result": {"output": "Hello"},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
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
        .arg("execution")
        .arg("list")
        .arg("--status")
        .arg("succeeded");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("succeeded"));
}

#[tokio::test]
async fn test_execution_result_raw_output() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution get endpoint with result
    use serde_json::json;
    use wiremock::{
        matchers::{method, path},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions/123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 123,
                "action_ref": "core.echo",
                "status": "succeeded",
                "config": {"message": "Hello"},
                "result": {"output": "Hello World", "exit_code": 0},
                "parent": null,
                "enforcement": null,
                "executor": null,
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
        .arg("execution")
        .arg("result")
        .arg("123");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Hello World"))
        .stdout(predicate::str::contains("exit_code"));
}

#[tokio::test]
async fn test_execution_list_with_pack_filter() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution list with pack filter
    use serde_json::json;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .and(query_param("pack_name", "core"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "action_ref": "core.echo",
                    "status": "succeeded",
                    "parent": null,
                    "enforcement": null,
                    "result": {"output": "Test output"},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
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
        .arg("execution")
        .arg("list")
        .arg("--pack")
        .arg("core");

    cmd.assert().success();
}

#[tokio::test]
async fn test_execution_list_with_action_filter() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution list with action filter
    use serde_json::json;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .and(query_param("action_ref", "core.echo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "action_ref": "core.echo",
                    "status": "succeeded",
                    "parent": null,
                    "enforcement": null,
                    "result": {"output": "Echo test"},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
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
        .arg("execution")
        .arg("list")
        .arg("--action")
        .arg("core.echo");

    cmd.assert().success();
}

#[tokio::test]
async fn test_execution_list_multiple_filters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock execution list with multiple filters
    use serde_json::json;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .and(query_param("status", "succeeded"))
        .and(query_param("pack_name", "core"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "action_ref": "core.echo",
                    "status": "succeeded",
                    "parent": null,
                    "enforcement": null,
                    "result": {},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
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
        .arg("execution")
        .arg("list")
        .arg("--status")
        .arg("succeeded")
        .arg("--pack")
        .arg("core");

    cmd.assert().success();
}

#[tokio::test]
async fn test_execution_get_with_profile() {
    let fixture = TestFixture::new().await;

    // Create multi-profile config
    let config = format!(
        r#"
current_profile: default
default_output_format: table
profiles:
  default:
    api_url: {}
    auth_token: valid_token
    refresh_token: refresh_token
    description: Default server
  production:
    api_url: {}
    auth_token: prod_token
    refresh_token: prod_refresh
    description: Production server
"#,
        fixture.server_url(),
        fixture.server_url()
    );
    fixture.write_config(&config);

    // Mock execution get endpoint
    mock_execution_get(&fixture.mock_server, 456, "running").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--profile")
        .arg("production")
        .arg("execution")
        .arg("show")
        .arg("456");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("running"));
}

#[tokio::test]
async fn test_execution_list_empty_result() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    // Mock empty execution list
    use serde_json::json;
    use wiremock::{
        matchers::{method, path},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": {
                "page": 1,
                "page_size": 50,
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
        .arg("execution")
        .arg("list");

    cmd.assert().success();
}

#[tokio::test]
async fn test_execution_get_invalid_id() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("show")
        .arg("not_a_number");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
}

#[tokio::test]
async fn test_execution_list_with_rule_trigger_and_top_level_filters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    use serde_json::json;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .and(query_param("rule_ref", "core.on_timer"))
        .and(query_param("trigger_ref", "core.timer"))
        .and(query_param("top_level_only", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": 1,
                    "action_ref": "core.echo",
                    "status": "running",
                    "parent": null,
                    "enforcement": 12,
                    "rule_ref": "core.on_timer",
                    "trigger_ref": "core.timer",
                    "result": {"output": "tick"},
                    "created": "2024-01-01T00:00:00Z",
                    "updated": "2024-01-01T00:00:00Z"
                }
            ],
            "pagination": {
                "page": 1,
                "page_size": 50,
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
        .arg("execution")
        .arg("list")
        .arg("--rule")
        .arg("core.on_timer")
        .arg("--trigger")
        .arg("core.timer")
        .arg("--top-level-only");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("core.on_timer"))
        .stdout(predicate::str::contains("core.timer"));
}

#[tokio::test]
async fn test_execution_watch_streams_updates() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    use serde_json::json;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions"))
        .and(query_param("status", "running"))
        .and(query_param("top_level_only", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": {
                "page": 1,
                "page_size": 50,
                "has_previous": false,
                "has_next": false
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let body = "data: {\"entity_id\":3,\"payload\":{\"id\":3,\"action_ref\":\"core.echo\",\"status\":\"running\",\"parent\":null,\"rule_ref\":\"core.on_timer\",\"trigger_ref\":\"core.timer\",\"created\":\"2024-01-01T00:00:01Z\",\"updated\":\"2024-01-01T00:00:02Z\"}}\n\n";
    Mock::given(method("GET"))
        .and(path("/api/v1/executions/stream"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("watch")
        .arg("--status")
        .arg("running")
        .arg("--top-level-only");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Watching executions"))
        .stdout(predicate::str::contains("core.echo"))
        .stdout(predicate::str::contains("running"));
}

#[tokio::test]
async fn test_execution_watch_existing_execution_by_id() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    use serde_json::json;
    use wiremock::{
        matchers::{method, path},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions/123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 123,
                "action_ref": "core.echo",
                "status": "completed",
                "result": {"output": "done"},
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:01Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("watch")
        .arg("123")
        .arg("--timeout")
        .arg("5");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Execution 123 completed"))
        .stdout(predicate::str::contains("completed"));
}

#[tokio::test]
async fn test_execution_watch_failed_preserves_output_and_exits_nonzero() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    mock_execution_get(&fixture.mock_server, 124, "failed").await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("watch")
        .arg("124")
        .arg("--timeout")
        .arg("5");

    cmd.assert()
        .failure()
        .stdout(predicate::str::contains("Execution 124 completed"))
        .stdout(predicate::str::contains("failed"))
        .stdout(predicate::str::contains("Hello"))
        .stderr(predicate::str::contains(
            "execution finished with status 'failed'",
        ));
}

#[tokio::test]
async fn test_execution_rerun_rejects_unresolved_secret_markers_before_post() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    use serde_json::json;
    use wiremock::{
        matchers::{method, path},
        Mock, ResponseTemplate,
    };

    Mock::given(method("GET"))
        .and(path("/api/v1/executions/125"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "id": 125,
                "action_ref": "core.echo",
                "config": {
                    "message": "safe",
                    "token": {"$attune_secret": true, "redacted": true}
                },
                "status": "failed",
                "created": "2024-01-01T00:00:00Z",
                "updated": "2024-01-01T00:00:01Z"
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("execution")
        .arg("rerun")
        .arg("125")
        .arg("--param")
        .arg("message=changed");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("redacted secrets at /token"))
        .stderr(predicate::str::contains("replace every secret path"));

    let requests = fixture.mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "rerun must not POST redaction markers");
    assert_eq!(requests[0].method.as_str(), "GET");
}
