//! Integration tests for Pack repository
//!
//! These tests verify all CRUD operations, transactions, error handling,
//! and constraint validation for the Pack repository.

mod helpers;

use attune_common::repositories::pack::{self, PackRepository};
use attune_common::repositories::{
    Create, Delete, FindById, FindByRef, List, Pagination, Patch, Update,
};
use attune_common::Error;
use helpers::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .with_label("Test Pack")
        .with_version("1.0.0")
        .with_description("A test pack")
        .create(&pool)
        .await
        .unwrap();

    assert!(pack.r#ref.starts_with("test_pack_"));
    assert_eq!(pack.version, "1.0.0");
    assert_eq!(pack.label, "Test Pack");
    assert_eq!(pack.description, Some("A test pack".to_string()));
    assert!(pack.created.timestamp() > 0);
    assert!(pack.updated.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_pack_duplicate_ref() {
    let pool = create_test_pool().await.unwrap();

    // Create first pack - use a specific unique ref for this test
    let unique_ref = helpers::unique_pack_ref("duplicate_test");
    PackFixture::new(&unique_ref).create(&pool).await.unwrap();

    // Try to create pack with same ref (should fail due to unique constraint)
    let result = PackFixture::new(&unique_ref).create(&pool).await;

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(matches!(error, Error::AlreadyExists { .. }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_pack_with_tags() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("tagged_pack")
        .with_tags(vec!["test".to_string(), "automation".to_string()])
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(pack.tags.len(), 2);
    assert!(pack.tags.contains(&"test".to_string()));
    assert!(pack.tags.contains(&"automation".to_string()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_pack_standard() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("standard_pack")
        .with_standard(true)
        .create(&pool)
        .await
        .unwrap();

    assert!(pack.is_standard);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_pack_by_id() {
    let pool = create_test_pool().await.unwrap();

    let created = PackFixture::new_unique("find_pack")
        .create(&pool)
        .await
        .unwrap();

    let found = PackRepository::find_by_id(&pool, created.id)
        .await
        .unwrap()
        .expect("Pack not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
    assert_eq!(found.label, created.label);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_pack_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = PackRepository::find_by_id(&pool, 999999).await.unwrap();

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_pack_by_ref() {
    let pool = create_test_pool().await.unwrap();

    let created = PackFixture::new_unique("ref_pack")
        .create(&pool)
        .await
        .unwrap();

    let found = PackRepository::find_by_ref(&pool, &created.r#ref)
        .await
        .unwrap()
        .expect("Pack not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_pack_by_ref_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = PackRepository::find_by_ref(&pool, "nonexistent.pack")
        .await
        .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_mutation_advisory_lock_serializes_only_same_ref() {
    let pool = create_test_pool().await.unwrap();
    let locked_ref = unique_pack_ref("mutation_lock");
    let different_ref = unique_pack_ref("mutation_lock_other");

    let mut first = pool.begin().await.unwrap();
    PackRepository::acquire_mutation_lock(&mut first, &locked_ref)
        .await
        .unwrap();

    let same_pool = pool.clone();
    let same_ref = locked_ref.clone();
    let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        let mut transaction = same_pool.begin().await.unwrap();
        PackRepository::acquire_mutation_lock(&mut transaction, &same_ref)
            .await
            .unwrap();
        acquired_tx.send(()).unwrap();
        transaction.commit().await.unwrap();
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut acquired_rx)
            .await
            .is_err()
    );

    let mut different = pool.begin().await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        PackRepository::acquire_mutation_lock(&mut different, &different_ref),
    )
    .await
    .expect("different pack ref should not share the advisory key")
    .unwrap();
    different.rollback().await.unwrap();

    first.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), acquired_rx)
        .await
        .expect("same-ref waiter should acquire after transaction end")
        .unwrap();
    waiter.await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_packs() {
    let pool = create_test_pool().await.unwrap();

    // Create multiple packs
    let pack1 = PackFixture::new_unique("pack1")
        .create(&pool)
        .await
        .unwrap();
    let pack2 = PackFixture::new_unique("pack2")
        .create(&pool)
        .await
        .unwrap();
    let pack3 = PackFixture::new_unique("pack3")
        .create(&pool)
        .await
        .unwrap();

    let packs = PackRepository::list(&pool).await.unwrap();

    // Should contain at least our created packs
    assert!(packs.len() >= 3);

    // Verify our packs are in the list
    let pack_refs: Vec<String> = packs.iter().map(|p| p.r#ref.clone()).collect();
    assert!(pack_refs.contains(&pack1.r#ref));
    assert!(pack_refs.contains(&pack2.r#ref));
    assert!(pack_refs.contains(&pack3.r#ref));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_packs_with_pagination() {
    let pool = create_test_pool().await.unwrap();

    // Create test packs
    for i in 1..=5 {
        PackFixture::new_unique(&format!("pack{}", i))
            .create(&pool)
            .await
            .unwrap();
    }

    // Test that pagination works by getting pages
    let page1 = PackRepository::list_paginated(&pool, Pagination::new(2, 0))
        .await
        .unwrap();
    // First page should have 2 items (or less if there are fewer total)
    assert!(page1.len() <= 2);

    // Test with different offset
    let page2 = PackRepository::list_paginated(&pool, Pagination::new(2, 2))
        .await
        .unwrap();
    // Second page should have items (or be empty if not enough total)
    assert!(page2.len() <= 2);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("update_pack")
        .with_label("Original Label")
        .with_version("1.0.0")
        .create(&pool)
        .await
        .unwrap();

    let update_input = pack::UpdatePackInput {
        label: Some("Updated Label".to_string()),
        version: Some("2.0.0".to_string()),
        description: Some(Patch::Set("Updated description".to_string())),
        ..Default::default()
    };

    let updated = PackRepository::update(&pool, pack.id, update_input)
        .await
        .unwrap();

    assert_eq!(updated.id, pack.id);
    assert_eq!(updated.label, "Updated Label");
    assert_eq!(updated.version, "2.0.0");
    assert_eq!(updated.description, Some("Updated description".to_string()));
    assert_eq!(updated.r#ref, pack.r#ref); // ref should not change
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_pack_partial() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("partial_pack")
        .with_label("Original Label")
        .with_version("1.0.0")
        .with_description("Original description")
        .create(&pool)
        .await
        .unwrap();

    // Update only the label
    let update_input = pack::UpdatePackInput {
        label: Some("New Label".to_string()),
        ..Default::default()
    };

    let updated = PackRepository::update(&pool, pack.id, update_input)
        .await
        .unwrap();

    assert_eq!(updated.label, "New Label");
    assert_eq!(updated.version, "1.0.0"); // version unchanged
    assert_eq!(updated.description, pack.description); // description unchanged
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_pack_not_found() {
    let pool = create_test_pool().await.unwrap();

    let update_input = pack::UpdatePackInput {
        label: Some("Updated".to_string()),
        ..Default::default()
    };

    let result = PackRepository::update(&pool, 999999, update_input).await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::NotFound { .. }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_pack_tags() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("tags_pack")
        .with_tags(vec!["old".to_string()])
        .create(&pool)
        .await
        .unwrap();

    let update_input = pack::UpdatePackInput {
        tags: Some(vec!["new".to_string(), "updated".to_string()]),
        ..Default::default()
    };

    let updated = PackRepository::update(&pool, pack.id, update_input)
        .await
        .unwrap();

    assert_eq!(updated.tags.len(), 2);
    assert!(updated.tags.contains(&"new".to_string()));
    assert!(updated.tags.contains(&"updated".to_string()));
    assert!(!updated.tags.contains(&"old".to_string()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("delete_pack")
        .create(&pool)
        .await
        .unwrap();

    // Verify pack exists
    let found = PackRepository::find_by_id(&pool, pack.id).await.unwrap();
    assert!(found.is_some());

    // Delete the pack
    PackRepository::delete(&pool, pack.id).await.unwrap();

    // Verify pack is gone
    let not_found = PackRepository::find_by_id(&pool, pack.id).await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_pack_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = PackRepository::delete(&pool, 999999).await.unwrap();

    assert!(!deleted, "Should return false when pack doesn't exist");
}

// TODO: Re-enable once ActionFixture is fixed
// #[tokio::test]
// async fn test_delete_pack_cascades_to_actions() {
//     let pool = create_test_pool().await.unwrap();
//
//     // Create pack with an action
//     let pack = PackFixture::new_unique("cascade_pack")
//         .create(&pool)
//         .await
//         .unwrap();
//
//     let action = ActionFixture::new(pack.id, "cascade_action")
//         .create(&pool)
//         .await
//         .unwrap();
//
//     // Verify action exists
//     let found_action = ActionRepository::find_by_id(&pool, action.id)
//         .await
//         .unwrap();
//     assert!(found_action.is_some());
//
//     // Delete pack
//     PackRepository::delete(&pool, pack.id).await.unwrap();
//
//     // Verify action is also deleted (cascade)
//     let action_after = ActionRepository::find_by_id(&pool, action.id)
//         .await
//         .unwrap();
//     assert!(action_after.is_none());
// }

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_count_packs() {
    let pool = create_test_pool().await.unwrap();

    // Get initial count
    let count_before = PackRepository::count(&pool).await.unwrap();

    // Create some packs
    PackFixture::new_unique("pack1")
        .create(&pool)
        .await
        .unwrap();
    PackFixture::new_unique("pack2")
        .create(&pool)
        .await
        .unwrap();
    PackFixture::new_unique("pack3")
        .create(&pool)
        .await
        .unwrap();

    let count_after = PackRepository::count(&pool).await.unwrap();
    // Should have at least 3 more packs (may have more from parallel tests)
    assert!(count_after >= count_before + 3);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_transaction_commit() {
    let pool = create_test_pool().await.unwrap();

    // Begin transaction
    let mut tx = pool.begin().await.unwrap();

    // Create pack in transaction with unique ref
    let unique_ref = helpers::unique_pack_ref("tx_pack");
    let input = pack::CreatePackInput {
        r#ref: unique_ref.clone(),
        label: "Transaction Pack".to_string(),
        description: None,
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({}),
        tags: vec![],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    let pack = PackRepository::create(&mut *tx, input).await.unwrap();

    // Commit transaction
    tx.commit().await.unwrap();

    // Verify pack exists after commit
    let found = PackRepository::find_by_id(&pool, pack.id)
        .await
        .unwrap()
        .expect("Pack should exist after commit");

    assert_eq!(found.r#ref, unique_ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_transaction_rollback() {
    let pool = create_test_pool().await.unwrap();

    // Begin transaction
    let mut tx = pool.begin().await.unwrap();

    // Create pack in transaction with unique ref
    let input = pack::CreatePackInput {
        r#ref: helpers::unique_pack_ref("rollback_pack"),
        label: "Rollback Pack".to_string(),
        description: None,
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({}),
        tags: vec![],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    let pack = PackRepository::create(&mut *tx, input).await.unwrap();
    let pack_id = pack.id;

    // Rollback transaction
    tx.rollback().await.unwrap();

    // Verify pack does NOT exist after rollback
    let not_found = PackRepository::find_by_id(&pool, pack_id).await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_invalid_ref_format() {
    let pool = create_test_pool().await.unwrap();

    let input = pack::CreatePackInput {
        r#ref: "invalid pack!@#".to_string(), // Contains invalid characters
        label: "Invalid Pack".to_string(),
        description: None,
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({}),
        tags: vec![],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    let result = PackRepository::create(&pool, input).await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::Validation { .. }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_dotted_ref_is_rejected_at_persistence_boundary() {
    let pool = create_test_pool().await.unwrap();
    let input = pack::CreatePackInput {
        r#ref: "org.pack".to_string(),
        label: "Dotted Pack".to_string(),
        description: None,
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({}),
        tags: vec![],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    let result = PackRepository::create(&pool, input).await;

    assert!(matches!(result.unwrap_err(), Error::Validation { .. }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_valid_ref_formats() {
    let pool = create_test_pool().await.unwrap();

    // Valid ref formats - each gets unique suffix
    let valid_base_refs = vec![
        "simple",
        "with_underscores",
        "with-hyphens",
        "mixed_all-together-123",
    ];

    for base_ref in valid_base_refs {
        let unique_ref = helpers::unique_pack_ref(base_ref);
        let input = pack::CreatePackInput {
            r#ref: unique_ref.clone(),
            label: format!("Pack {}", base_ref),
            description: None,
            version: "1.0.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        };

        let result = PackRepository::create(&pool, input).await;
        assert!(result.is_ok(), "Ref '{}' should be valid", unique_ref);
    }
}
