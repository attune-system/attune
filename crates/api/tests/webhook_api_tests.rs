//! Integration tests for webhook API endpoints

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
    let config = Config::load().expect("Failed to load config");
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

/// Helper to get JWT token for authenticated requests
async fn get_auth_token(app: &axum::Router, username: &str, password: &str) -> String {
    let login_request = json!({
        "username": username,
        "password": password
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&login_request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    json["data"]["access_token"].as_str().unwrap().to_string()
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enable_webhook() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "webhook_test").await;
    let _trigger_id =
        create_test_trigger(&state, pack_id, "webhook_test", "webhook_test.trigger").await;

    // Get auth token (assumes a test user exists)
    let token = get_auth_token(&app, "test_user", "test_password").await;

    // Enable webhooks
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/triggers/webhook_test.trigger/webhooks/enable")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify response structure
    assert!(json["data"]["webhook_enabled"].as_bool().unwrap());
    assert!(json["data"]["webhook_key"].is_string());
    let webhook_key = json["data"]["webhook_key"].as_str().unwrap();
    assert!(webhook_key.starts_with("wh_"));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_disable_webhook() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "webhook_disable_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "webhook_disable_test",
        "webhook_disable_test.trigger",
    )
    .await;

    // Enable webhooks first
    let _ = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Get auth token
    let token = get_auth_token(&app, "test_user", "test_password").await;

    // Disable webhooks
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/triggers/webhook_disable_test.trigger/webhooks/disable")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify webhooks are disabled
    assert!(!json["data"]["webhook_enabled"].as_bool().unwrap());
    assert!(json["data"]["webhook_key"].is_null());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_regenerate_webhook_key() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "webhook_regen_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "webhook_regen_test",
        "webhook_regen_test.trigger",
    )
    .await;

    // Enable webhooks first
    let original_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Get auth token
    let token = get_auth_token(&app, "test_user", "test_password").await;

    // Regenerate webhook key
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/triggers/webhook_regen_test.trigger/webhooks/regenerate")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify new key is different from original
    let new_key = json["data"]["webhook_key"].as_str().unwrap();
    assert_ne!(new_key, original_info.webhook_key);
    assert!(new_key.starts_with("wh_"));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_regenerate_webhook_key_not_enabled() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data without enabling webhooks
    let pack_id = create_test_pack(&state, "webhook_not_enabled_test").await;
    let _trigger_id = create_test_trigger(
        &state,
        pack_id,
        "webhook_not_enabled_test",
        "webhook_not_enabled_test.trigger",
    )
    .await;

    // Get auth token
    let token = get_auth_token(&app, "test_user", "test_password").await;

    // Try to regenerate without enabling first
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/triggers/webhook_not_enabled_test.trigger/webhooks/regenerate")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_receive_webhook() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "webhook_receive_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "webhook_receive_test",
        "webhook_receive_test.trigger",
    )
    .await;

    // Enable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Send webhook
    let webhook_payload = json!({
        "payload": {
            "event": "test_event",
            "data": {
                "foo": "bar",
                "number": 42
            }
        },
        "headers": {
            "X-Test-Header": "test-value"
        },
        "source_ip": "192.168.1.1",
        "user_agent": "Test Agent/1.0"
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

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify response
    assert!(json["data"]["event_id"].is_number());
    assert_eq!(
        json["data"]["trigger_ref"].as_str().unwrap(),
        "webhook_receive_test.trigger"
    );
    assert!(json["data"]["received_at"].is_string());
    assert_eq!(
        json["data"]["message"].as_str().unwrap(),
        "Webhook received successfully"
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_receive_webhook_invalid_key() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state));
    let app = server.router();

    // Try to send webhook with invalid key
    let webhook_payload = json!({
        "payload": {
            "event": "test_event"
        }
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/wh_invalid_key_12345")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&webhook_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_receive_webhook_disabled() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "webhook_disabled_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "webhook_disabled_test",
        "webhook_disabled_test.trigger",
    )
    .await;

    // Enable then disable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    TriggerRepository::disable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to disable webhook");

    // Try to send webhook with disabled key
    let webhook_payload = json!({
        "payload": {
            "event": "test_event"
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

    // Should return 404 because disabled webhook keys are not found
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_requires_auth_for_management() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "webhook_auth_test").await;
    let _trigger_id = create_test_trigger(
        &state,
        pack_id,
        "webhook_auth_test",
        "webhook_auth_test.trigger",
    )
    .await;

    // Try to enable without auth
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/triggers/webhook_auth_test.trigger/webhooks/enable")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_receive_webhook_minimal_payload() {
    let state = setup_test_state().await;
    let server = Server::new(std::sync::Arc::new(state.clone()));
    let app = server.router();

    // Create test data
    let pack_id = create_test_pack(&state, "webhook_minimal_test").await;
    let trigger_id = create_test_trigger(
        &state,
        pack_id,
        "webhook_minimal_test",
        "webhook_minimal_test.trigger",
    )
    .await;

    // Enable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&state.db, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Send webhook with minimal payload (only required fields)
    let webhook_payload = json!({
        "payload": {
            "message": "minimal test"
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
