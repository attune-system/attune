use std::sync::Arc;

use attune_api::{AppState, Server};
use attune_common::config::{BlobStorageConfig, Config};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

#[tokio::test]
async fn public_settings_serve_disclosure_and_branding_from_startup_config() {
    let root = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_yaml_ng::from_str(include_str!("../../../config.test.yaml")).unwrap();
    config.storage = BlobStorageConfig::Filesystem {
        root: root.path().join("blobs"),
    };
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    pool.close().await;

    let notice = "Authorized use only.\n\nRead the policy before signing in.";
    for (disclosure, expected) in [
        (None, Value::Null),
        (Some(" \n\t"), Value::Null),
        (Some(notice), json!(notice)),
    ] {
        config.security.login_disclosure = disclosure.map(str::to_owned);
        config.branding.logo_url = disclosure
            .filter(|text| *text == notice)
            .map(|_| "/branding/logo.svg".to_owned());
        let response = Server::new(Arc::new(AppState::new(pool.clone(), config.clone())))
            .router()
            .oneshot(
                Request::builder()
                    .uri("/auth/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["data"]["login_disclosure"], expected);
        assert_eq!(
            body["data"]["branding"]["logo_url"],
            json!(config.branding.logo_url)
        );
        assert_eq!(body["data"]["branding"]["favicon_url"], Value::Null);
        assert_eq!(body["data"]["local_password_enabled"], true);
        assert!(body["data"].get("jwt_secret").is_none());
    }
}
