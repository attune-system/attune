//! Integration tests for agent binary distribution endpoints
//!
//! The agent endpoints (`/api/v1/agent/binary` and `/api/v1/agent/info`) are
//! intentionally unauthenticated — the agent needs to download its binary
//! before it has JWT credentials. An optional `bootstrap_token` can restrict
//! access, but that is validated inside the handler, not via RequireAuth
//! middleware.
//!
//! The test configuration (`config.test.yaml`) does NOT include an `agent`
//! section, so both endpoints return 503 Service Unavailable. This is the
//! correct behaviour: the endpoints are reachable (no 401/404 from middleware)
//! but the feature is not configured.

use axum::http::StatusCode;

#[allow(dead_code)]
mod helpers;
use helpers::TestContext;

// ── /api/v1/agent/info ──────────────────────────────────────────────

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_agent_info_not_configured() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/api/v1/agent/info", None)
        .await
        .expect("Failed to make request");

    // Agent config is not set in config.test.yaml, so the handler returns 503.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    assert_eq!(body["error"], "Not configured");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_agent_info_no_auth_required() {
    // Verify that the endpoint is reachable WITHOUT any JWT token.
    // If RequireAuth middleware were applied, this would return 401.
    // Instead we expect 503 (not configured) — proving the endpoint
    // is publicly accessible.
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/api/v1/agent/info", None)
        .await
        .expect("Failed to make request");

    // Must NOT be 401 Unauthorized — the endpoint has no auth middleware.
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "agent/info should not require authentication"
    );
    // Should be 503 because agent config is absent.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── /api/v1/agent/binary ────────────────────────────────────────────

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_agent_binary_not_configured() {
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/api/v1/agent/binary", None)
        .await
        .expect("Failed to make request");

    // The binary endpoint is fail-closed: when no bootstrap token is configured,
    // validate_token returns 503 before binary-dir configuration is checked.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON");
    assert_eq!(body["error"], "Agent downloads disabled");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_agent_binary_no_auth_required() {
    // Same reasoning as test_agent_info_no_auth_required: the binary
    // download endpoint must be publicly accessible (no RequireAuth).
    // The route still has no auth middleware, but the handler now fails closed
    // with 503 when no bootstrap_token is configured.
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/api/v1/agent/binary", None)
        .await
        .expect("Failed to make request");

    // Must NOT be 401 Unauthorized — the endpoint has no auth middleware.
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "agent/binary should not require authentication when no bootstrap_token is configured"
    );
    // Should be 503 because agent config is absent.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_agent_binary_invalid_arch() {
    // Architecture validation (`validate_arch`) rejects unsupported values
    // with 400 Bad Request. However, in the handler the execution order is:
    //   1. validate_token (fails with 503 — downloads disabled)
    //   2. check agent config (never reached)
    //   3. validate_arch (never reached)
    //
    // So even with an invalid arch like "mips", we still get 503 before arch
    // validation is reached. The arch validation is covered by unit tests in
    // routes/agent.rs instead.
    let ctx = TestContext::new()
        .await
        .expect("Failed to create test context");

    let response = ctx
        .get("/api/v1/agent/binary?arch=mips", None)
        .await
        .expect("Failed to make request");

    // 503 from the agent-config-not-set check, NOT 400 from arch validation.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
