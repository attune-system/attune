//! Integration tests for CLI queue commands.
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

fn command(fixture: &TestFixture) -> Command {
    let mut command = Command::cargo_bin("attune").expect("attune binary");
    command
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url());
    command
}

fn queue_summary(queue_ref: &str) -> Value {
    json!({
        "id": 4,
        "ref": queue_ref,
        "pack_ref": "ops",
        "is_adhoc": false,
        "label": "Urgent work",
        "description": "Items requiring attention",
        "enabled": true,
        "accepting_new_items": true,
        "dispatch_action_ref": "ops.process",
        "trace_tag_template": null,
        "reference_visibility": "public",
        "reference_allowed_pack_refs": [],
        "retired_at": null,
        "created": "2026-09-25T10:00:00Z",
        "updated": "2026-09-25T10:05:00Z"
    })
}

fn queue_item(queue_ref: &str, status: &str) -> Value {
    json!({
        "id": 42,
        "queue": 4,
        "queue_ref": queue_ref,
        "item_key": "customer/42",
        "priority": 25,
        "status": status,
        "payload": {"customer": "A&B"},
        "metadata": {"source": "cli"},
        "trace_tag": "manual.cli.42",
        "enqueue_source": "api",
        "requested_by_identity": 7,
        "requested_by_execution": null,
        "requested_by_enforcement": null,
        "leased_execution": if status == "leased" { Some(99) } else { None },
        "lease_token": if status == "leased" { Some("c447e077-dcd0-4aa8-a101-f95b377d22b9") } else { None },
        "lease_expires_at": if status == "leased" { Some("2026-09-25T10:10:00Z") } else { None },
        "attempt_count": 2,
        "last_error": {"message": "temporary failure"},
        "ack_summary": null,
        "created": "2026-09-25T10:00:00Z",
        "updated": "2026-09-25T10:05:00Z"
    })
}

fn pagination(total: u64) -> Value {
    json!({
        "page": 1,
        "page_size": 50,
        "has_previous": false,
        "has_next": false,
        "total_items": total,
        "total_pages": if total == 0 { 0 } else { 1 }
    })
}

#[test]
fn queue_help_registers_interoperability_commands() {
    Command::cargo_bin("attune")
        .expect("attune binary")
        .args(["queue", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("enqueue"))
        .stdout(predicate::str::contains("items"));

    Command::cargo_bin("attune")
        .expect("attune binary")
        .args(["queue", "items", "ops.work", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("show"));
}

#[tokio::test]
async fn queue_list_sends_supported_filters_and_encodes_values() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/packs/ops%2Fspecial/queues"))
        .and(query_param("enabled", "true"))
        .and(query_param("is_adhoc", "false"))
        .and(query_param("search", "urgent & ready"))
        .and(query_param("referencing_pack_ref", "agent/tools"))
        .and(query_param("page", "2"))
        .and(query_param("per_page", "25"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [queue_summary("ops.urgent")],
            "pagination": pagination(1)
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args([
            "queue",
            "list",
            "--pack",
            "ops/special",
            "--enabled",
            "true",
            "--is-adhoc",
            "false",
            "--search",
            "urgent & ready",
            "--referencing-pack-ref",
            "agent/tools",
            "--page",
            "2",
            "--per-page",
            "25",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ops.urgent"))
        .stdout(predicate::str::contains("ops.process"));
}

#[tokio::test]
async fn queue_list_json_is_undecorated_and_keeps_pagination() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/queues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [queue_summary("ops.urgent")],
            "pagination": pagination(1)
        })))
        .mount(&fixture.mock_server)
        .await;

    let output = command(&fixture)
        .args(["--json", "queue", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&output).expect("stdout is only JSON");
    assert_eq!(parsed["items"][0]["ref"], "ops.urgent");
    assert_eq!(parsed["pagination"]["total_items"], 1);
}

#[tokio::test]
async fn queue_list_table_has_an_empty_state() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/queues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": pagination(0)
        })))
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args(["queue", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No work queues found."));
}

#[tokio::test]
async fn queue_enqueue_posts_exact_request_and_encodes_queue_ref() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    let request = json!({
        "item_key": "customer/42",
        "priority": 25,
        "payload": {"customer": "A&B"},
        "metadata": {"source": "cli"},
        "trace_tag": "manual.cli.42"
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/queues/ops%2Furgent%20queue/items"))
        .and(body_json(request.clone()))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "data": queue_item("ops/urgent queue", "queued")
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    let output = command(&fixture)
        .args([
            "--json",
            "queue",
            "enqueue",
            "ops/urgent queue",
            "--request-json",
            &request.to_string(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&output).expect("stdout is only JSON");
    assert_eq!(parsed["status"], "queued");
    assert_eq!(parsed["payload"]["customer"], "A&B");
}

#[tokio::test]
async fn queue_enqueue_reads_stdin_and_outputs_undecorated_yaml() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    let request = json!({"payload": {"job": 42}});
    Mock::given(method("POST"))
        .and(path("/api/v1/queues/ops.work/items"))
        .and(body_json(json!({"payload": {"job": 42}, "metadata": {}})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "data": queue_item("ops.work", "queued")
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    let output = command(&fixture)
        .args([
            "--yaml",
            "queue",
            "enqueue",
            "ops.work",
            "--request-file",
            "-",
        ])
        .write_stdin(request.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_yaml_ng::from_slice(&output).expect("stdout is only YAML");
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["status"], "queued");
}

#[tokio::test]
async fn queue_item_list_sends_filters_and_shows_operational_columns() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/queues/ops%2Fwork/items"))
        .and(query_param("item_key", "customer/42 & active"))
        .and(query_param("enqueue_source", "api/import"))
        .and(query_param("statuses", "leased"))
        .and(query_param("statuses", "retry"))
        .and(query_param("page", "3"))
        .and(query_param("per_page", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [queue_item("ops/work", "leased")],
            "pagination": pagination(1)
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args([
            "queue",
            "items",
            "ops/work",
            "list",
            "--item-key",
            "customer/42 & active",
            "--enqueue-source",
            "api/import",
            "--status",
            "leased",
            "--status",
            "retry",
            "--page",
            "3",
            "--per-page",
            "10",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status"))
        .stdout(predicate::str::contains("Priority"))
        .stdout(predicate::str::contains("Attempts"))
        .stdout(predicate::str::contains("leased"));
}

#[tokio::test]
async fn queue_item_list_json_empty_response_is_undecorated() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/queues/ops.work/items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "pagination": pagination(0)
        })))
        .mount(&fixture.mock_server)
        .await;

    let output = command(&fixture)
        .args(["--json", "queue", "items", "ops.work", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&output).expect("stdout is only JSON");
    assert_eq!(parsed["items"], json!([]));
    assert_eq!(parsed["pagination"]["total_items"], 0);
}

#[tokio::test]
async fn queue_item_show_encodes_path_and_reports_lease_status_in_yaml() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/queues/ops%2Fwork/items/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": queue_item("ops/work", "leased")
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    let output = command(&fixture)
        .args(["--yaml", "queue", "items", "ops/work", "show", "42"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_yaml_ng::from_slice(&output).expect("stdout is only YAML");
    assert_eq!(parsed["status"], "leased");
    assert_eq!(parsed["leased_execution"], 99);
    assert_eq!(parsed["attempt_count"], 2);
    assert_eq!(parsed["last_error"]["message"], "temporary failure");
}

#[tokio::test]
async fn queue_item_show_surfaces_api_errors() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("GET"))
        .and(path("/api/v1/queues/ops.work/items/404"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": "Queue item '404' not found"
        })))
        .mount(&fixture.mock_server)
        .await;

    command(&fixture)
        .args(["queue", "items", "ops.work", "show", "404"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Queue item '404' not found"));
}

#[test]
fn queue_page_bounds_are_rejected_by_clap() {
    Command::cargo_bin("attune")
        .expect("attune binary")
        .args(["queue", "list", "--page", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("page must be at least 1"));

    Command::cargo_bin("attune")
        .expect("attune binary")
        .args(["queue", "items", "ops.work", "list", "--per-page", "101"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "per-page must be between 1 and 100",
        ));
}

#[test]
fn queue_enqueue_rejects_missing_or_unknown_request_fields_before_http() {
    Command::cargo_bin("attune")
        .expect("attune binary")
        .args(["queue", "enqueue", "ops.work"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--request-json"));

    Command::cargo_bin("attune")
        .expect("attune binary")
        .args([
            "queue",
            "enqueue",
            "ops.work",
            "--request-json",
            r#"{"payload":{},"unsupported":true}"#,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown field `unsupported`"));
}
