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
    let access = attune_common::auth::jwt::validate_token(
        body["data"]["access_token"].as_str().unwrap(),
        &ctx.state.jwt_config,
    )
    .unwrap();
    assert!(access.exp - access.iat <= 300);
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
    let rotated = refresh(&ctx, old).await;
    assert_eq!(rotated.status(), StatusCode::OK);
    let successor: Value = rotated.json().await.unwrap();
    let response = ctx
        .post("/auth/session/logout", json!({"refresh_token":old}), None)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        refresh(&ctx, successor["data"]["refresh_token"].as_str().unwrap())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
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
            password_hash: Some(
                attune_api::auth::password::hash_password("NewPassword123!").unwrap(),
            ),
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
    oidc_browser_session("HTTPS://app.example.com/auth/callback", true).await;
}

#[tokio::test]
async fn oidc_browser_session_supports_loopback_http() {
    oidc_browser_session("http://localhost:3000/auth/callback", false).await;
}

async fn oidc_browser_session(callback: &str, secure: bool) {
    use attune_api::auth::oidc::{
        ACCESS_COOKIE_NAME, LOGIN_EXCHANGE_COOKIE, OIDC_REDIRECT_COOKIE_NAME, REFRESH_COOKIE_NAME,
    };
    use axum::{body::Body, http::Request, response::Response};
    use axum_extra::extract::cookie::{Cookie, SameSite};
    use std::sync::Arc;
    use tower::ServiceExt;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn checked_cookies(response: &Response, secure: bool) -> Vec<Cookie<'_>> {
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| Cookie::parse(value.to_str().unwrap()).unwrap())
            .collect();
        assert!(!cookies.is_empty());
        for cookie in &cookies {
            assert_eq!(
                cookie.secure().unwrap_or(false),
                secure,
                "{}",
                cookie.name()
            );
            let http_only = cookie.name() != OIDC_REDIRECT_COOKIE_NAME
                || cookie.max_age().is_some_and(|age| age.is_zero());
            assert_eq!(
                cookie.http_only().unwrap_or(false),
                http_only,
                "{}",
                cookie.name()
            );
            assert_eq!(cookie.same_site(), Some(SameSite::Lax), "{}", cookie.name());
            assert_eq!(cookie.path(), Some("/"), "{}", cookie.name());
        }
        cookies
    }

    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/discovery"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": provider.uri(),
            "authorization_endpoint": format!("{}/authorize", provider.uri()),
            "token_endpoint": format!("{}/token", provider.uri()),
            "jwks_uri": format!("{}/jwks", provider.uri()),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
        })))
        .expect(1)
        .mount(&provider)
        .await;
    let mut ctx = TestContext::new().await.unwrap();
    let mut config = (*ctx.state.config).clone();
    config.security.oidc = Some(
        serde_json::from_value(json!({
            "enabled": true,
            "discovery_url": format!("{}/discovery", provider.uri()),
            "client_id": "attune-web",
            "redirect_uri": callback,
        }))
        .unwrap(),
    );
    ctx.state = Arc::new(attune_api::state::AppState::new_with_audit(
        ctx.pool.clone(),
        config,
        ctx.state.audit_emitter.clone(),
    ));
    ctx.app = attune_api::server::Server::new(ctx.state.clone()).router();
    let login_redirect = ctx
        .app
        .clone()
        .oneshot(
            Request::get("/auth/oidc/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_redirect.status(), StatusCode::TEMPORARY_REDIRECT);
    let authorization_url =
        url::Url::parse(login_redirect.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    assert!(authorization_url
        .query_pairs()
        .any(|(name, value)| name == "redirect_uri" && value == callback));
    assert_eq!(checked_cookies(&login_redirect, secure).len(), 4);

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
    let callback_cookies = checked_cookies(&response, secure);
    let exchange = callback_cookies
        .iter()
        .find(|cookie| cookie.name() == LOGIN_EXCHANGE_COOKIE)
        .unwrap();
    let binding = format!("{}={}", exchange.name(), exchange.value());
    assert_eq!(
        callback_cookies
            .iter()
            .filter(|cookie| cookie.max_age().is_some_and(|age| age.is_zero()))
            .count(),
        4
    );
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
    let response = ctx.app.clone().oneshot(request(&binding)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let issued = checked_cookies(&response, secure);
    assert!(issued
        .iter()
        .any(|cookie| cookie.name() == ACCESS_COOKIE_NAME));
    let issued_refresh = issued
        .iter()
        .find(|cookie| cookie.name() == REFRESH_COOKIE_NAME)
        .unwrap();
    let refresh_credential = issued_refresh.value().to_owned();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["data"]["redirect_to"], "/executions");
    assert!(body["data"]["tokens"]["refresh_token"].as_str() == Some(&refresh_credential));
    let refreshed = ctx
        .app
        .clone()
        .oneshot(
            Request::post("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!("{REFRESH_COOKIE_NAME}={refresh_credential}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.status(), StatusCode::OK);
    let rotated = checked_cookies(&refreshed, secure);
    assert!(rotated
        .iter()
        .any(|cookie| cookie.name() == ACCESS_COOKIE_NAME));
    let rotated_refresh = rotated
        .iter()
        .find(|cookie| cookie.name() == REFRESH_COOKIE_NAME)
        .unwrap();
    assert!(rotated_refresh.value() != refresh_credential);
    let logout = ctx
        .app
        .clone()
        .oneshot(
            Request::get("/auth/logout")
                .header(
                    header::COOKIE,
                    format!("{REFRESH_COOKIE_NAME}={}", rotated_refresh.value()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::TEMPORARY_REDIRECT);
    let removed = checked_cookies(&logout, secure);
    assert!(removed
        .iter()
        .any(|cookie| cookie.name() == REFRESH_COOKIE_NAME));
    assert!(removed
        .iter()
        .all(|cookie| cookie.max_age().is_some_and(|age| age.is_zero())));
    assert_eq!(
        ctx.app
            .clone()
            .oneshot(request(&binding))
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
        refresh(&ctx, &machine).await.status(),
        StatusCode::UNAUTHORIZED
    );
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

#[tokio::test]
async fn interactive_body_refresh_keeps_only_matching_browser_cookie_in_sync() {
    use attune_api::auth::oidc::REFRESH_COOKIE_NAME;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        response::Response,
    };
    use tower::ServiceExt;

    async fn request(ctx: &TestContext, body: Option<&str>, cookie: Option<&str>) -> Response {
        let mut request = Request::builder().method("POST").uri("/auth/refresh");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, format!("{REFRESH_COOKIE_NAME}={cookie}"));
        }
        let payload = if let Some(token) = body {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json!({"refresh_token": token}).to_string())
        } else {
            Body::empty()
        };
        ctx.app
            .clone()
            .oneshot(request.body(payload).unwrap())
            .await
            .unwrap()
    }
    fn cookie(response: &Response) -> Option<String> {
        let prefix = format!("{REFRESH_COOKIE_NAME}=");
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .find_map(|value| {
                value
                    .to_str()
                    .ok()?
                    .split(';')
                    .next()?
                    .strip_prefix(prefix.as_str())
                    .map(str::to_owned)
            })
    }
    async fn body(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
    }

    let ctx = TestContext::new().await.unwrap();
    let initial = login(&ctx).await;
    let response = request(&ctx, initial["refresh_token"].as_str(), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response.headers().contains_key(header::SET_COOKIE),
        "JSON-only refresh must not create cookies"
    );
    let current = body(response).await["data"]["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();

    let other = ctx
        .post(
            "/auth/register",
            json!({"login":"other-session-user","password":"TestPassword123!"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::OK);
    let other: Value = other.json().await.unwrap();
    let other_refresh = other["data"]["refresh_token"].as_str().unwrap();
    let response = request(&ctx, Some(&current), Some(other_refresh)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response.headers().contains_key(header::SET_COOKIE),
        "Body refresh must not overwrite an unrelated cookie session"
    );
    let current = body(response).await["data"]["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        request(&ctx, None, Some(other_refresh)).await.status(),
        StatusCode::OK
    );

    let response = request(&ctx, Some(&current), Some(&current)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let rotated_cookie =
        cookie(&response).expect("Matching browser cookie must rotate with JSON refresh");
    let response_body = body(response).await;
    assert!(
        response_body["data"]["refresh_token"].as_str() == Some(rotated_cookie.as_str()),
        "Cookie and JSON credentials must match"
    );
    assert!(
        rotated_cookie != current,
        "Refresh must rotate the credential"
    );
    assert_eq!(
        request(&ctx, None, Some(&rotated_cookie)).await.status(),
        StatusCode::OK
    );
    ctx.cleanup().await.unwrap();
}
