//! Integration tests for CLI inquiry commands.
#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::{
    matchers::{body_json, method, path, query_param},
    Mock, ResponseTemplate,
};

mod common;
use common::TestFixture;

fn inquiry(id: i64, status: &str) -> Value {
    json!({
        "id": id,
        "created_by_execution": 77,
        "workflow_execution": 12,
        "workflow_task_name": "approve",
        "purpose": "deployment-approval",
        "prompt": "Approve deployment?",
        "response_schema": {
            "approved": {"type": "boolean", "required": true}
        },
        "response_options": [{
            "ref": "approve",
            "label": "Approve",
            "style": "positive",
            "response": {"approved": true}
        }],
        "assigned_to": 9,
        "status": status,
        "response": if status == "responded" { Some(json!({"approved": true})) } else { None },
        "timeout_at": "2026-09-21T12:00:00Z",
        "responded_by": if status == "responded" { Some(9) } else { None },
        "responded_at": if status == "responded" { Some("2026-09-21T11:05:00Z") } else { None },
        "created": "2026-09-21T11:00:00Z",
        "updated": "2026-09-21T11:05:00Z"
    })
}

fn command(fixture: &TestFixture) -> Command {
    let mut command = Command::cargo_bin("attune").expect("attune binary");
    command
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url());
    command
}

#[tokio::test]
async fn inquiry_list_sends_all_supported_filters() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/inquiries"))
        .and(query_param("status", "pending"))
        .and(query_param("created_by_execution", "77"))
        .and(query_param("assigned_to", "9"))
        .and(query_param("offset", "10"))
        .and(query_param("limit", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": 42,
                "created_by_execution": 77,
                "prompt": "Approve deployment?",
                "assigned_to": 9,
                "status": "pending",
                "has_response": false,
                "timeout_at": null,
                "created": "2026-09-21T11:00:00Z"
            }],
            "pagination": {
                "page": 3,
                "page_size": 5,
                "has_previous": true,
                "has_next": false
            }
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args([
            "inquiry",
            "list",
            "--status",
            "pending",
            "--created-by-execution",
            "77",
            "--assigned-to",
            "9",
            "--offset",
            "10",
            "--limit",
            "5",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Approve deployment?"));
}

#[tokio::test]
async fn inquiry_list_json_preserves_pagination_metadata() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/inquiries"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": {
                "page": 2,
                "page_size": 50,
                "has_previous": true,
                "has_next": false
            }
        })))
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args(["--json", "inquiry", "list", "--offset", "50"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""has_previous": true"#));
}

#[tokio::test]
async fn inquiry_show_fetches_numeric_id() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/inquiries/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": inquiry(42, "pending")
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args(["--json", "inquiry", "show", "42"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""created_by_execution": 77"#));
}

#[tokio::test]
async fn inquiry_respond_option_posts_the_stored_response_object() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/inquiries/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": inquiry(42, "pending")
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/inquiries/42/respond"))
        .and(body_json(json!({"response": {"approved": true}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": inquiry(42, "responded")
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args(["inquiry", "respond", "42", "--option", "approve"])
        .assert()
        .success()
        .stdout(predicate::str::contains("responded"));
}

#[tokio::test]
async fn execution_inquiry_create_reads_stdin_and_preserves_flat_schema() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("execution_token", "refresh_token");
    let request = json!({
        "purpose": "deployment-approval",
        "prompt": "Approve deployment?",
        "response_schema": {
            "approved": {"type": "boolean", "required": true},
            "reason": {"type": "string", "required": false}
        },
        "response_options": [{
            "ref": "approve",
            "label": "Approve",
            "style": "positive",
            "response": {"approved": true}
        }],
        "assigned_to": 9,
        "timeout_seconds": 3600
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/inquiries"))
        .and(body_json(request.clone()))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "data": {
                "inquiry": inquiry(42, "pending"),
                "response_options": []
            }
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args([
            "--json",
            "inquiry",
            "execution",
            "create",
            "--request-file",
            "-",
        ])
        .write_stdin(request.to_string())
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""inquiry""#));
}

#[tokio::test]
async fn execution_inquiry_cancel_calls_creator_endpoint() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("execution_token", "refresh_token");
    Mock::given(method("POST"))
        .and(path("/api/v1/inquiries/42/cancel"))
        .and(body_json(json!({})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": inquiry(42, "cancelled")
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args(["inquiry", "execution", "cancel", "42"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cancelled"));
}
