#![allow(deprecated)]

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
    let mut command = Command::cargo_bin("attune").unwrap();
    command
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args(["--api-url", &fixture.server_url()]);
    command
}

fn execution(id: i64) -> Value {
    json!({"id": id, "action_ref": "core.echo", "config": {"message":"old"}, "status":"requested", "created":"2026-09-30T00:00:00Z", "updated":"2026-09-30T00:00:00Z"})
}

#[tokio::test]
async fn manual_execution_paths_send_the_full_request_contract() {
    for prefix in [
        vec!["action", "execute", "core.echo"],
        vec!["run", "core.echo"],
        vec!["execution", "rerun", "42"],
    ] {
        let fixture = TestFixture::new().await;
        fixture.write_authenticated_config("valid_token", "refresh_token");
        if prefix[0] == "execution" {
            Mock::given(method("GET"))
                .and(path("/api/v1/executions/42"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"data":execution(42)})),
                )
                .expect(1)
                .mount(&fixture.mock_server)
                .await;
        }
        let expected = json!({
            "action_ref":"core.echo", "parameters":{"message":"new"},
            "env_vars":{"LOG_LEVEL":"debug", "COUNT":"3", "EMPTY":""},
            "permission_set_refs":["standard", "core.reader"],
            "artifact_retention_policy":"hours", "artifact_retention_limit":12,
            "worker_selector":{}, "worker_tolerations":[], "worker_affinity":{}, "timeout_seconds":600
        });
        Mock::given(method("POST"))
            .and(path("/api/v1/executions/execute"))
            .and(body_json(expected))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data":execution(43)})))
            .expect(1)
            .mount(&fixture.mock_server)
            .await;
        command(&fixture)
            .args(&prefix)
            .args([
                "--params-json",
                r#"{"message":"new"}"#,
                "--env",
                "LOG_LEVEL=debug",
                "--env",
                "COUNT=3",
                "--env",
                "EMPTY=",
                "--permission-set",
                "standard",
                "--permission-set",
                "core.reader",
                "--artifact-retention-policy",
                "hours",
                "--artifact-retention-limit",
                "12",
                "--worker-selector",
                "{}",
                "--worker-tolerations",
                "[]",
                "--worker-affinity",
                "{}",
                "--execution-timeout",
                "600",
            ])
            .assert()
            .success();
    }
}

#[tokio::test]
async fn permission_defaults_and_explicit_no_token_have_distinct_wire_values() {
    for disable in [false, true] {
        let fixture = TestFixture::new().await;
        fixture.write_authenticated_config("valid_token", "refresh_token");
        let mut expected = json!({"action_ref":"core.echo", "parameters":{}});
        if disable {
            expected["permission_set_refs"] = json!([]);
        }
        Mock::given(method("POST"))
            .and(path("/api/v1/executions/execute"))
            .and(body_json(expected))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data":execution(43)})))
            .expect(1)
            .mount(&fixture.mock_server)
            .await;
        let mut cmd = command(&fixture);
        cmd.args(["run", "core.echo"]);
        if disable {
            cmd.arg("--no-api-token");
        }
        cmd.assert().success();
    }
}

#[tokio::test]
async fn illegal_environment_has_a_visible_error_and_never_submits() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    command(&fixture)
        .args(["run", "core.echo", "--env", "ATTUNE_API_TOKEN=hidden"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ATTUNE_API_TOKEN"))
        .stderr(predicate::str::contains("reserved ATTUNE_ prefix"))
        .stderr(predicate::str::contains("hidden").not());
    assert!(fixture
        .mock_server
        .received_requests()
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn conflicting_permission_flags_are_rejected() {
    let fixture = TestFixture::new().await;
    command(&fixture)
        .args([
            "run",
            "core.echo",
            "--permission-set",
            "standard",
            "--no-api-token",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[tokio::test]
async fn json_environment_is_forwarded_as_strings() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");
    Mock::given(method("POST")).and(path("/api/v1/executions/execute"))
        .and(body_json(json!({"action_ref":"core.echo", "parameters":{}, "env_vars":{"COUNT":"3", "DEBUG":"true"}})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"data":execution(43)})))
        .expect(1).mount(&fixture.mock_server).await;
    command(&fixture)
        .args([
            "run",
            "core.echo",
            "--env-json",
            r#"{"COUNT":"3","DEBUG":"true"}"#,
        ])
        .assert()
        .success();
}
