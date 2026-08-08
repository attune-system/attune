//! Integration tests for webhook API endpoints

use attune_api::authz::AuthorizationService;
use attune_common::repositories::{
    identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
        PermissionAssignmentRepository, PermissionSetRepository,
    },
    pack::{CreatePackInput, PackRepository},
    trigger::{CreateTriggerInput, TriggerRepository},
    Create,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

mod helpers;

use helpers::TestContext;

async fn setup_test_context() -> TestContext {
    TestContext::new()
        .await
        .expect("Failed to create webhook test context")
}

/// Helper to create a test pack
async fn create_test_pack(pool: &PgPool, name: &str) -> i64 {
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

    let pack = PackRepository::create(pool, input)
        .await
        .expect("Failed to create pack");

    pack.id
}

/// Helper to create a test trigger
async fn create_test_trigger(
    pool: &PgPool,
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

    let trigger = TriggerRepository::create(pool, input)
        .await
        .expect("Failed to create trigger");

    trigger.id
}

/// Helper to create a user with webhook-management access.
async fn get_auth_token(app: &axum::Router, pool: &PgPool) -> String {
    let login = format!("webhook_test_{}", uuid::Uuid::new_v4().simple());
    let register_request = json!({
        "login": login,
        "password": "TestPassword123!",
        "display_name": "Webhook Test User"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&register_request).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let token = json["data"]["access_token"].as_str().unwrap().to_string();

    let identity = IdentityRepository::find_by_login(pool, &login)
        .await
        .expect("Failed to look up webhook test identity")
        .expect("Webhook test identity was not created");
    let permission_set = PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: format!("test.webhook_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: Some("Webhook test grants".to_string()),
            description: None,
            grants: json!([
                {"resource": "triggers", "actions": ["read", "update"]}
            ]),
        },
    )
    .await
    .expect("Failed to create webhook test permission set");
    PermissionAssignmentRepository::create(
        pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await
    .expect("Failed to assign webhook test permission set");
    AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    AuthorizationService::invalidate_permission_set_caches().await;

    token
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enable_webhook() {
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data
    let pack_id = create_test_pack(&ctx.pool, "webhook_test").await;
    let _trigger_id =
        create_test_trigger(&ctx.pool, pack_id, "webhook_test", "webhook_test.trigger").await;

    // Get auth token (assumes a test user exists)
    let token = get_auth_token(&app, &ctx.pool).await;

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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data
    let pack_id = create_test_pack(&ctx.pool, "webhook_disable_test").await;
    let trigger_id = create_test_trigger(
        &ctx.pool,
        pack_id,
        "webhook_disable_test",
        "webhook_disable_test.trigger",
    )
    .await;

    // Enable webhooks first
    let _ = TriggerRepository::enable_webhook(&ctx.pool, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Get auth token
    let token = get_auth_token(&app, &ctx.pool).await;

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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data
    let pack_id = create_test_pack(&ctx.pool, "webhook_regen_test").await;
    let trigger_id = create_test_trigger(
        &ctx.pool,
        pack_id,
        "webhook_regen_test",
        "webhook_regen_test.trigger",
    )
    .await;

    // Enable webhooks first
    let original_info = TriggerRepository::enable_webhook(&ctx.pool, trigger_id)
        .await
        .expect("Failed to enable webhook");

    // Get auth token
    let token = get_auth_token(&app, &ctx.pool).await;

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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data without enabling webhooks
    let pack_id = create_test_pack(&ctx.pool, "webhook_not_enabled_test").await;
    let _trigger_id = create_test_trigger(
        &ctx.pool,
        pack_id,
        "webhook_not_enabled_test",
        "webhook_not_enabled_test.trigger",
    )
    .await;

    // Get auth token
    let token = get_auth_token(&app, &ctx.pool).await;

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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data
    let pack_id = create_test_pack(&ctx.pool, "webhook_receive_test").await;
    let trigger_id = create_test_trigger(
        &ctx.pool,
        pack_id,
        "webhook_receive_test",
        "webhook_receive_test.trigger",
    )
    .await;

    // Enable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&ctx.pool, trigger_id)
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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data
    let pack_id = create_test_pack(&ctx.pool, "webhook_disabled_test").await;
    let trigger_id = create_test_trigger(
        &ctx.pool,
        pack_id,
        "webhook_disabled_test",
        "webhook_disabled_test.trigger",
    )
    .await;

    // Enable then disable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&ctx.pool, trigger_id)
        .await
        .expect("Failed to enable webhook");

    TriggerRepository::disable_webhook(&ctx.pool, trigger_id)
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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data
    let pack_id = create_test_pack(&ctx.pool, "webhook_auth_test").await;
    let _trigger_id = create_test_trigger(
        &ctx.pool,
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
    let ctx = setup_test_context().await;
    let app = ctx.app.clone();

    // Create test data
    let pack_id = create_test_pack(&ctx.pool, "webhook_minimal_test").await;
    let trigger_id = create_test_trigger(
        &ctx.pool,
        pack_id,
        "webhook_minimal_test",
        "webhook_minimal_test.trigger",
    )
    .await;

    // Enable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&ctx.pool, trigger_id)
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
