mod common;

use assert_cmd::Command;
use common::TestFixture;
use predicates::prelude::*;
use serde_json::{json, Value};
use wiremock::{
    matchers::{body_json, method, path},
    Mock, ResponseTemplate,
};

fn command(fixture: &TestFixture) -> Command {
    command_with_output(fixture, "json")
}

fn command_with_output(fixture: &TestFixture, output: &str) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("attune"));
    command
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .env_remove("ATTUNE_PROFILE")
        .env_remove("ATTUNE_API_URL")
        .env_remove("ATTUNE_API_TOKEN")
        .env_remove("ATTUNE_AUTH_TOKEN")
        .env_remove("ATTUNE_REFRESH_TOKEN")
        .args(["--api-url", &fixture.server_url(), "--output", output])
        .timeout(std::time::Duration::from_secs(10));
    command
}

#[tokio::test]
async fn live_namespace_mode_is_sent_when_selected_and_retained_in_metadata() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");
    Mock::given(method("POST")).and(path("/api/v1/cache/namespaces"))
        .and(body_json(json!({"owner_type": "system", "namespace": "users", "mode": "live"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data": {
            "id": 1, "namespace": "users", "owner_type": "system", "mode": "live", "revision": 0, "source_revision": null, "refresh_concurrency": "parallel"}})))
        .expect(1).mount(&fixture.mock_server).await;
    let output = command(&fixture)
        .args([
            "cache",
            "namespace",
            "create",
            "users",
            "--owner-type",
            "system",
            "--mode",
            "live",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["mode"], "live");
    assert_eq!(response["revision"], 0);
}

#[tokio::test]
async fn live_apply_accepts_complete_record_upserts_from_stdin() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");
    Mock::given(method("POST")).and(path("/api/v1/cache/namespaces/users/entries/apply"))
        .and(body_json(json!({"owner_type": "system", "request_id": "sync-1", "expected_revision": 0,
            "entries": [{"external_id": "a", "value": {"name": "Alex"}, "source_updated_at": null, "source_checksum": null}],
            "deleted_external_ids": [], "source_revision": null})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"revision": 1}})))
        .expect(1).mount(&fixture.mock_server).await;
    command(&fixture)
        .args([
            "cache",
            "entry",
            "apply",
            "users",
            "--owner-type",
            "system",
            "--request-id",
            "sync-1",
            "--expected-revision",
            "0",
            "--file",
            "-",
        ])
        .write_stdin("{\"external_id\":\"a\",\"value\":{\"name\":\"Alex\"}}\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"revision\": 1"));
}

#[tokio::test]
async fn live_apply_sends_atomic_delta_and_reports_revision() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");
    Mock::given(method("POST"))
        .and(path("/api/v1/cache/namespaces/users/entries/apply"))
        .and(body_json(json!({"owner_type": "system", "request_id": "sync-2", "expected_revision": 1,
            "entries": [], "deleted_external_ids": ["removed"], "source_revision": "cursor-2"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
            "revision": 2, "record_count": 0, "size_bytes": 0, "source_revision": "cursor-2", "replayed": false}})))
        .expect(1).mount(&fixture.mock_server).await;
    let output = command(&fixture)
        .args([
            "cache",
            "entry",
            "apply",
            "users",
            "--owner-type",
            "system",
            "--request-id",
            "sync-2",
            "--expected-revision",
            "1",
            "--delete-external-id",
            "removed",
            "--source-revision",
            "cursor-2",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["revision"], 2);
}

#[tokio::test]
async fn live_lookup_accepts_revision_without_exposing_backing_generation() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");
    Mock::given(method("POST"))
        .and(path("/api/v1/cache/namespaces/users/entries/lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
            "generation_id": null, "revision": 4, "item": null, "stale": false}})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    command(&fixture)
        .args([
            "cache",
            "entry",
            "get",
            "users",
            "missing",
            "--owner-type",
            "system",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"revision\": 4"));
}

#[tokio::test]
async fn generation_show_and_list_display_creator_execution_in_json_and_table() {
    for output in ["json", "table"] {
        for operation in ["show", "list"] {
            let fixture = TestFixture::new().await;
            fixture.write_authenticated_config("token", "refresh");
            let metadata = generation("original", "retired", Some(9876543210));
            let body = if operation == "show" {
                json!({"data": metadata})
            } else {
                json!({"data": {"generations": [metadata], "next_cursor": null}})
            };
            Mock::given(method("GET"))
                .and(path(if operation == "show" {
                    "/api/v1/cache/namespaces/users/generations/42"
                } else {
                    "/api/v1/cache/namespaces/users/generations"
                }))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&fixture.mock_server)
                .await;
            let mut cmd = command_with_output(&fixture, output);
            cmd.args(["cache", "generation", operation, "users"]);
            if operation == "show" {
                cmd.arg("42");
            }
            cmd.args(["--owner-type", "system"]);
            cmd.assert()
                .success()
                .stdout(predicate::str::contains("9876543210"));
        }
    }
}

fn generation(refresh_id: &str, state: &str, execution: Option<i64>) -> Value {
    json!({
        "generation_id": 42, "namespace_id": 1, "status": state,
        "client_refresh_id": refresh_id, "expected_active_generation_id": null, "source_generation_id": null,
        "expected_chunk_count": 1, "expected_record_count": 1, "expected_size_bytes": null,
        "record_count": 0, "size_bytes": 0, "checksum_algorithm": null, "checksum": null,
        "source_revision": "original-revision", "created_by": null,
        "created_by_execution": execution, "created": "2026-10-07T00:00:00Z",
        "sealed": null, "activated": null, "retired": null, "readable_until": null,
        "failed": null, "failure_reason": null
    })
}

#[tokio::test]
async fn clone_begin_and_deletion_only_chunk_send_no_existing_values() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");
    let mut response = generation("delta", "staging", None);
    response["source_generation_id"] = json!(41);
    response["expected_active_generation_id"] = json!(41);
    Mock::given(method("POST"))
        .and(path("/api/v1/cache/namespaces/users/generations"))
        .and(body_json(json!({
            "owner_type": "system", "client_refresh_id": "delta",
            "source_generation_id": 41, "expected_active_generation_id": 41,
            "expected_chunk_count": 1
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data": response})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let output = command(&fixture)
        .args([
            "cache",
            "refresh",
            "begin",
            "users",
            "--owner-type",
            "system",
            "--client-refresh-id",
            "delta",
            "--source-generation",
            "41",
            "--expected-active",
            "41",
            "--expected-chunk-count",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let metadata: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(metadata["source_generation_id"], 41);
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/cache/namespaces/users/generations/42/chunks/0",
        ))
        .and(body_json(
            json!({"owner_type": "system", "entries": [], "deleted_external_ids": ["removed"]}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": metadata})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    command(&fixture)
        .args([
            "cache",
            "refresh",
            "upload",
            "users",
            "42",
            "--owner-type",
            "system",
            "--chunk-index",
            "0",
            "--delete-external-id",
            "removed",
        ])
        .assert()
        .success();
}

#[tokio::test]
async fn namespace_policy_is_sent_flat_only_when_explicit_and_retained_in_json() {
    for operation in ["create", "update"] {
        for mode in [Some("reuse"), Some("conflict"), Some("parallel"), None] {
            if operation == "update" && mode.is_none() {
                continue;
            }
            let fixture = TestFixture::new().await;
            fixture.write_authenticated_config("token", "refresh");
            let mut body = json!({"owner_type": "system"});
            if operation == "create" {
                body["namespace"] = json!("users");
            }
            if let Some(mode) = mode {
                body["refresh_concurrency"] = json!(mode);
            }
            Mock::given(method(if operation == "create" { "POST" } else { "PUT" }))
                .and(path(if operation == "create" {
                    "/api/v1/cache/namespaces"
                } else {
                    "/api/v1/cache/namespaces/users"
                }))
                .and(body_json(body))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                    "id": 1, "namespace": "users", "owner_type": "system",
                    "refresh_concurrency": mode.unwrap_or("parallel")
                }})))
                .expect(1)
                .mount(&fixture.mock_server)
                .await;
            let mut cmd = command(&fixture);
            cmd.args([
                "cache",
                "namespace",
                operation,
                "users",
                "--owner-type",
                "system",
            ]);
            if let Some(mode) = mode {
                cmd.args(["--refresh-concurrency", mode]);
            }
            let output = cmd.assert().success().get_output().stdout.clone();
            let response: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(response["refresh_concurrency"], mode.unwrap_or("parallel"));
        }
    }
}

#[tokio::test]
async fn invalid_refresh_concurrency_fails_before_http() {
    let fixture = TestFixture::new().await;
    command(&fixture)
        .args([
            "cache",
            "namespace",
            "create",
            "users",
            "--owner-type",
            "system",
            "--refresh-concurrency",
            "wait",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "possible values: reuse, conflict, parallel",
        ));
    assert!(fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn apply_reused_refresh_never_uploads_seals_promotes_or_abandons() {
    for input_from_stdin in [false, true] {
        for state in ["staging", "ready"] {
            let fixture = TestFixture::new().await;
            fixture.write_authenticated_config("token", "refresh");
            Mock::given(method("POST"))
                .and(path("/api/v1/cache/namespaces/users/generations"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "data": generation("other-producer", state, Some(9876543210))
                })))
                .expect(1)
                .mount(&fixture.mock_server)
                .await;
            let input = fixture.config_dir.path().join("input.ndjson");
            std::fs::write(&input, "{\"external_id\":\"one\",\"value\":{}}\n").unwrap();
            let mut cmd = command(&fixture);
            cmd.args([
                "cache",
                "refresh",
                "apply",
                "users",
                "--owner-type",
                "system",
                "--client-refresh-id",
                "my-producer",
                "--expect-empty",
                "--input",
            ]);
            if input_from_stdin {
                cmd.args(["-", "--expected-chunk-count", "1"])
                    .write_stdin("{\"external_id\":\"one\",\"value\":{}}\n");
            } else {
                cmd.arg(&input);
            }
            let output = cmd.assert().failure().get_output().stdout.clone();
            let error: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(error["code"], "COMMAND_FAILED");
            let message = error["error"].as_str().unwrap();
            assert!(message.contains("reused generation 42"));
            assert!(message.contains("9876543210"));
            assert!(message.contains("attune cache generation show users 42 --owner-type system"));
            let requests = fixture.mock_server.received_requests().await.unwrap();
            assert_eq!(
                requests.len(),
                1,
                "reuse must stop before any write to the returned generation"
            );
            let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
            assert_eq!(body["client_refresh_id"], "my-producer");
            assert!(body.get("created_by_execution").is_none());
        }
    }
}

#[tokio::test]
async fn begin_returns_reused_metadata_and_creator_without_writes() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");
    let original = generation("other-producer", "ready", Some(9876543210));
    Mock::given(method("POST"))
        .and(path("/api/v1/cache/namespaces/users/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": original})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let output = command(&fixture)
        .args([
            "cache",
            "refresh",
            "begin",
            "users",
            "--owner-type",
            "system",
            "--client-refresh-id",
            "my-producer",
            "--expected-chunk-count",
            "1",
            "--expect-empty",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response, original);
    assert_eq!(
        fixture.mock_server.received_requests().await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn apply_same_refresh_id_replay_can_upload_and_publish() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("token", "refresh");
    for (suffix, state) in [
        ("", "staging"),
        ("/42/seal", "ready"),
        ("/42/promote", "active"),
    ] {
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v1/cache/namespaces/users/generations{suffix}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": generation("my-producer", state, None)
            })))
            .expect(1)
            .mount(&fixture.mock_server)
            .await;
    }
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/cache/namespaces/users/generations/42/chunks/0",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {}})))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;
    let input = fixture.config_dir.path().join("input.ndjson");
    std::fs::write(&input, "{\"external_id\":\"one\",\"value\":{}}\n").unwrap();
    let output = command(&fixture)
        .args([
            "cache",
            "refresh",
            "apply",
            "users",
            "--owner-type",
            "system",
            "--client-refresh-id",
            "my-producer",
            "--expect-empty",
            "--input",
        ])
        .arg(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["status"], "active");
    assert_eq!(response["created_by_execution"], Value::Null);
    assert_eq!(
        fixture.mock_server.received_requests().await.unwrap().len(),
        4
    );
}
