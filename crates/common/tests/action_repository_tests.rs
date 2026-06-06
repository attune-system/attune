//! Integration tests for Action repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Action repository.

mod helpers;

use attune_common::repositories::{
    action::{ActionRepository, CreateActionInput, UpdateActionInput},
    Create, Delete, FindById, FindByRef, List, Update,
};
use helpers::*;
use serde_json::json;

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_action() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(action.pack, pack.id);
    assert_eq!(action.pack_ref, pack.r#ref);
    assert!(action.r#ref.contains("test_pack_"));
    assert!(action.r#ref.contains(".test_action_"));
    assert!(action.created.timestamp() > 0);
    assert!(action.updated.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_action_with_optional_fields() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "full_action")
        .with_label("Full Test Action")
        .with_description("Action with all optional fields")
        .with_entrypoint("custom.py")
        .with_param_schema(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            }
        }))
        .with_out_schema(json!({
            "type": "object",
            "properties": {
                "result": {"type": "string"}
            }
        }))
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(action.label, "Full Test Action");
    assert_eq!(
        action.description,
        Some("Action with all optional fields".to_string())
    );
    assert_eq!(action.entrypoint, "custom.py");
    assert!(action.param_schema.is_some());
    assert!(action.out_schema.is_some());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_action_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let created = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    let found = ActionRepository::find_by_id(&pool, created.id)
        .await
        .unwrap();

    assert!(found.is_some());
    let action = found.unwrap();
    assert_eq!(action.id, created.id);
    assert_eq!(action.r#ref, created.r#ref);
    assert_eq!(action.pack, pack.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_action_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let found = ActionRepository::find_by_id(&pool, 99999).await.unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_action_by_ref() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let created = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    let found = ActionRepository::find_by_ref(&pool, &created.r#ref)
        .await
        .unwrap();

    assert!(found.is_some());
    let action = found.unwrap();
    assert_eq!(action.id, created.id);
    assert_eq!(action.r#ref, created.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_action_by_ref_not_found() {
    let pool = create_test_pool().await.unwrap();

    let found = ActionRepository::find_by_ref(&pool, "nonexistent.action")
        .await
        .unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_actions() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    // Create multiple actions
    ActionFixture::new_unique(pack.id, &pack.r#ref, "action1")
        .create(&pool)
        .await
        .unwrap();
    ActionFixture::new_unique(pack.id, &pack.r#ref, "action2")
        .create(&pool)
        .await
        .unwrap();
    ActionFixture::new_unique(pack.id, &pack.r#ref, "action3")
        .create(&pool)
        .await
        .unwrap();

    let actions = ActionRepository::list(&pool).await.unwrap();

    // Should contain at least our created actions
    assert!(actions.len() >= 3);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_actions_empty() {
    let pool = create_test_pool().await.unwrap();

    let actions = ActionRepository::list(&pool).await.unwrap();
    // May have actions from other tests, just verify we can list without error
    drop(actions);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_action() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    let original_updated = action.updated;

    // Wait a bit to ensure timestamp difference
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let update = UpdateActionInput {
        label: Some("Updated Label".to_string()),
        description: Some(attune_common::repositories::Patch::Set(
            "Updated description".to_string(),
        )),
        ..Default::default()
    };

    let updated = ActionRepository::update(&pool, action.id, update)
        .await
        .unwrap();

    assert_eq!(updated.id, action.id);
    assert_eq!(updated.label, "Updated Label");
    assert_eq!(updated.description, Some("Updated description".to_string()));
    assert_eq!(updated.entrypoint, action.entrypoint); // Unchanged
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_action_not_found() {
    let pool = create_test_pool().await.unwrap();

    let update = UpdateActionInput {
        label: Some("New Label".to_string()),
        ..Default::default()
    };

    let result = ActionRepository::update(&pool, 99999, update).await;

    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_action_partial() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .with_label("Original")
        .with_description("Original description")
        .create(&pool)
        .await
        .unwrap();

    // Update only the label
    let update = UpdateActionInput {
        label: Some("Updated Label Only".to_string()),
        ..Default::default()
    };

    let updated = ActionRepository::update(&pool, action.id, update)
        .await
        .unwrap();

    assert_eq!(updated.label, "Updated Label Only");
    assert_eq!(updated.description, action.description); // Unchanged
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_action() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    let deleted = ActionRepository::delete(&pool, action.id).await.unwrap();

    assert!(deleted);

    // Verify it's gone
    let found = ActionRepository::find_by_id(&pool, action.id)
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_action_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = ActionRepository::delete(&pool, 99999).await.unwrap();

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_actions_cascade_delete_with_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    // Delete the pack
    sqlx::query("DELETE FROM pack WHERE id = $1")
        .bind(pack.id)
        .execute(&pool)
        .await
        .unwrap();

    // Action should be cascade deleted
    let found = ActionRepository::find_by_id(&pool, action.id)
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_foreign_key_constraint() {
    let pool = create_test_pool().await.unwrap();

    // Try to create action with non-existent pack
    let input = CreateActionInput {
        r#ref: "test.action".to_string(),
        pack: 99999,
        pack_ref: "nonexistent.pack".to_string(),
        label: "Test Action".to_string(),
        description: Some("Test".to_string()),
        entrypoint: "main.py".to_string(),
        runtime: None,
        enabled: true,
        runtime_version_constraint: None,
        required_worker_runtimes: serde_json::json!({}),
        worker_selector: serde_json::json!({}),
        worker_tolerations: serde_json::json!([]),
        worker_affinity: serde_json::json!({}),
        param_schema: None,
        out_schema: None,
        is_adhoc: false,
        accesses_mcp: false,
        default_execution_permission_set_refs: Vec::new(),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
        timeout_seconds: None,
    };

    let result = ActionRepository::create(&pool, input).await;

    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_multiple_actions_same_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    // Create multiple actions in the same pack
    let action1 = ActionFixture::new_unique(pack.id, &pack.r#ref, "action1")
        .create(&pool)
        .await
        .unwrap();
    let action2 = ActionFixture::new_unique(pack.id, &pack.r#ref, "action2")
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(action1.pack, pack.id);
    assert_eq!(action2.pack, pack.id);
    assert_ne!(action1.id, action2.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_unique_ref_constraint() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    // Create first action - use non-unique name since we're testing duplicate detection
    let action_name = helpers::unique_action_name("duplicate");
    ActionFixture::new(pack.id, &pack.r#ref, &action_name)
        .create(&pool)
        .await
        .unwrap();

    // Try to create another action with same ref (should fail)
    let result = ActionFixture::new(pack.id, &pack.r#ref, &action_name)
        .create(&pool)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_with_json_schemas() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    let param_schema = json!({
        "type": "object",
        "properties": {
            "input": {"type": "string"},
            "count": {"type": "integer"}
        },
        "required": ["input"]
    });

    let out_schema = json!({
        "type": "object",
        "properties": {
            "output": {"type": "string"},
            "status": {"type": "string"}
        }
    });

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "schema_action")
        .with_param_schema(param_schema.clone())
        .with_out_schema(out_schema.clone())
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(action.param_schema, Some(param_schema));
    assert_eq!(action.out_schema, Some(out_schema));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_timestamps_auto_populated() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    let now = chrono::Utc::now();
    assert!(action.created <= now);
    assert!(action.updated <= now);
    assert!(action.created <= action.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_updated_changes_on_update() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    let original_created = action.created;
    let original_updated = action.updated;

    // Wait a bit
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let update = UpdateActionInput {
        label: Some("Updated".to_string()),
        ..Default::default()
    };

    let updated = ActionRepository::update(&pool, action.id, update)
        .await
        .unwrap();

    assert_eq!(updated.created, original_created); // Created unchanged
    assert!(updated.updated > original_updated); // Updated changed
}
