//! Integration tests for CLI key commands.
#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use wiremock::{
    matchers::{method, path, query_param},
    Mock, ResponseTemplate,
};

mod common;
use common::*;

fn key_response(value: serde_json::Value) -> serde_json::Value {
    json!({
        "data": {
            "id": 1,
            "ref": "api_token",
            "owner_type": "system",
            "owner": null,
            "name": "API token",
            "encrypted": true,
            "value": value,
            "created": "2024-01-01T00:00:00Z",
            "updated": "2024-01-01T00:00:01Z"
        }
    })
}

#[tokio::test]
async fn test_key_show_does_not_request_decryption_by_default() {
    let fixture = TestFixture::new().await;
    fixture.write_default_config();

    Mock::given(method("GET"))
        .and(path("/api/v1/keys/api_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(key_response(json!(null))))
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .env("ATTUNE_API_TOKEN", "valid_token")
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("key")
        .arg("show")
        .arg("api_token");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("[REDACTED]"))
        .stdout(predicate::str::contains("sha256:").not());
}

#[tokio::test]
async fn test_key_show_decrypt_adds_explicit_query() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("valid_token", "refresh_token");

    Mock::given(method("GET"))
        .and(path("/api/v1/keys/api_token"))
        .and(query_param("decrypt", "true"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(key_response(json!("revealed-secret"))),
        )
        .mount(&fixture.mock_server)
        .await;

    let mut cmd = Command::cargo_bin("attune").unwrap();
    cmd.env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .arg("--api-url")
        .arg(fixture.server_url())
        .arg("key")
        .arg("show")
        .arg("api_token")
        .arg("--decrypt");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("revealed-secret"));
}
