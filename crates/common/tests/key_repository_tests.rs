//! Integration tests for Key repository
//!
//! These tests verify CRUD operations, owner validation, encryption handling,
//! and constraints for the Key repository.

mod helpers;

use attune_common::{
    models::enums::OwnerType,
    repositories::{
        key::{CreateKeyInput, KeyRepository, UpdateKeyInput},
        Create, Delete, FindById, List, Update,
    },
    Error,
};
use helpers::*;

// ============================================================================
// CREATE Tests - System Owner
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_system_owner() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("system_key", "test_value")
        .create(&pool)
        .await
        .unwrap();

    assert!(key.id > 0);
    assert_eq!(key.owner_type, OwnerType::System);
    assert_eq!(key.owner, Some("system".to_string()));
    assert_eq!(key.owner_identity, None);
    assert_eq!(key.owner_pack, None);
    assert_eq!(key.owner_action, None);
    assert_eq!(key.owner_sensor, None);
    assert!(!key.encrypted);
    assert_eq!(key.value, serde_json::json!("test_value"));
    assert!(key.created.timestamp() > 0);
    assert!(key.updated.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_system_encrypted() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("encrypted_key", "encrypted_value")
        .with_encrypted(true)
        .with_encryption_key_hash("sha256:abc123")
        .create(&pool)
        .await
        .unwrap();

    assert!(key.encrypted);
    assert_eq!(key.encryption_key_hash, Some("sha256:abc123".to_string()));
}

// ============================================================================
// CREATE Tests - Identity Owner
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_identity_owner() {
    let pool = create_test_pool().await.unwrap();

    // Create an identity first
    let identity = IdentityFixture::new_unique("testuser")
        .create(&pool)
        .await
        .unwrap();

    let key = KeyFixture::new_identity_unique(identity.id, "api_key", "secret_token")
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(key.owner_type, OwnerType::Identity);
    assert_eq!(key.owner, Some(identity.id.to_string()));
    assert_eq!(key.owner_identity, Some(identity.id));
    assert_eq!(key.owner_pack, None);
    assert_eq!(key.value, serde_json::json!("secret_token"));
}

// ============================================================================
// CREATE Tests - Pack Owner
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_pack_owner() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("testpack")
        .create(&pool)
        .await
        .unwrap();

    let key = KeyFixture::new_pack_unique(pack.id, &pack.r#ref, "config_key", "config_value")
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(key.owner_type, OwnerType::Pack);
    assert_eq!(key.owner, Some(pack.id.to_string()));
    assert_eq!(key.owner_pack, Some(pack.id));
    assert_eq!(key.owner_pack_ref, Some(pack.r#ref.clone()));
    assert_eq!(key.value, serde_json::json!("config_value"));
}

// ============================================================================
// CREATE Tests - Constraints
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_duplicate_ref_fails() {
    let pool = create_test_pool().await.unwrap();

    let key_ref = format!("duplicate_key_{}", unique_test_id());

    // Create first key
    let input = CreateKeyInput {
        r#ref: key_ref.clone(),
        owner_type: OwnerType::System,
        owner: Some("system".to_string()),
        owner_identity: None,
        owner_pack: None,
        owner_pack_ref: None,
        owner_action: None,
        owner_action_ref: None,
        owner_sensor: None,
        owner_sensor_ref: None,
        name: key_ref.clone(),
        encrypted: false,
        encryption_key_hash: None,
        value: serde_json::json!("value1"),
    };

    KeyRepository::create(&pool, input.clone()).await.unwrap();

    // Try to create duplicate
    let result = KeyRepository::create(&pool, input).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_system_with_owner_fields_fails() {
    let pool = create_test_pool().await.unwrap();

    // Create an identity
    let identity = IdentityFixture::new_unique("testuser")
        .create(&pool)
        .await
        .unwrap();

    // Try to create system key with owner_identity set (should fail)
    let input = CreateKeyInput {
        r#ref: format!("invalid_key_{}", unique_test_id()),
        owner_type: OwnerType::System,
        owner: Some("system".to_string()),
        owner_identity: Some(identity.id), // This should cause failure
        owner_pack: None,
        owner_pack_ref: None,
        owner_action: None,
        owner_action_ref: None,
        owner_sensor: None,
        owner_sensor_ref: None,
        name: "invalid".to_string(),
        encrypted: false,
        encryption_key_hash: None,
        value: serde_json::json!("value"),
    };

    let result = KeyRepository::create(&pool, input).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_identity_without_owner_id_fails() {
    let pool = create_test_pool().await.unwrap();

    // Try to create identity key without owner_identity set
    let input = CreateKeyInput {
        r#ref: format!("invalid_key_{}", unique_test_id()),
        owner_type: OwnerType::Identity,
        owner: None,
        owner_identity: None, // Missing required field
        owner_pack: None,
        owner_pack_ref: None,
        owner_action: None,
        owner_action_ref: None,
        owner_sensor: None,
        owner_sensor_ref: None,
        name: "invalid".to_string(),
        encrypted: false,
        encryption_key_hash: None,
        value: serde_json::json!("value"),
    };

    let result = KeyRepository::create(&pool, input).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_multiple_owners_fails() {
    let pool = create_test_pool().await.unwrap();

    let identity = IdentityFixture::new_unique("testuser")
        .create(&pool)
        .await
        .unwrap();

    let pack = PackFixture::new_unique("testpack")
        .create(&pool)
        .await
        .unwrap();

    // Try to create key with both identity and pack owners (should fail)
    let input = CreateKeyInput {
        r#ref: format!("invalid_key_{}", unique_test_id()),
        owner_type: OwnerType::Identity,
        owner: None,
        owner_identity: Some(identity.id),
        owner_pack: Some(pack.id), // Can't have multiple owners
        owner_pack_ref: None,
        owner_action: None,
        owner_action_ref: None,
        owner_sensor: None,
        owner_sensor_ref: None,
        name: "invalid".to_string(),
        encrypted: false,
        encryption_key_hash: None,
        value: serde_json::json!("value"),
    };

    let result = KeyRepository::create(&pool, input).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_key_invalid_ref_format_fails() {
    let pool = create_test_pool().await.unwrap();

    // Try uppercase ref (should fail CHECK constraint)
    let input = CreateKeyInput {
        r#ref: "UPPERCASE_KEY".to_string(),
        owner_type: OwnerType::System,
        owner: Some("system".to_string()),
        owner_identity: None,
        owner_pack: None,
        owner_pack_ref: None,
        owner_action: None,
        owner_action_ref: None,
        owner_sensor: None,
        owner_sensor_ref: None,
        name: "uppercase".to_string(),
        encrypted: false,
        encryption_key_hash: None,
        value: serde_json::json!("value"),
    };

    let result = KeyRepository::create(&pool, input).await;
    assert!(result.is_err());
}

// ============================================================================
// READ Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_id_exists() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("find_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let found = KeyRepository::find_by_id(&pool, key.id).await.unwrap();

    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.id, key.id);
    assert_eq!(found.r#ref, key.r#ref);
    assert_eq!(found.value, key.value);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_id_not_exists() {
    let pool = create_test_pool().await.unwrap();

    let result = KeyRepository::find_by_id(&pool, 99999).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_by_id_exists() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("get_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let found = KeyRepository::get_by_id(&pool, key.id).await.unwrap();

    assert_eq!(found.id, key.id);
    assert_eq!(found.r#ref, key.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_by_id_not_exists_fails() {
    let pool = create_test_pool().await.unwrap();

    let result = KeyRepository::get_by_id(&pool, 99999).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::NotFound { .. }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ref_exists() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("ref_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let found = KeyRepository::find_by_ref(&pool, &key.r#ref).await.unwrap();

    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.id, key.id);
    assert_eq!(found.r#ref, key.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ref_not_exists() {
    let pool = create_test_pool().await.unwrap();

    let result = KeyRepository::find_by_ref(&pool, "nonexistent_key")
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_all_keys() {
    let pool = create_test_pool().await.unwrap();

    // Create multiple keys
    let key1 = KeyFixture::new_system_unique("list_key_a", "value1")
        .create(&pool)
        .await
        .unwrap();

    let key2 = KeyFixture::new_system_unique("list_key_b", "value2")
        .create(&pool)
        .await
        .unwrap();

    let keys = KeyRepository::list(&pool).await.unwrap();

    // Should have at least our 2 keys (may have more from parallel tests)
    assert!(keys.len() >= 2);

    // Verify our keys are in the list
    assert!(keys.iter().any(|k| k.id == key1.id));
    assert!(keys.iter().any(|k| k.id == key2.id));
}

// ============================================================================
// UPDATE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_value() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("update_key", "original_value")
        .create(&pool)
        .await
        .unwrap();

    let original_updated = key.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "key", key.id, original_updated).await;

    let input = UpdateKeyInput {
        value: Some(serde_json::json!("new_value")),
        ..Default::default()
    };

    let updated = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert_eq!(updated.value, serde_json::json!("new_value"));
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_name() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("update_name_key", "value")
        .create(&pool)
        .await
        .unwrap();

    // Use a unique name to avoid conflicts with parallel tests
    let new_name = format!("new_name_{}", unique_test_id());
    let input = UpdateKeyInput {
        name: Some(new_name.clone()),
        ..Default::default()
    };

    let updated = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert_eq!(updated.name, new_name);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_encrypted_status() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("encrypt_key", "plain_value")
        .create(&pool)
        .await
        .unwrap();

    assert!(!key.encrypted);

    let input = UpdateKeyInput {
        encrypted: Some(true),
        encryption_key_hash: Some("sha256:xyz789".to_string()),
        value: Some(serde_json::json!("encrypted_value")),
        ..Default::default()
    };

    let updated = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert!(updated.encrypted);
    assert_eq!(
        updated.encryption_key_hash,
        Some("sha256:xyz789".to_string())
    );
    assert_eq!(updated.value, serde_json::json!("encrypted_value"));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_multiple_fields() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("multi_update_key", "value")
        .create(&pool)
        .await
        .unwrap();

    // Use a unique name to avoid conflicts with parallel tests
    let new_name = format!("updated_name_{}", unique_test_id());
    let input = UpdateKeyInput {
        name: Some(new_name.clone()),
        value: Some(serde_json::json!("updated_value")),
        encrypted: Some(true),
        encryption_key_hash: Some("hash123".to_string()),
    };

    let updated = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert_eq!(updated.name, new_name);
    assert_eq!(updated.value, serde_json::json!("updated_value"));
    assert!(updated.encrypted);
    assert_eq!(updated.encryption_key_hash, Some("hash123".to_string()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_no_changes() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("nochange_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let original_updated = key.updated;

    let input = UpdateKeyInput::default();

    let updated = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert_eq!(updated.id, key.id);
    assert_eq!(updated.name, key.name);
    assert_eq!(updated.value, key.value);
    // Updated timestamp should not change when no fields are updated
    assert_eq!(updated.updated, original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_nonexistent_key_fails() {
    let pool = create_test_pool().await.unwrap();

    let input = UpdateKeyInput {
        value: Some(serde_json::json!("new_value")),
        ..Default::default()
    };

    let result = KeyRepository::update(&pool, 99999, input).await;
    assert!(result.is_err());
}

// ============================================================================
// DELETE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_existing_key() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("delete_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let deleted = KeyRepository::delete(&pool, key.id).await.unwrap();
    assert!(deleted);

    // Verify key is gone
    let result = KeyRepository::find_by_id(&pool, key.id).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_nonexistent_key() {
    let pool = create_test_pool().await.unwrap();

    let deleted = KeyRepository::delete(&pool, 99999).await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_key_when_identity_deleted() {
    let pool = create_test_pool().await.unwrap();

    let identity = IdentityFixture::new_unique("deleteuser")
        .create(&pool)
        .await
        .unwrap();

    let key = KeyFixture::new_identity_unique(identity.id, "user_key", "value")
        .create(&pool)
        .await
        .unwrap();

    // Delete the identity - this will fail because key references it
    use attune_common::repositories::{identity::IdentityRepository, Delete as _};
    let delete_result = IdentityRepository::delete(&pool, identity.id).await;

    // Should fail due to foreign key constraint (no CASCADE on key table)
    assert!(delete_result.is_err());

    // Key should still exist
    let result = KeyRepository::find_by_id(&pool, key.id).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_key_when_pack_deleted() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("deletepack")
        .create(&pool)
        .await
        .unwrap();

    let key = KeyFixture::new_pack_unique(pack.id, &pack.r#ref, "pack_key", "value")
        .create(&pool)
        .await
        .unwrap();

    // Delete the pack - this will fail because key references it
    use attune_common::repositories::{pack::PackRepository, Delete as _};
    let delete_result = PackRepository::delete(&pool, pack.id).await;

    // Should fail due to foreign key constraint (no CASCADE on key table)
    assert!(delete_result.is_err());

    // Key should still exist
    let result = KeyRepository::find_by_id(&pool, key.id).await.unwrap();
    assert!(result.is_some());
}

// ============================================================================
// Specialized Query Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_owner_type_system() {
    let pool = create_test_pool().await.unwrap();

    let _key1 = KeyFixture::new_system_unique("sys_key1", "value1")
        .create(&pool)
        .await
        .unwrap();

    let _key2 = KeyFixture::new_system_unique("sys_key2", "value2")
        .create(&pool)
        .await
        .unwrap();

    let keys = KeyRepository::find_by_owner_type(&pool, OwnerType::System)
        .await
        .unwrap();

    // Should have at least our 2 system keys
    assert!(keys.len() >= 2);
    assert!(keys.iter().all(|k| k.owner_type == OwnerType::System));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_owner_type_identity() {
    let pool = create_test_pool().await.unwrap();

    let identity1 = IdentityFixture::new_unique("user1")
        .create(&pool)
        .await
        .unwrap();

    let identity2 = IdentityFixture::new_unique("user2")
        .create(&pool)
        .await
        .unwrap();

    let key1 = KeyFixture::new_identity_unique(identity1.id, "key1", "value1")
        .create(&pool)
        .await
        .unwrap();

    let key2 = KeyFixture::new_identity_unique(identity2.id, "key2", "value2")
        .create(&pool)
        .await
        .unwrap();

    let keys = KeyRepository::find_by_owner_type(&pool, OwnerType::Identity)
        .await
        .unwrap();

    // Should contain our identity keys
    assert!(keys.iter().any(|k| k.id == key1.id));
    assert!(keys.iter().any(|k| k.id == key2.id));
    assert!(keys.iter().all(|k| k.owner_type == OwnerType::Identity));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_owner_type_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("ownerpack")
        .create(&pool)
        .await
        .unwrap();

    let key1 = KeyFixture::new_pack_unique(pack.id, &pack.r#ref, "pack_key1", "value1")
        .create(&pool)
        .await
        .unwrap();

    let key2 = KeyFixture::new_pack_unique(pack.id, &pack.r#ref, "pack_key2", "value2")
        .create(&pool)
        .await
        .unwrap();

    let keys = KeyRepository::find_by_owner_type(&pool, OwnerType::Pack)
        .await
        .unwrap();

    // Should contain our pack keys
    assert!(keys.iter().any(|k| k.id == key1.id));
    assert!(keys.iter().any(|k| k.id == key2.id));
    assert!(keys.iter().all(|k| k.owner_type == OwnerType::Pack));
}

// ============================================================================
// Timestamp Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_created_timestamp_set_automatically() {
    let pool = create_test_pool().await.unwrap();

    let before = chrono::Utc::now();

    let key = KeyFixture::new_system_unique("timestamp_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let after = chrono::Utc::now();

    assert!(key.created >= before);
    assert!(key.created <= after);
    assert_eq!(key.created, key.updated); // Should be equal on creation
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_updated_timestamp_changes_on_update() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("update_time_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let original_updated = key.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "key", key.id, original_updated).await;

    let input = UpdateKeyInput {
        value: Some(serde_json::json!("new_value")),
        ..Default::default()
    };

    let updated = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert!(updated.updated > original_updated);
    assert_eq!(updated.created, key.created); // Created should not change
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_updated_timestamp_unchanged_on_read() {
    let pool = create_test_pool().await.unwrap();

    let key = KeyFixture::new_system_unique("read_time_key", "value")
        .create(&pool)
        .await
        .unwrap();

    let original_updated = key.updated;

    // Read the key
    let found = KeyRepository::find_by_id(&pool, key.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.updated, original_updated); // Should not change
}

// ============================================================================
// Encryption Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_key_encrypted_flag() {
    let pool = create_test_pool().await.unwrap();

    let plain_key = KeyFixture::new_system_unique("plain_key", "plain_value")
        .create(&pool)
        .await
        .unwrap();

    let encrypted_key = KeyFixture::new_system_unique("encrypted_key", "cipher_text")
        .with_encrypted(true)
        .with_encryption_key_hash("sha256:abc")
        .create(&pool)
        .await
        .unwrap();

    assert!(!plain_key.encrypted);
    assert_eq!(plain_key.encryption_key_hash, None);

    assert!(encrypted_key.encrypted);
    assert_eq!(
        encrypted_key.encryption_key_hash,
        Some("sha256:abc".to_string())
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_encryption_status() {
    let pool = create_test_pool().await.unwrap();

    // Create plain key
    let key = KeyFixture::new_system_unique("to_encrypt", "plain_value")
        .create(&pool)
        .await
        .unwrap();

    assert!(!key.encrypted);

    // Encrypt it
    let input = UpdateKeyInput {
        encrypted: Some(true),
        encryption_key_hash: Some("sha256:newkey".to_string()),
        value: Some(serde_json::json!("encrypted_value")),
        ..Default::default()
    };

    let encrypted = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert!(encrypted.encrypted);
    assert_eq!(
        encrypted.encryption_key_hash,
        Some("sha256:newkey".to_string())
    );
    assert_eq!(encrypted.value, serde_json::json!("encrypted_value"));

    // Decrypt it
    let input = UpdateKeyInput {
        encrypted: Some(false),
        encryption_key_hash: None,
        value: Some(serde_json::json!("plain_value")),
        ..Default::default()
    };

    let decrypted = KeyRepository::update(&pool, key.id, input).await.unwrap();

    assert!(!decrypted.encrypted);
    assert_eq!(decrypted.value, serde_json::json!("plain_value"));
}

// ============================================================================
// Owner Validation Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_multiple_keys_same_pack_different_names() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("multikey_pack")
        .create(&pool)
        .await
        .unwrap();

    let key1 = KeyFixture::new_pack_unique(pack.id, &pack.r#ref, "key1", "value1")
        .create(&pool)
        .await
        .unwrap();

    let key2 = KeyFixture::new_pack_unique(pack.id, &pack.r#ref, "key2", "value2")
        .create(&pool)
        .await
        .unwrap();

    assert_ne!(key1.id, key2.id);
    assert_eq!(key1.owner_pack, Some(pack.id));
    assert_eq!(key2.owner_pack, Some(pack.id));
    assert_ne!(key1.name, key2.name);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_same_key_name_different_owners() {
    let pool = create_test_pool().await.unwrap();

    let pack1 = PackFixture::new_unique("pack1")
        .create(&pool)
        .await
        .unwrap();

    let pack2 = PackFixture::new_unique("pack2")
        .create(&pool)
        .await
        .unwrap();

    // Same base key name, different owners - should be allowed
    // Use same base name so fixture creates keys with same logical name
    let base_name = format!("api_key_{}", unique_test_id());

    let key1 = KeyFixture::new_pack(pack1.id, &pack1.r#ref, &base_name, "value1")
        .create(&pool)
        .await
        .unwrap();

    let key2 = KeyFixture::new_pack(pack2.id, &pack2.r#ref, &base_name, "value2")
        .create(&pool)
        .await
        .unwrap();

    assert_ne!(key1.id, key2.id);
    assert_eq!(key1.name, key2.name); // Same name
    assert_ne!(key1.owner_pack, key2.owner_pack); // Different owners
}
