//! Comprehensive integration tests for webhook security features (Phase 3)
//!
//! Tests cover:
//! - HMAC signature verification (SHA256, SHA512, SHA1)
//! - Rate limiting
//! - IP whitelisting
//! - Payload size limits
//! - Event logging
//! - Error scenarios

use attune_api::{AppState, Server};
use attune_common::{
    config::Config,
    db::Database,
    repositories::{
        pack::{CreatePackInput, PackRepository},
        trigger::{CreateTriggerInput, TriggerRepository},
        Create,
    },
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use tower::ServiceExt;

/// Helper to create test database and state
async fn setup_test_state() -> AppState {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);
    let config = Config::load_from_file(&config_path).expect("Failed to load config");
    let database = Database::new(&config.database)
        .await
        .expect("Failed to connect to database");

    AppState::new(database.pool().clone(), config)
}

/// Helper to create a test pack
async fn create_test_pack(state: &AppState, name: &str) -> i64 {
    let input = CreatePackInput {
        r#ref: name.to_string(),
        label: format!("{} Pack", name),
        description: Some(format!("Test pack for {}", name)),
        version: "1.0.0".to_string(),
        conf_schema: serde_json::json!({}),
        config: serde_json::json!({}),
        meta: serde_json::json!({}),
        tags: vec![],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    let pack = PackRepository::create(&state.db, input)
        .await
        .expect("Failed to create pack");

    pack.id
}

/// Helper to create a test trigger
async fn create_test_trigger(
    state: &AppState,
    pack_id: i64,
    pack_ref: &str,
    trigger_ref: &str,
) -> i64 {
    let input = CreateTriggerInput {
        r#ref: trigger_ref.to_string(),
        pack: Some(pack_id),
        pack_ref: Some(pack_ref.to_string()),
        label: format!("{} Trigger", trigger_ref),
        description: Some(format!("Test trigger {}", trigger_ref)),
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&state.db, input)
        .await
        .expect("Failed to create trigger");

    trigger.id
}

/// Helper to generate HMAC signature
fn generate_hmac_signature(payload: &[u8], secret: &str, algorithm: &str) -> String {
    use hmac::{digest::KeyInit, Hmac, Mac};
    use sha1::Sha1;
    use sha2::{Sha256, Sha512};

    match algorithm {
        "sha256" => {
            type HmacSha256 = Hmac<Sha256>;
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
            mac.update(payload);
            let result = mac.finalize();
            format!("sha256={}", hex::encode(result.into_bytes()))
        }
        "sha512" => {
            type HmacSha512 = Hmac<Sha512>;
            let mut mac = HmacSha512::new_from_slice(secret.as_bytes()).unwrap();
            mac.update(payload);
            let result = mac.finalize();
            format!("sha512={}", hex::encode(result.into_bytes()))
        }
        "sha1" => {
            type HmacSha1 = Hmac<Sha1>;
            let mut mac = HmacSha1::new_from_slice(secret.as_bytes()).unwrap();
            mac.update(payload);
            let result = mac.finalize();
            format!("sha1={}", hex::encode(result.into_bytes()))
        }
        _ => panic!("Unsupported algorithm: {}", algorithm),
    }
}

// ============================================================================
// HMAC SIGNATURE TESTS
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_hmac_sha256_valid() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "hmac_sha256_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "hmac_sha256_test",
        "hmac_sha256_test.trigger",
    )
    .await;

    // Enable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Configure HMAC
    let hmac_secret = "test-secret-key-12345";
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha256',
         webhook_hmac_secret = $1
         WHERE id = $2",
    )
    .bind(hmac_secret)
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure HMAC");

    // Prepare webhook payload
    let webhook_payload = json!({
        "payload": {
            "event": "test_event",
            "data": {"foo": "bar"}
        }
    });
    let payload_bytes = serde_json::to_vec(&webhook_payload).unwrap();

    // Generate valid signature
    let signature = generate_hmac_signature(&payload_bytes, hmac_secret, "sha256");

    // Send webhook with valid signature
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-webhook-signature", signature)
                .body(Body::from(payload_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_hmac_sha512_valid() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "hmac_sha512_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "hmac_sha512_test",
        "hmac_sha512_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    let hmac_secret = "test-secret-sha512";
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha512',
         webhook_hmac_secret = $1
         WHERE id = $2",
    )
    .bind(hmac_secret)
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure HMAC");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });
    let payload_bytes = serde_json::to_vec(&webhook_payload).unwrap();
    let signature = generate_hmac_signature(&payload_bytes, hmac_secret, "sha512");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-webhook-signature", signature)
                .body(Body::from(payload_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_hmac_invalid_signature() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "hmac_invalid_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "hmac_invalid_test",
        "hmac_invalid_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    let hmac_secret = "test-secret-key";
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha256',
         webhook_hmac_secret = $1
         WHERE id = $2",
    )
    .bind(hmac_secret)
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure HMAC");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Send with invalid signature
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-webhook-signature", "sha256=invalid_signature_here")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_hmac_missing_signature() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "hmac_missing_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "hmac_missing_test",
        "hmac_missing_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha256',
         webhook_hmac_secret = 'secret'
         WHERE id = $1",
    )
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure HMAC");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Send without signature header
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_hmac_wrong_secret() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "hmac_wrong_secret_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "hmac_wrong_secret_test",
        "hmac_wrong_secret_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    let hmac_secret = "correct-secret";
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha256',
         webhook_hmac_secret = $1
         WHERE id = $2",
    )
    .bind(hmac_secret)
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure HMAC");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });
    let payload_bytes = serde_json::to_vec(&webhook_payload).unwrap();

    // Generate signature with wrong secret
    let wrong_signature = generate_hmac_signature(&payload_bytes, "wrong-secret", "sha256");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-webhook-signature", wrong_signature)
                .body(Body::from(payload_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================================
// RATE LIMITING TESTS
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_rate_limit_enforced() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));

    let pack_id = create_test_pack(&state, "rate_limit_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "rate_limit_test",
        "rate_limit_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Configure rate limit: 3 requests per 60 seconds
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_rate_limit_enabled = true,
         webhook_rate_limit_requests = 3,
         webhook_rate_limit_window_seconds = 60
         WHERE id = $1",
    )
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure rate limit");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Send 3 requests (should succeed)
    for i in 0..3 {
        let app = server.router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Request {} should succeed",
            i + 1
        );
    }

    // 4th request should be rate limited
    let app = server.router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_rate_limit_disabled() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));

    let pack_id = create_test_pack(&state, "no_rate_limit_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "no_rate_limit_test",
        "no_rate_limit_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Ensure rate limiting is disabled (default)
    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Send multiple requests - all should succeed
    for _ in 0..10 {
        let app = server.router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}

// ============================================================================
// IP WHITELISTING TESTS
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_ip_whitelist_allowed() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "ip_whitelist_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "ip_whitelist_test",
        "ip_whitelist_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Configure IP whitelist
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_ip_whitelist_enabled = true,
         webhook_ip_whitelist = ARRAY['192.168.1.0/24', '10.0.0.1']
         WHERE id = $1",
    )
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure IP whitelist");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Test with allowed IP in CIDR range
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-forwarded-for", "192.168.1.100")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Test with exact match IP
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-forwarded-for", "10.0.0.1")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_ip_whitelist_blocked() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "ip_blocked_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "ip_blocked_test",
        "ip_blocked_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_ip_whitelist_enabled = true,
         webhook_ip_whitelist = ARRAY['192.168.1.0/24']
         WHERE id = $1",
    )
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure IP whitelist");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Test with IP not in whitelist
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-forwarded-for", "8.8.8.8")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// PAYLOAD SIZE LIMIT TESTS
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_payload_size_limit_enforced() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "size_limit_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "size_limit_test",
        "size_limit_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Set small payload limit: 1 KB
    sqlx::query("UPDATE attune.trigger SET webhook_payload_size_limit_kb = 1 WHERE id = $1")
        .bind(trigger_id)
        .execute(&state.db)
        .await
        .expect("Failed to set payload size limit");

    // Create a large payload (> 1 KB)
    let large_data = "x".repeat(2000);
    let webhook_payload = json!({
        "payload": {
            "large_field": large_data
        }
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_payload_size_within_limit() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "size_ok_test").await;
    let trigger_id =
        create_test_trigger(&state, pack_id, "size_ok_test", "size_ok_test.trigger").await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Set payload limit: 10 KB
    sqlx::query("UPDATE attune.trigger SET webhook_payload_size_limit_kb = 10 WHERE id = $1")
        .bind(trigger_id)
        .execute(&state.db)
        .await
        .expect("Failed to set payload size limit");

    // Create a small payload (< 10 KB)
    let webhook_payload = json!({
        "payload": {
            "message": "This is a small payload"
        }
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// ============================================================================
// EVENT LOGGING TESTS
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_webhook_event_logging_success() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "event_log_test").await;
    let trigger_id =
        create_test_trigger(&state, pack_id, "event_log_test", "event_log_test.trigger").await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-forwarded-for", "192.168.1.1")
                .header("user-agent", "TestAgent/1.0")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify event was logged
    let log_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM attune.webhook_event_log WHERE trigger_id = $1")
            .bind(trigger_id)
            .fetch_one(&state.db)
            .await
            .expect("Failed to check event log");

    assert!(log_count.0 > 0, "Event should be logged");

    // Check log details
    let log: (i32, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status_code, source_ip, user_agent FROM attune.webhook_event_log
         WHERE trigger_id = $1 ORDER BY created DESC LIMIT 1",
    )
    .bind(trigger_id)
    .fetch_one(&state.db)
    .await
    .expect("Failed to fetch log details");

    assert_eq!(log.0, 200);
    assert_eq!(log.1.as_deref(), Some("192.168.1.1"));
    assert_eq!(log.2.as_deref(), Some("TestAgent/1.0"));
}

#[tokio::test]
#[ignore]
async fn test_webhook_event_logging_failure() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "event_log_fail_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "event_log_fail_test",
        "event_log_fail_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Configure HMAC to force failure
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha256',
         webhook_hmac_secret = 'secret'
         WHERE id = $1",
    )
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure HMAC");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Send without signature (should fail)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Verify failure was logged
    let log: (i32, Option<String>, Option<bool>) = sqlx::query_as(
        "SELECT status_code, error_message, hmac_verified FROM attune.webhook_event_log
         WHERE trigger_id = $1 ORDER BY created DESC LIMIT 1",
    )
    .bind(trigger_id)
    .fetch_one(&state.db)
    .await
    .expect("Failed to fetch log details");

    assert_eq!(log.0, 401);
    assert!(log.1.is_some());
    assert_eq!(log.2, Some(false));
}

// ============================================================================
// COMBINED SECURITY FEATURES TESTS
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_webhook_all_security_features_pass() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "all_features_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "all_features_test",
        "all_features_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    let hmac_secret = "all-features-secret";

    // Enable all security features
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha256',
         webhook_hmac_secret = $1,
         webhook_rate_limit_enabled = true,
         webhook_rate_limit_requests = 10,
         webhook_rate_limit_window_seconds = 60,
         webhook_ip_whitelist_enabled = true,
         webhook_ip_whitelist = ARRAY['192.168.1.0/24'],
         webhook_payload_size_limit_kb = 10
         WHERE id = $2",
    )
    .bind(hmac_secret)
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure all features");

    let webhook_payload = json!({
        "payload": {"message": "test with all features"}
    });
    let payload_bytes = serde_json::to_vec(&webhook_payload).unwrap();
    let signature = generate_hmac_signature(&payload_bytes, hmac_secret, "sha256");

    // Send webhook that passes all checks
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-webhook-signature", signature)
                .header("x-forwarded-for", "192.168.1.50")
                .header("user-agent", "TestClient/1.0")
                .body(Body::from(payload_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify event log shows all checks passed
    let log: (Option<bool>, bool, Option<bool>) = sqlx::query_as(
        "SELECT hmac_verified, rate_limited, ip_allowed FROM attune.webhook_event_log
         WHERE trigger_id = $1 ORDER BY created DESC LIMIT 1",
    )
    .bind(trigger_id)
    .fetch_one(&state.db)
    .await
    .expect("Failed to fetch log details");

    assert_eq!(log.0, Some(true)); // HMAC verified
    assert!(!log.1); // Not rate limited
    assert_eq!(log.2, Some(true)); // IP allowed
}

#[tokio::test]
#[ignore]
async fn test_webhook_multiple_security_failures() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "multi_fail_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "multi_fail_test",
        "multi_fail_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Enable multiple security features
    sqlx::query(
        "UPDATE attune.trigger SET
         webhook_hmac_enabled = true,
         webhook_hmac_algorithm = 'sha256',
         webhook_hmac_secret = 'secret',
         webhook_ip_whitelist_enabled = true,
         webhook_ip_whitelist = ARRAY['10.0.0.0/8']
         WHERE id = $1",
    )
    .bind(trigger_id)
    .execute(&state.db)
    .await
    .expect("Failed to configure features");

    let webhook_payload = json!({
        "payload": {"message": "test"}
    });

    // Send webhook that fails multiple checks (wrong IP, missing signature)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .header("x-forwarded-for", "8.8.8.8") // Wrong IP
                // Missing signature header
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should fail on IP check first
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// EDGE CASES AND ERROR SCENARIOS
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_webhook_malformed_json() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "malformed_json_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "malformed_json_test",
        "malformed_json_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Send malformed JSON
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .body(Body::from("{invalid json here"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore]
async fn test_webhook_empty_payload() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    let pack_id = create_test_pack(&state, "empty_payload_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "empty_payload_test",
        "empty_payload_test.trigger",
    )
    .await;

    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Send empty body
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/webhooks/{}", webhook_info.webhook_key))
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
