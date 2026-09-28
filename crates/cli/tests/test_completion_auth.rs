//! Authentication regression tests for API-backed dynamic completion.
#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use wiremock::{
    matchers::{body_json, header, method, path, query_param},
    Mock, ResponseTemplate,
};

mod common;
use common::TestFixture;

#[tokio::test]
async fn dynamic_completion_refreshes_expired_access_token_and_persists_rotation() {
    let fixture = TestFixture::new().await;
    fixture.write_authenticated_config("expired_access", "existing_refresh");

    Mock::given(method("GET"))
        .and(path("/api/v1/actions/search"))
        .and(query_param("q", "core."))
        .and(query_param("page_size", "100"))
        .and(header("authorization", "Bearer expired_access"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "Access token expired"
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/auth/refresh"))
        .and(body_json(json!({"refresh_token": "existing_refresh"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "access_token": "refreshed_access",
                "refresh_token": "rotated_refresh"
            }
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/actions/search"))
        .and(query_param("q", "core."))
        .and(query_param("page_size", "100"))
        .and(header("authorization", "Bearer refreshed_access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"ref": "core.echo"}],
            "pagination": {
                "page": 1,
                "page_size": 100,
                "total_items": 1,
                "total_pages": 1,
                "has_previous": false,
                "has_next": false
            }
        })))
        .expect(1)
        .mount(&fixture.mock_server)
        .await;

    Command::cargo_bin("attune")
        .expect("attune binary")
        .env("XDG_CONFIG_HOME", fixture.config_dir_path())
        .env("HOME", fixture.config_dir_path())
        .args(["__complete", "--cursor", "1", "run", "core."])
        .assert()
        .success()
        .stdout(predicate::str::contains("core.echo\n"));

    let saved = std::fs::read_to_string(&fixture.config_path).expect("saved CLI config");
    assert!(saved.contains("auth_token: refreshed_access"));
    assert!(saved.contains("refresh_token: rotated_refresh"));
}
