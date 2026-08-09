//! Integration tests for webhook functionality

use attune_common::models::trigger::Trigger;
use attune_common::repositories::trigger::{CreateTriggerInput, TriggerRepository};
use attune_common::repositories::{Create, FindById};
use sqlx::PgPool;

mod helpers;

async fn setup_test_db() -> PgPool {
    helpers::create_test_pool()
        .await
        .expect("Failed to create isolated webhook test pool")
}

async fn create_test_trigger(pool: &PgPool) -> Trigger {
    let input = CreateTriggerInput {
        r#ref: format!("test.webhook_trigger_{}", uuid::Uuid::new_v4()),
        pack: None,
        pack_ref: Some("test".to_string()),
        label: "Test Webhook Trigger".to_string(),
        description: Some("A test trigger for webhook functionality".to_string()),
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    TriggerRepository::create(pool, input)
        .await
        .expect("Failed to create test trigger")
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_enable() {
    let pool = setup_test_db().await;
    let trigger = create_test_trigger(&pool).await;

    // Initially, webhook should be disabled
    assert!(!trigger.webhook_enabled);
    assert!(trigger.webhook_key.is_none());

    // Enable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&pool, trigger.id)
        .await
        .expect("Failed to enable webhook");

    // Verify webhook info
    assert!(webhook_info.enabled);
    assert!(webhook_info.webhook_key.starts_with("wh_"));
    assert_eq!(webhook_info.webhook_key.len(), 35); // "wh_" + 32 chars
    assert!(webhook_info.webhook_url.contains(&webhook_info.webhook_key));

    // Fetch trigger again to verify database state
    let updated_trigger = TriggerRepository::find_by_id(&pool, trigger.id)
        .await
        .expect("Failed to fetch trigger")
        .expect("Trigger not found");

    assert!(updated_trigger.webhook_enabled);
    assert_eq!(
        updated_trigger.webhook_key.as_ref().unwrap(),
        &webhook_info.webhook_key
    );

    // Cleanup
    sqlx::query("DELETE FROM trigger WHERE id = $1")
        .bind(trigger.id)
        .execute(&pool)
        .await
        .expect("Failed to cleanup");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_disable() {
    let pool = setup_test_db().await;
    let trigger = create_test_trigger(&pool).await;

    // Enable webhooks first
    let webhook_info = TriggerRepository::enable_webhook(&pool, trigger.id)
        .await
        .expect("Failed to enable webhook");

    let webhook_key = webhook_info.webhook_key.clone();

    // Disable webhooks
    let result = TriggerRepository::disable_webhook(&pool, trigger.id)
        .await
        .expect("Failed to disable webhook");

    assert!(result);

    // Fetch trigger to verify
    let updated_trigger = TriggerRepository::find_by_id(&pool, trigger.id)
        .await
        .expect("Failed to fetch trigger")
        .expect("Trigger not found");

    assert!(!updated_trigger.webhook_enabled);
    // Disabling clears the active webhook key.
    assert_ne!(webhook_key, "");
    assert!(updated_trigger.webhook_key.is_none());

    // Cleanup
    sqlx::query("DELETE FROM trigger WHERE id = $1")
        .bind(trigger.id)
        .execute(&pool)
        .await
        .expect("Failed to cleanup");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_key_regeneration() {
    let pool = setup_test_db().await;
    let trigger = create_test_trigger(&pool).await;

    // Enable webhooks
    let initial_info = TriggerRepository::enable_webhook(&pool, trigger.id)
        .await
        .expect("Failed to enable webhook");

    let old_key = initial_info.webhook_key.clone();

    // Regenerate key
    let regenerate_result = TriggerRepository::regenerate_webhook_key(&pool, trigger.id)
        .await
        .expect("Failed to regenerate webhook key");

    assert!(regenerate_result.previous_key_revoked);
    assert_ne!(regenerate_result.webhook_key, old_key);
    assert!(regenerate_result.webhook_key.starts_with("wh_"));

    // Fetch trigger to verify new key
    let updated_trigger = TriggerRepository::find_by_id(&pool, trigger.id)
        .await
        .expect("Failed to fetch trigger")
        .expect("Trigger not found");

    assert_eq!(
        updated_trigger.webhook_key.as_ref().unwrap(),
        &regenerate_result.webhook_key
    );

    // Cleanup
    sqlx::query("DELETE FROM trigger WHERE id = $1")
        .bind(trigger.id)
        .execute(&pool)
        .await
        .expect("Failed to cleanup");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_webhook_key() {
    let pool = setup_test_db().await;
    let trigger = create_test_trigger(&pool).await;

    // Enable webhooks
    let webhook_info = TriggerRepository::enable_webhook(&pool, trigger.id)
        .await
        .expect("Failed to enable webhook");

    // Find by webhook key
    let found_trigger = TriggerRepository::find_by_webhook_key(&pool, &webhook_info.webhook_key)
        .await
        .expect("Failed to find trigger by webhook key")
        .expect("Trigger not found");

    assert_eq!(found_trigger.id, trigger.id);
    assert_eq!(found_trigger.r#ref, trigger.r#ref);
    assert!(found_trigger.webhook_enabled);

    // Test with invalid key
    let not_found =
        TriggerRepository::find_by_webhook_key(&pool, "wh_invalid_key_12345678901234567890")
            .await
            .expect("Query failed");

    assert!(not_found.is_none());

    // Cleanup
    sqlx::query("DELETE FROM trigger WHERE id = $1")
        .bind(trigger.id)
        .execute(&pool)
        .await
        .expect("Failed to cleanup");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_webhook_key_uniqueness() {
    let pool = setup_test_db().await;
    let trigger1 = create_test_trigger(&pool).await;
    let trigger2 = create_test_trigger(&pool).await;

    // Enable webhooks for both triggers
    let info1 = TriggerRepository::enable_webhook(&pool, trigger1.id)
        .await
        .expect("Failed to enable webhook for trigger 1");

    let info2 = TriggerRepository::enable_webhook(&pool, trigger2.id)
        .await
        .expect("Failed to enable webhook for trigger 2");

    // Keys should be different
    assert_ne!(info1.webhook_key, info2.webhook_key);

    // Both should be valid format
    assert!(info1.webhook_key.starts_with("wh_"));
    assert!(info2.webhook_key.starts_with("wh_"));

    // Cleanup
    sqlx::query("DELETE FROM trigger WHERE id IN ($1, $2)")
        .bind(trigger1.id)
        .bind(trigger2.id)
        .execute(&pool)
        .await
        .expect("Failed to cleanup");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enable_webhook_idempotent() {
    let pool = setup_test_db().await;
    let trigger = create_test_trigger(&pool).await;

    // Enable webhooks first time
    let info1 = TriggerRepository::enable_webhook(&pool, trigger.id)
        .await
        .expect("Failed to enable webhook");

    // Enable webhooks second time (should return same key)
    let info2 = TriggerRepository::enable_webhook(&pool, trigger.id)
        .await
        .expect("Failed to enable webhook again");

    // Should return the same key
    assert_eq!(info1.webhook_key, info2.webhook_key);
    assert!(info2.enabled);

    // Cleanup
    sqlx::query("DELETE FROM trigger WHERE id = $1")
        .bind(trigger.id)
        .execute(&pool)
        .await
        .expect("Failed to cleanup");
}
