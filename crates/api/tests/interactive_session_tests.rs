//! Actual login/refresh/logout boundaries, with independently owned databases.
mod helpers;
use attune_common::repositories::{
    identity::{IdentityRepository, UpdateIdentityInput},
    Update,
};
use axum::http::{header, StatusCode};
use helpers::TestContext;
use serde_json::{json, Value};

async fn login(ctx: &TestContext) -> Value {
    let response = ctx
        .post(
            "/auth/register",
            json!({"login":"session-user", "password":"TestPassword123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert!(body["data"]["expires_in"].as_i64().unwrap() <= 300);
    body["data"].clone()
}
async fn refresh(ctx: &TestContext, token: &str) -> helpers::TestResponse {
    ctx.post("/auth/refresh", json!({"refresh_token":token}), None)
        .await
        .unwrap()
}

#[tokio::test]
async fn rotation_replay_revokes_successor_and_concurrent_refresh_is_single_use() {
    let ctx = TestContext::new().await.unwrap();
    let tokens = login(&ctx).await;
    let old = tokens["refresh_token"].as_str().unwrap();
    let (a, b) = tokio::join!(refresh(&ctx, old), refresh(&ctx, old));
    assert_eq!(
        [a.status(), b.status()]
            .iter()
            .filter(|s| **s == StatusCode::OK)
            .count(),
        1
    );
    let winner = if a.status() == StatusCode::OK { a } else { b };
    let next: Value = winner.json().await.unwrap();
    assert_eq!(
        refresh(&ctx, next["data"]["refresh_token"].as_str().unwrap())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn logout_and_password_update_revoke_refresh_but_new_login_works() {
    let ctx = TestContext::new().await.unwrap();
    let tokens = login(&ctx).await;
    let old = tokens["refresh_token"].as_str().unwrap();
    assert_eq!(refresh(&ctx, old).await.status(), StatusCode::OK);
    let response = ctx
        .post("/auth/session/logout", json!({"refresh_token":old}), None)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(refresh(&ctx, old).await.status(), StatusCode::UNAUTHORIZED);
    let response = ctx
        .post(
            "/auth/login",
            json!({"login":"session-user","password":"TestPassword123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let tokens: Value = response.json().await.unwrap();
    let identity = IdentityRepository::find_by_login(&ctx.pool, "session-user")
        .await
        .unwrap()
        .unwrap();
    IdentityRepository::update(
        &ctx.pool,
        identity.id,
        UpdateIdentityInput {
            password_hash: Some(attune_common::auth::hash_password("NewPassword123!").unwrap()),
            display_name: None,
            attributes: None,
            frozen: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        refresh(&ctx, tokens["data"]["refresh_token"].as_str().unwrap())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = ctx
        .post(
            "/auth/login",
            json!({"login":"session-user","password":"NewPassword123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn oidc_browser_exchange_is_bound_single_use_and_has_no_bearer_fragment() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let ctx = TestContext::new().await.unwrap();
    login(&ctx).await;
    let identity = IdentityRepository::find_by_login(&ctx.pool, "session-user")
        .await
        .unwrap()
        .unwrap();
    let tokens = attune_api::auth::session::issue(&ctx.state, &identity)
        .await
        .unwrap();
    let response = attune_api::auth::oidc::oidc_callback_redirect_response(
        &ctx.state,
        &tokens,
        Some("/executions".into()),
        "",
    )
    .await
    .unwrap();
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    assert!(!location.contains("access_token"));
    assert!(!location.contains("refresh_token"));
    let code = location.split("#code=").nth(1).unwrap();
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("attune_login_exchange="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let request = |binding: &str| {
        Request::builder()
            .method("POST")
            .uri("/auth/exchange")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, binding)
            .body(Body::from(json!({"code":code}).to_string()))
            .unwrap()
    };
    assert_eq!(
        ctx.app
            .clone()
            .oneshot(request("attune_login_exchange=wrong"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = ctx.app.clone().oneshot(request(cookie)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["data"]["redirect_to"], "/executions");
    assert_eq!(
        refresh(
            &ctx,
            body["data"]["tokens"]["refresh_token"].as_str().unwrap()
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ctx.app
            .clone()
            .oneshot(request(cookie))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ctx.cleanup().await.unwrap();
}
