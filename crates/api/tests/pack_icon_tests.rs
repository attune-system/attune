//! Real public response-path regressions; no database, broker or browser required.
use std::sync::Arc;

use attune_api::{AppState, Server};
use attune_common::config::{BlobStorageConfig, Config};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use sqlx::postgres::PgPoolOptions;
use tempfile::TempDir;
use tower::ServiceExt;

struct IconFixture {
    root: TempDir,
    app: Router,
}

impl IconFixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut config: Config =
            serde_yaml_ng::from_str(include_str!("../../../config.test.yaml")).unwrap();
        config.packs_base_dir = root.path().join("packs").to_string_lossy().into_owned();
        config.artifacts_dir = root.path().join("artifacts").to_string_lossy().into_owned();
        config.storage = BlobStorageConfig::Filesystem {
            root: root.path().join("blobs"),
        };
        // Closed lazy pool makes any accidental database dependency fail immediately.
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        pool.close().await;
        let state = Arc::new(AppState::new(pool, config));
        let app = Server::new(state).router();
        Self { root, app }
    }

    fn write(&self, pack: &str, filename: &str, bytes: &[u8]) {
        let dir = self.root.path().join("packs").join(pack);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(filename), bytes).unwrap();
    }

    async fn get(&self, path: &str, destination: &str) -> Response {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("sec-fetch-dest", destination)
                    // Deliberately no Authorization: icons are public metadata.
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}

async fn assert_contained(response: Response, content_type: &str, bytes: &[u8]) {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    assert_eq!(
        response.headers()[header::CONTENT_SECURITY_POLICY],
        "sandbox; default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=300"
    );
    assert!(!response.headers().contains_key(header::CONTENT_DISPOSITION));
    assert_eq!(
        to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
        bytes
    );
}

#[tokio::test]
async fn pack_icon_sandboxes_owned_active_svg_for_documents_and_embeddings() {
    let fixture = IconFixture::new().await;
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" onload="parent.postMessage('executed','*')">
<script>fetch('/api/v1/packs'); localStorage.setItem('icon-xss','yes')</script>
<foreignObject width="10" height="10"><body xmlns="http://www.w3.org/1999/xhtml"><script>alert(document.domain)</script></body></foreignObject>
<image href="https://attacker.invalid/track"/>
<rect width="10" height="10" style="fill:green"/></svg>"#;
    fixture.write("active", "pack-icon.svg", svg);
    for destination in ["document", "iframe", "object", "image"] {
        let response = fixture.get("/api/v1/packs/active/icon", destination).await;
        assert_contained(response, "image/svg+xml", svg).await;
    }
}

#[tokio::test]
async fn pack_icon_preserves_public_static_svg_and_png_bytes() {
    let fixture = IconFixture::new().await;
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><style>rect { fill: green }</style><rect width="10" height="10"/></svg>"#;
    // Valid one-pixel PNG, rather than a filename-only mock.
    use base64::{engine::general_purpose::STANDARD, Engine};
    let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNg+M/wHwAEAQH/cetH5QAAAABJRU5ErkJggg==").unwrap();
    fixture.write("static_svg", "pack-icon.svg", svg);
    fixture.write("static_png", "pack-icon.png", &png);
    for destination in ["document", "image"] {
        assert_contained(
            fixture
                .get("/api/v1/packs/static_svg/icon", destination)
                .await,
            "image/svg+xml",
            svg,
        )
        .await;
        assert_contained(
            fixture
                .get("/api/v1/packs/static_png/icon", destination)
                .await,
            "image/png",
            &png,
        )
        .await;
    }
}

#[tokio::test]
async fn pack_icon_contains_mislabeled_content_and_rejects_missing_icons() {
    let fixture = IconFixture::new().await;
    let html = b"<html><script>alert(document.domain)</script></html>";
    fixture.write("mislabeled", "pack-icon.png", html);
    assert_contained(
        fixture
            .get("/api/v1/packs/mislabeled/icon", "document")
            .await,
        "image/png",
        html,
    )
    .await;
    for path in ["/api/v1/packs/missing/icon", "/api/v1/packs/bad%20ref/icon"] {
        assert_eq!(
            fixture.get(path, "document").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    // Neighboring protected metadata must still reject anonymous requests.
    assert_eq!(
        fixture.get("/api/v1/packs", "empty").await.status(),
        StatusCode::UNAUTHORIZED
    );
}
