#![allow(deprecated)]
mod common;

use assert_cmd::Command;
use attune_common::build_info::BuildInfo;
use common::TestFixture;
use serde_json::{json, Value};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn command(fixture: &TestFixture, binary: &str) -> Command {
    let mut cmd = Command::cargo_bin(binary).unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .env_remove("ATTUNE_PROFILE")
        .env_remove("ATTUNE_API_URL")
        .env_remove("ATTUNE_API_TOKEN")
        .env_remove("ATTUNE_AUTH_TOKEN")
        .env_remove("ATTUNE_REFRESH_TOKEN");
    cmd
}

#[tokio::test]
async fn cli_and_mcp_query_the_selected_profile_and_keep_versions_separate() {
    for binary in ["attune", "attune-mcp"] {
        let fixture = TestFixture::new().await;
        let default_server = MockServer::start().await;
        fixture.write_config(&format!("current_profile: default\nprofiles:\n  default:\n    api_url: {}\n  staging:\n    api_url: {}\n", default_server.uri(), fixture.server_url()));
        Mock::given(method("GET"))
            .and(path("/api/v1/info"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"data":{"version":"9.8.7", "git_sha":"a".repeat(40)}})),
            )
            .expect(1)
            .mount(&fixture.mock_server)
            .await;
        let mut cmd = command(&fixture, binary);
        cmd.args(["--profile", "staging"]);
        if binary == "attune" {
            cmd.args(["--json", "info"]);
        } else {
            cmd.arg("--info");
        }
        let output = cmd.assert().success().get_output().stdout.clone();
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["local"]["binary"], binary);
        assert_eq!(report["local"]["version"], BuildInfo::current().version);
        assert_eq!(report["local"]["git_sha"], BuildInfo::current().git_sha);
        assert_eq!(report["server"]["version"], "9.8.7");
        assert_eq!(report["server"]["git_sha"], "a".repeat(40));
        assert_eq!(report["server"]["profile"], "staging");
        assert_eq!(report["server"]["api_url"], fixture.server_url());
        assert!(default_server.received_requests().await.unwrap().is_empty());
        assert!(std::fs::read_to_string(&fixture.config_path)
            .unwrap()
            .starts_with("current_profile: default"));
    }
}

#[tokio::test]
async fn server_failures_preserve_local_info_and_return_nonzero() {
    for binary in ["attune", "attune-mcp"] {
        let fixture = TestFixture::new().await;
        fixture.write_authenticated_config("test-token", "refresh-token");
        Mock::given(method("GET"))
            .and(path("/api/v1/info"))
            .respond_with(
                ResponseTemplate::new(503).set_body_json(json!({"error":"Server unavailable"})),
            )
            .expect(1)
            .mount(&fixture.mock_server)
            .await;
        let mut cmd = command(&fixture, binary);
        if binary == "attune" {
            cmd.args(["--json", "info"]);
        } else {
            cmd.arg("--info");
        }
        let output = cmd.assert().failure().get_output().stdout.clone();
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["local"]["version"], BuildInfo::current().version);
        assert_eq!(report["server"]["status"], "unavailable");
        assert_eq!(report["server"]["profile"], "default");
        assert!(report["server"]["error"]
            .as_str()
            .unwrap()
            .contains("Server unavailable"));
        assert!(report["server"].get("version").is_none());
    }
}

#[tokio::test]
async fn local_info_works_without_a_profile_and_ignores_runtime_sha_overrides() {
    for binary in ["attune", "attune-mcp"] {
        let fixture = TestFixture::new().await;
        let mut cmd = command(&fixture, binary);
        cmd.env(
            "ATTUNE_BUILD_GIT_SHA",
            "runtime-value-must-not-replace-build-metadata",
        );
        if binary == "attune" {
            cmd.args(["--json", "info", "--local"]);
        } else {
            cmd.args(["--info", "--local"]);
        }
        let output = cmd.assert().success().get_output().stdout.clone();
        let local: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(local["binary"], binary);
        assert_eq!(local["git_sha"], BuildInfo::current().git_sha);
        assert!(!fixture.config_path.exists());
        assert!(fixture
            .mock_server
            .received_requests()
            .await
            .unwrap()
            .is_empty());
    }
}
