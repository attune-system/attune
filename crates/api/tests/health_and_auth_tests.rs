//! Integration tests for health check and authentication endpoints

use axum::http::{header, StatusCode};
use helpers::*;
use serde_json::json;

mod helpers;

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_register_debug() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": "debuguser",
                "password": "TestPassword123!",
                "display_name": "Debug User"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    let status = response.status();
    println!("Status: {}", status);

    let body_text = response.text().await.expect("Failed to get body");
    println!("Body: {}", body_text);

    // This test is just for debugging - will fail if not 201
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_health_check() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/health", None)
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");

    assert_eq!(body["status"], "ok");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_health_detailed() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/health/detailed", None)
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");

    assert_eq!(body["status"], "ok");
    assert_eq!(body["database"], "connected");
    assert!(body["version"].is_string());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_health_ready() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/health/ready", None)
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    // Readiness endpoint returns empty body with 200 status
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_health_live() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/health/live", None)
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    // Liveness endpoint returns empty body with 200 status
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_register_user() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": "newuser",
                "password": "SecurePassword123!",
                "display_name": "New User"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");

    assert!(body["data"].is_object());
    assert!(body["data"]["access_token"].is_string());
    assert!(body["data"]["refresh_token"].is_string());
    assert!(body["data"]["user"].is_object());
    assert_eq!(body["data"]["user"]["login"], "newuser");
    assert_eq!(body["data"]["user"]["display_name"], "New User");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_register_duplicate_user() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    // Register first user
    let _ = ctx
        .post(
            "/auth/register",
            json!({
                "login": "duplicate",
                "password": "SecurePassword123!",
                "display_name": "Duplicate User"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    // Try to register same user again
    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": "duplicate",
                "password": "SecurePassword123!",
                "display_name": "Duplicate User"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_register_invalid_password() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": "testuser",
                "password": "weak",
                "display_name": "Test User"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_login_success() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    // Register a user first
    let _ = ctx
        .post(
            "/auth/register",
            json!({
                "login": "loginuser",
                "password": "SecurePassword123!",
                "display_name": "Login User"
            }),
            None,
        )
        .await
        .expect("Failed to register user");

    // Now try to login
    let response = ctx
        .post(
            "/auth/login",
            json!({
                "login": "loginuser",
                "password": "SecurePassword123!"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");

    assert!(body["data"]["access_token"].is_string());
    assert!(body["data"]["refresh_token"].is_string());
    assert_eq!(body["data"]["user"]["login"], "loginuser");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_login_success_clears_browser_auth_cookies() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let _ = ctx
        .post(
            "/auth/register",
            json!({
                "login": "cookieclearuser",
                "password": "SecurePassword123!",
                "display_name": "Cookie Clear User"
            }),
            None,
        )
        .await
        .expect("Failed to register user");

    let response = ctx
        .post(
            "/auth/login",
            json!({
                "login": "cookieclearuser",
                "password": "SecurePassword123!"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let set_cookies: Vec<_> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();

    assert!(set_cookies
        .iter()
        .any(|value| { value.starts_with("attune_access_token=") && value.contains("Max-Age=0") }));
    assert!(set_cookies.iter().any(|value| {
        value.starts_with("attune_oidc_id_token=") && value.contains("Max-Age=0")
    }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_login_wrong_password() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    // Register a user first
    let _ = ctx
        .post(
            "/auth/register",
            json!({
                "login": "wrongpassuser",
                "password": "SecurePassword123!",
                "display_name": "Wrong Pass User"
            }),
            None,
        )
        .await
        .expect("Failed to register user");

    // Try to login with wrong password
    let response = ctx
        .post(
            "/auth/login",
            json!({
                "login": "wrongpassuser",
                "password": "WrongPassword123!"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_login_nonexistent_user() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/login",
            json!({
                "login": "nonexistent",
                "password": "SomePassword123!"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ── LDAP auth tests ──────────────────────────────────────────────────

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_ldap_login_returns_501_when_not_configured() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/ldap/login",
            json!({
                "login": "jdoe",
                "password": "secret"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    // LDAP is not configured in config.test.yaml, so the endpoint
    // should return 501 Not Implemented.
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_logout_clears_browser_auth_cookies() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/auth/logout", None)
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/login")
    );

    let set_cookies: Vec<_> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();

    assert!(set_cookies
        .iter()
        .any(|value| { value.starts_with("attune_access_token=") && value.contains("Max-Age=0") }));
    assert!(set_cookies
        .iter()
        .any(|value| { value.starts_with("attune_oidc_state=") && value.contains("Max-Age=0") }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_ldap_login_validates_empty_login() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/ldap/login",
            json!({
                "login": "",
                "password": "secret"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    // Validation should fail before we even check LDAP config
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_ldap_login_validates_empty_password() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/ldap/login",
            json!({
                "login": "jdoe",
                "password": ""
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_ldap_login_validates_missing_fields() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post("/auth/ldap/login", json!({}), None)
        .await
        .expect("Failed to make request");

    // Missing required fields should return 422
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── auth/settings LDAP field tests ──────────────────────────────────

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_auth_settings_includes_ldap_fields_disabled() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/auth/settings", None)
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");

    // LDAP is not configured in config.test.yaml, so these should all
    // reflect the disabled state.
    assert_eq!(body["data"]["ldap_enabled"], false);
    assert_eq!(body["data"]["ldap_visible_by_default"], false);
    assert!(body["data"]["ldap_provider_name"].is_null());
    assert!(body["data"]["ldap_provider_label"].is_null());
    assert!(body["data"]["ldap_provider_icon_url"].is_null());

    // Existing fields should still be present
    assert!(body["data"]["authentication_enabled"].is_boolean());
    assert!(body["data"]["local_password_enabled"].is_boolean());
    assert!(body["data"]["oidc_enabled"].is_boolean());
    assert!(body["data"]["self_registration_enabled"].is_boolean());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_current_user() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context")
        .with_auth()
        .await
        .expect("Failed to authenticate");

    let response = ctx
        .get("/auth/me", ctx.token())
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");

    assert!(body["data"].is_object());
    assert!(body["data"]["id"].is_number());
    assert!(body["data"]["login"].is_string());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_current_user_unauthorized() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/auth/me", None)
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_current_user_invalid_token() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/auth/me", Some("invalid-token"))
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_refresh_token() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    // Register a user first
    let register_response = ctx
        .post(
            "/auth/register",
            json!({
                "login": "refreshuser",
                "email": "refresh@example.com",
                "password": "SecurePassword123!",
                "display_name": "Refresh User"
            }),
            None,
        )
        .await
        .expect("Failed to register user");

    let register_body: serde_json::Value = register_response
        .json()
        .await
        .expect("Failed to parse JSON");

    let refresh_token = register_body["data"]["refresh_token"]
        .as_str()
        .expect("Missing refresh token");

    // Use refresh token to get new access token
    let response = ctx
        .post(
            "/auth/refresh",
            json!({
                "refresh_token": refresh_token
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");

    assert!(body["data"]["access_token"].is_string());
    assert!(body["data"]["refresh_token"].is_string());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_refresh_with_invalid_token() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .post(
            "/auth/refresh",
            json!({
                "refresh_token": "invalid-refresh-token"
            }),
            None,
        )
        .await
        .expect("Failed to make request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
