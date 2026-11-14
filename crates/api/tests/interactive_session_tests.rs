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

#[tokio::test]
async fn frozen_deleted_and_pre_session_credentials_fail_closed() {
    use attune_common::repositories::Delete;
    let ctx = TestContext::new().await.unwrap();
    let tokens = login(&ctx).await;
    let identity = IdentityRepository::find_by_login(&ctx.pool, "session-user")
        .await
        .unwrap()
        .unwrap();
    let unrecorded = attune_common::auth::jwt::generate_refresh_token(
        identity.id,
        &identity.login,
        &ctx.state.jwt_config,
    )
    .unwrap();
    assert_eq!(
        refresh(&ctx, &unrecorded).await.status(),
        StatusCode::UNAUTHORIZED
    );
    IdentityRepository::update(
        &ctx.pool,
        identity.id,
        UpdateIdentityInput {
            display_name: None,
            password_hash: None,
            attributes: None,
            frozen: Some(true),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        refresh(&ctx, tokens["refresh_token"].as_str().unwrap())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    IdentityRepository::delete(&ctx.pool, identity.id)
        .await
        .unwrap();
    assert_eq!(
        refresh(&ctx, tokens["refresh_token"].as_str().unwrap())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn integration_refresh_retains_provenance_and_credential_revocation() {
    use attune_common::repositories::{
        integration_token::{CreateIntegrationTokenInput, IntegrationTokenRepository},
        Create, Delete,
    };
    let ctx = TestContext::new().await.unwrap();
    login(&ctx).await;
    let identity = IdentityRepository::find_by_login(&ctx.pool, "session-user")
        .await
        .unwrap()
        .unwrap();
    let record = IntegrationTokenRepository::create(
        &ctx.pool,
        CreateIntegrationTokenInput {
            identity: identity.id,
            label: "machine".into(),
            description: None,
            token_hash: "unused-test-hash".into(),
            token_prefix: "test".into(),
            token_suffix: "test".into(),
            created_by: Some(identity.id),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let machine = attune_common::auth::jwt::generate_integration_refresh_token(
        record.id,
        identity.id,
        &identity.login,
        &ctx.state.jwt_config,
    )
    .unwrap();
    let response = refresh(&ctx, &machine).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    let claims = attune_common::auth::jwt::validate_token(
        body["data"]["access_token"].as_str().unwrap(),
        &ctx.state.jwt_config,
    )
    .unwrap();
    assert_eq!(
        claims
            .integration_access_provenance()
            .unwrap()
            .integration_token_id,
        record.id
    );
    let response = ctx
        .post(
            "/auth/session/logout",
            json!({"refresh_token":machine}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(refresh(&ctx, &machine).await.status(), StatusCode::OK);
    IntegrationTokenRepository::delete(&ctx.pool, record.id)
        .await
        .unwrap();
    assert_eq!(
        refresh(&ctx, &machine).await.status(),
        StatusCode::UNAUTHORIZED
    );
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn password_change_fences_stale_issuance_and_pending_browser_exchange() {
    use attune_common::repositories::interactive_session::InteractiveSessionRepository as Sessions;
    let ctx = TestContext::new().await.unwrap();
    login(&ctx).await;
    let identity = IdentityRepository::find_by_login(&ctx.pool, "session-user")
        .await
        .unwrap()
        .unwrap();
    let tokens = attune_api::auth::session::issue(&ctx.state, &identity)
        .await
        .unwrap();
    Sessions::create_exchange(
        &ctx.pool,
        identity.id,
        "code",
        "browser",
        "encrypted",
        "/",
        &tokens.refresh_token,
    )
    .await
    .unwrap();
    let response = ctx
        .post(
            "/auth/change-password",
            json!({"current_password":"TestPassword123!","new_password":"ChangedPassword123!"}),
            Some(&tokens.access_token),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(attune_api::auth::session::issue(&ctx.state, &identity)
        .await
        .is_err());
    assert!(Sessions::consume_exchange(&ctx.pool, "code", "browser")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        refresh(&ctx, &tokens.refresh_token).await.status(),
        StatusCode::UNAUTHORIZED
    );
    ctx.cleanup().await.unwrap();
}

#[tokio::test]
async fn browser_exchange_expiry_freeze_and_concurrent_consume_are_fenced() {
    use attune_common::repositories::interactive_session::InteractiveSessionRepository as Sessions;
    let ctx = TestContext::new().await.unwrap();
    let tokens = login(&ctx).await;
    let identity = IdentityRepository::find_by_login(&ctx.pool, "session-user")
        .await
        .unwrap()
        .unwrap();
    let refresh = tokens["refresh_token"].as_str().unwrap();
    Sessions::create_exchange(
        &ctx.pool,
        identity.id,
        "code",
        "browser",
        "encrypted",
        "/",
        refresh,
    )
    .await
    .unwrap();
    IdentityRepository::update(
        &ctx.pool,
        identity.id,
        UpdateIdentityInput {
            display_name: None,
            password_hash: None,
            attributes: None,
            frozen: Some(true),
        },
    )
    .await
    .unwrap();
    assert!(Sessions::consume_exchange(&ctx.pool, "code", "browser")
        .await
        .unwrap()
        .is_none());
    IdentityRepository::update(
        &ctx.pool,
        identity.id,
        UpdateIdentityInput {
            display_name: None,
            password_hash: None,
            attributes: None,
            frozen: Some(false),
        },
    )
    .await
    .unwrap();
    let (a, b) = tokio::join!(
        Sessions::consume_exchange(&ctx.pool, "code", "browser"),
        Sessions::consume_exchange(&ctx.pool, "code", "browser")
    );
    assert_eq!(
        [a.unwrap(), b.unwrap()]
            .iter()
            .filter(|v| v.is_some())
            .count(),
        1
    );
    Sessions::create_exchange(
        &ctx.pool,
        identity.id,
        "expired",
        "browser",
        "encrypted",
        "/",
        refresh,
    )
    .await
    .unwrap();
    // Owned fixture mutation, no clock sleeps or process-global clock state.
    sqlx::query("UPDATE interactive_login_exchange SET expires=clock_timestamp()-interval '1 second' WHERE code_hash=$1").bind(attune_common::repositories::interactive_session::token_hash("expired")).execute(&ctx.pool).await.unwrap();
    assert!(Sessions::consume_exchange(&ctx.pool, "expired", "browser")
        .await
        .unwrap()
        .is_none());
    ctx.cleanup().await.unwrap();
}
