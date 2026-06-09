//! Integration tests for Trigger repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Trigger repository.

mod helpers;

use attune_common::{
    repositories::{
        trigger::{CreateTriggerInput, TriggerRepository, UpdateTriggerInput},
        Create, Delete, FindById, FindByRef, List, Patch, Update,
    },
    Error,
};
use helpers::*;
use serde_json::json;

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_trigger() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateTriggerInput {
        r#ref: format!("{}.webhook", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Webhook Trigger".to_string(),
        description: Some("Test webhook trigger".to_string()),
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    assert!(trigger.r#ref.contains(".webhook"));
    assert_eq!(trigger.pack, Some(pack.id));
    assert_eq!(trigger.pack_ref, Some(pack.r#ref));
    assert_eq!(trigger.label, "Webhook Trigger");
    assert!(trigger.enabled);
    assert!(trigger.created.timestamp() > 0);
    assert!(trigger.updated.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_trigger_without_pack() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("standalone_trigger"));
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "Standalone Trigger".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    assert_eq!(trigger.r#ref, trigger_ref);
    assert_eq!(trigger.pack, None);
    assert_eq!(trigger.pack_ref, None);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_trigger_with_schemas() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("schema_pack")
        .create(&pool)
        .await
        .unwrap();

    let param_schema = json!({
        "type": "object",
        "properties": {
            "url": {"type": "string"},
            "method": {"type": "string", "enum": ["GET", "POST"]}
        },
        "required": ["url"]
    });

    let out_schema = json!({
        "type": "object",
        "properties": {
            "status": {"type": "integer"},
            "body": {"type": "string"}
        }
    });

    let input = CreateTriggerInput {
        r#ref: format!("{}.http_trigger", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "HTTP Trigger".to_string(),
        description: Some("HTTP request trigger".to_string()),
        enabled: true,
        param_schema: Some(param_schema.clone()),
        out_schema: Some(out_schema.clone()),
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    assert_eq!(trigger.param_schema, Some(param_schema));
    assert_eq!(trigger.out_schema, Some(out_schema));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_trigger_disabled() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("disabled_trigger"));
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "Disabled Trigger".to_string(),
        description: None,
        enabled: false,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    assert!(!trigger.enabled);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_trigger_duplicate_ref() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("duplicate"));

    // Create first trigger
    let input1 = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "First".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    TriggerRepository::create(&pool, input1).await.unwrap();

    // Try to create second trigger with same ref
    let input2 = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "Second".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let result = TriggerRepository::create(&pool, input2).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::AlreadyExists { entity, field, .. } => {
            assert_eq!(entity, "Trigger");
            assert_eq!(field, "ref");
        }
        _ => panic!("Expected AlreadyExists error"),
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_trigger_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("find_pack")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateTriggerInput {
        r#ref: format!("{}.find_trigger", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Find Trigger".to_string(),
        description: Some("Test find".to_string()),
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let created = TriggerRepository::create(&pool, input).await.unwrap();

    let found = TriggerRepository::find_by_id(&pool, created.id)
        .await
        .unwrap()
        .expect("Trigger not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
    assert_eq!(found.label, created.label);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_trigger_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let found = TriggerRepository::find_by_id(&pool, 999999).await.unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_trigger_by_ref() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("ref_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger_ref = format!("{}.ref_trigger", pack.r#ref);
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Ref Trigger".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let created = TriggerRepository::create(&pool, input).await.unwrap();

    let found = TriggerRepository::find_by_ref(&pool, &trigger_ref)
        .await
        .unwrap()
        .expect("Trigger not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, trigger_ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_trigger_by_ref_not_found() {
    let pool = create_test_pool().await.unwrap();

    let found = TriggerRepository::find_by_ref(&pool, "nonexistent.trigger")
        .await
        .unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_triggers() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("list_pack")
        .create(&pool)
        .await
        .unwrap();

    // Create multiple triggers
    let input1 = CreateTriggerInput {
        r#ref: format!("{}.trigger1", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Trigger 1".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let trigger1 = TriggerRepository::create(&pool, input1).await.unwrap();

    let input2 = CreateTriggerInput {
        r#ref: format!("{}.trigger2", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Trigger 2".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let trigger2 = TriggerRepository::create(&pool, input2).await.unwrap();

    let triggers = TriggerRepository::list(&pool).await.unwrap();

    // Should contain at least our created triggers
    assert!(triggers.len() >= 2);

    let trigger_ids: Vec<i64> = triggers.iter().map(|t| t.id).collect();
    assert!(trigger_ids.contains(&trigger1.id));
    assert!(trigger_ids.contains(&trigger2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_triggers_by_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack1 = PackFixture::new_unique("pack1")
        .create(&pool)
        .await
        .unwrap();
    let pack2 = PackFixture::new_unique("pack2")
        .create(&pool)
        .await
        .unwrap();

    // Create triggers for pack1
    let input1a = CreateTriggerInput {
        r#ref: format!("{}.trigger_a", pack1.r#ref),
        pack: Some(pack1.id),
        pack_ref: Some(pack1.r#ref.clone()),
        label: "Pack 1 Trigger A".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let trigger1a = TriggerRepository::create(&pool, input1a).await.unwrap();

    let input1b = CreateTriggerInput {
        r#ref: format!("{}.trigger_b", pack1.r#ref),
        pack: Some(pack1.id),
        pack_ref: Some(pack1.r#ref.clone()),
        label: "Pack 1 Trigger B".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let trigger1b = TriggerRepository::create(&pool, input1b).await.unwrap();

    // Create trigger for pack2
    let input2 = CreateTriggerInput {
        r#ref: format!("{}.trigger", pack2.r#ref),
        pack: Some(pack2.id),
        pack_ref: Some(pack2.r#ref.clone()),
        label: "Pack 2 Trigger".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    TriggerRepository::create(&pool, input2).await.unwrap();

    // Find triggers for pack1
    let pack1_triggers = TriggerRepository::find_by_pack(&pool, pack1.id)
        .await
        .unwrap();

    // Should have exactly 2 triggers for pack1
    assert_eq!(pack1_triggers.len(), 2);

    let trigger_ids: Vec<i64> = pack1_triggers.iter().map(|t| t.id).collect();
    assert!(trigger_ids.contains(&trigger1a.id));
    assert!(trigger_ids.contains(&trigger1b.id));

    // All triggers should belong to pack1
    assert!(pack1_triggers.iter().all(|t| t.pack == Some(pack1.id)));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enabled_triggers() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("enabled_pack")
        .create(&pool)
        .await
        .unwrap();

    // Create enabled trigger
    let input_enabled = CreateTriggerInput {
        r#ref: format!("{}.enabled", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Enabled Trigger".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let trigger_enabled = TriggerRepository::create(&pool, input_enabled)
        .await
        .unwrap();

    // Create disabled trigger
    let input_disabled = CreateTriggerInput {
        r#ref: format!("{}.disabled", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Disabled Trigger".to_string(),
        description: None,
        enabled: false,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    TriggerRepository::create(&pool, input_disabled)
        .await
        .unwrap();

    // Find enabled triggers
    let enabled_triggers = TriggerRepository::find_enabled(&pool).await.unwrap();

    // Should contain at least our enabled trigger
    let enabled_ids: Vec<i64> = enabled_triggers.iter().map(|t| t.id).collect();
    assert!(enabled_ids.contains(&trigger_enabled.id));

    // All returned triggers should be enabled
    assert!(enabled_triggers.iter().all(|t| t.enabled));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_trigger() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("update_pack")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateTriggerInput {
        r#ref: format!("{}.update_trigger", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Original Label".to_string(),
        description: Some("Original description".to_string()),
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();
    let original_updated = trigger.updated;

    // Wait a moment to ensure timestamp changes
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let update_input = UpdateTriggerInput {
        label: Some("Updated Label".to_string()),
        description: Some(Patch::Set("Updated description".to_string())),
        enabled: Some(false),
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        ..Default::default()
    };

    let updated = TriggerRepository::update(&pool, trigger.id, update_input)
        .await
        .unwrap();

    assert_eq!(updated.id, trigger.id);
    assert_eq!(updated.r#ref, trigger.r#ref); // Ref should not change
    assert_eq!(updated.label, "Updated Label");
    assert_eq!(updated.description, Some("Updated description".to_string()));
    assert!(!updated.enabled);
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_trigger_partial() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("partial_trigger"));
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "Original".to_string(),
        description: Some("Original".to_string()),
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    // Update only label
    let update_input = UpdateTriggerInput {
        label: Some("Only Label Changed".to_string()),
        description: None,
        enabled: None,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        ..Default::default()
    };

    let updated = TriggerRepository::update(&pool, trigger.id, update_input)
        .await
        .unwrap();

    assert_eq!(updated.label, "Only Label Changed");
    assert_eq!(updated.description, trigger.description); // Should remain unchanged
    assert_eq!(updated.enabled, trigger.enabled); // Should remain unchanged
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_trigger_schemas() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("schema_update"));
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "Schema Trigger".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    let new_param_schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"}
        }
    });

    let new_out_schema = json!({
        "type": "object",
        "properties": {
            "result": {"type": "boolean"}
        }
    });

    let update_input = UpdateTriggerInput {
        label: None,
        description: None,
        enabled: None,
        param_schema: Some(Patch::Set(new_param_schema.clone())),
        out_schema: Some(Patch::Set(new_out_schema.clone())),
        sensor: None,
        sensor_ref: None,
        ..Default::default()
    };

    let updated = TriggerRepository::update(&pool, trigger.id, update_input)
        .await
        .unwrap();

    assert_eq!(updated.param_schema, Some(new_param_schema));
    assert_eq!(updated.out_schema, Some(new_out_schema));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_trigger_not_found() {
    let pool = create_test_pool().await.unwrap();

    let update_input = UpdateTriggerInput {
        label: Some("New Label".to_string()),
        description: None,
        enabled: None,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        ..Default::default()
    };

    let result = TriggerRepository::update(&pool, 999999, update_input).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    match err {
        Error::NotFound { entity, .. } => {
            assert_eq!(entity, "trigger");
        }
        _ => panic!("Expected NotFound error, got: {:?}", err),
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_trigger() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("delete_trigger"));
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "To Be Deleted".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    // Verify trigger exists
    let found = TriggerRepository::find_by_id(&pool, trigger.id)
        .await
        .unwrap();
    assert!(found.is_some());

    // Delete the trigger
    let deleted = TriggerRepository::delete(&pool, trigger.id).await.unwrap();
    assert!(deleted);

    // Verify trigger no longer exists
    let not_found = TriggerRepository::find_by_id(&pool, trigger.id)
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_trigger_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = TriggerRepository::delete(&pool, 999999).await.unwrap();

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_trigger_timestamps_auto_populated() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("timestamp_trigger"));
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "Timestamp Test".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    // Timestamps should be set
    assert!(trigger.created.timestamp() > 0);
    assert!(trigger.updated.timestamp() > 0);

    // Created and updated should be very close initially
    let diff = (trigger.updated - trigger.created).num_milliseconds().abs();
    assert!(diff < 1000); // Within 1 second
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_trigger_updated_changes_on_update() {
    let pool = create_test_pool().await.unwrap();

    let trigger_ref = format!("core.{}", unique_pack_ref("update_timestamp"));
    let input = CreateTriggerInput {
        r#ref: trigger_ref.clone(),
        pack: None,
        pack_ref: None,
        label: "Original".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();
    let original_created = trigger.created;
    let original_updated = trigger.updated;

    // Wait a moment to ensure timestamp changes
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let update_input = UpdateTriggerInput {
        label: Some("Updated".to_string()),
        description: None,
        enabled: None,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        ..Default::default()
    };

    let updated = TriggerRepository::update(&pool, trigger.id, update_input)
        .await
        .unwrap();

    // Created should remain the same
    assert_eq!(updated.created, original_created);

    // Updated should be newer
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_multiple_triggers_same_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("multi_pack")
        .create(&pool)
        .await
        .unwrap();

    // Create multiple triggers in the same pack
    let input1 = CreateTriggerInput {
        r#ref: format!("{}.webhook", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Webhook".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let trigger1 = TriggerRepository::create(&pool, input1).await.unwrap();

    let input2 = CreateTriggerInput {
        r#ref: format!("{}.timer", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Timer".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };
    let trigger2 = TriggerRepository::create(&pool, input2).await.unwrap();

    // Both should be different triggers
    assert_ne!(trigger1.id, trigger2.id);
    assert_ne!(trigger1.r#ref, trigger2.r#ref);

    // Both should belong to the same pack
    assert_eq!(trigger1.pack, Some(pack.id));
    assert_eq!(trigger2.pack, Some(pack.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_trigger_cascade_delete_with_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("cascade_pack")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateTriggerInput {
        r#ref: format!("{}.cascade_trigger", pack.r#ref),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Cascade Trigger".to_string(),
        description: None,
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    let trigger = TriggerRepository::create(&pool, input).await.unwrap();

    // Delete the pack
    use attune_common::repositories::pack::PackRepository;
    PackRepository::delete(&pool, pack.id).await.unwrap();

    // Verify trigger was cascade deleted
    let not_found = TriggerRepository::find_by_id(&pool, trigger.id)
        .await
        .unwrap();
    assert!(not_found.is_none());
}
