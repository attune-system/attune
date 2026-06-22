//! Integration tests for Artifact repository
//!
//! Tests cover CRUD operations, specialized queries, constraints,
//! enum handling, timestamps, and edge cases.

use attune_common::models::enums::{
    ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
};
use attune_common::repositories::artifact::{
    ArtifactRepository, ArtifactSearchFilters, CreateArtifactInput, UpdateArtifactInput,
};
use attune_common::repositories::{Create, Delete, FindById, FindByRef, List, Patch, Update};
use attune_common::Error;
use sqlx::PgPool;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

mod helpers;
use helpers::create_test_pool;

// Global counter for unique IDs across all tests
static GLOBAL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test fixture for creating unique artifact data
struct ArtifactFixture {
    sequence: AtomicU64,
    test_id: String,
}

impl ArtifactFixture {
    fn new(test_name: &str) -> Self {
        let global_count = GLOBAL_COUNTER.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        // Create unique test ID from test name, timestamp, and global counter
        let mut hasher = DefaultHasher::new();
        test_name.hash(&mut hasher);
        timestamp.hash(&mut hasher);
        global_count.hash(&mut hasher);
        let hash = hasher.finish();

        let test_id = format!("test_{}_{:x}", global_count, hash);

        Self {
            sequence: AtomicU64::new(0),
            test_id,
        }
    }

    fn unique_ref(&self, prefix: &str) -> String {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        format!("{}_{}_ref_{}", prefix, self.test_id, seq)
    }

    fn unique_owner(&self, prefix: &str) -> String {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        format!("{}_{}_owner_{}", prefix, self.test_id, seq)
    }

    fn create_input(&self, ref_suffix: &str) -> CreateArtifactInput {
        CreateArtifactInput {
            r#ref: self.unique_ref(ref_suffix),
            scope: OwnerType::System,
            owner: self.unique_owner("system"),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::default(),
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: None,
            description: None,
            content_type: None,
            data: None,
        }
    }
}

async fn setup_db() -> PgPool {
    create_test_pool()
        .await
        .expect("Failed to create test pool")
}

// ============================================================================
// Basic CRUD Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_artifact() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("create_artifact");
    let input = fixture.create_input("basic");

    let artifact = ArtifactRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create artifact");

    assert!(artifact.id > 0);
    assert_eq!(artifact.r#ref, input.r#ref);
    assert_eq!(artifact.scope, input.scope);
    assert_eq!(artifact.owner, input.owner);
    assert_eq!(artifact.r#type, input.r#type);
    assert_eq!(artifact.retention_policy, input.retention_policy);
    assert_eq!(artifact.retention_limit, input.retention_limit);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_id_exists() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_id_exists");
    let input = fixture.create_input("find");

    let created = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact");

    let found = ArtifactRepository::find_by_id(&pool, created.id)
        .await
        .expect("Failed to query artifact")
        .expect("Artifact not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
    assert_eq!(found.scope, created.scope);
    assert_eq!(found.owner, created.owner);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_id_not_exists() {
    let pool = setup_db().await;
    let non_existent_id = 999_999_999_999i64;

    let found = ArtifactRepository::find_by_id(&pool, non_existent_id)
        .await
        .expect("Failed to query artifact");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_by_id_not_found_error() {
    let pool = setup_db().await;
    let non_existent_id = 999_999_999_998i64;

    let result = ArtifactRepository::get_by_id(&pool, non_existent_id).await;

    assert!(result.is_err());
    match result {
        Err(Error::NotFound { entity, .. }) => {
            assert_eq!(entity, "artifact");
        }
        _ => panic!("Expected NotFound error"),
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ref_exists() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_ref_exists");
    let input = fixture.create_input("ref_test");

    let created = ArtifactRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create artifact");

    let found = ArtifactRepository::find_by_ref(&pool, &input.r#ref)
        .await
        .expect("Failed to query artifact")
        .expect("Artifact not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ref_not_exists() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_ref_not_exists");

    let found = ArtifactRepository::find_by_ref(&pool, &fixture.unique_ref("nonexistent"))
        .await
        .expect("Failed to query artifact");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_artifacts() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("list");

    // Create multiple artifacts
    for i in 0..3 {
        let input = fixture.create_input(&format!("list_{}", i));
        ArtifactRepository::create(&pool, input)
            .await
            .expect("Failed to create artifact");
    }

    let artifacts = ArtifactRepository::list(&pool)
        .await
        .expect("Failed to list artifacts");

    // Should have at least the 3 we created
    assert!(artifacts.len() >= 3);

    // Should be ordered by created DESC (newest first)
    for i in 0..artifacts.len().saturating_sub(1) {
        assert!(artifacts[i].created >= artifacts[i + 1].created);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_search_artifacts_by_classification() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("search_by_classification");

    let mut general_input = fixture.create_input("general");
    general_input.classification = ArtifactClassification::General;
    let general = ArtifactRepository::create(&pool, general_input)
        .await
        .expect("Failed to create general artifact");

    let mut runtime_log_input = fixture.create_input("runtime_log");
    runtime_log_input.r#ref = format!("{}.stdout.log", fixture.unique_ref("runtime"));
    runtime_log_input.classification = ArtifactClassification::RuntimeLog;
    let runtime_log = ArtifactRepository::create(&pool, runtime_log_input)
        .await
        .expect("Failed to create runtime log artifact");

    let result = ArtifactRepository::search(
        &pool,
        &ArtifactSearchFilters {
            classification: Some(ArtifactClassification::RuntimeLog),
            limit: 50,
            offset: 0,
            ..Default::default()
        },
    )
    .await
    .expect("Failed to search artifacts by classification");

    assert!(result
        .rows
        .iter()
        .any(|artifact| artifact.id == runtime_log.id));
    assert!(!result.rows.iter().any(|artifact| artifact.id == general.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_artifact_ref() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("update_ref");
    let input = fixture.create_input("original");

    let created = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact");

    let new_ref = fixture.unique_ref("updated");
    let update_input = UpdateArtifactInput {
        r#ref: Some(new_ref.clone()),
        ..Default::default()
    };

    let updated = ArtifactRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update artifact");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.r#ref, new_ref);
    assert_eq!(updated.scope, created.scope);
    assert!(updated.updated > created.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_artifact_all_fields() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("update_all");
    let input = fixture.create_input("original");

    let created = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact");

    let update_input = UpdateArtifactInput {
        r#ref: Some(fixture.unique_ref("all_updated")),
        scope: Some(OwnerType::Identity),
        owner: Some(fixture.unique_owner("identity")),
        r#type: Some(ArtifactType::FileImage),
        visibility: Some(ArtifactVisibility::Public),
        classification: Some(ArtifactClassification::General),
        retention_policy: Some(RetentionPolicyType::Days),
        retention_limit: Some(30),
        name: Some(Patch::Set("Updated Name".to_string())),
        description: Some(Patch::Set("Updated description".to_string())),
        content_type: Some(Patch::Set("image/png".to_string())),
        size_bytes: Some(12345),
        data: Some(Patch::Set(serde_json::json!({"key": "value"}))),
    };

    let updated = ArtifactRepository::update(&pool, created.id, update_input.clone())
        .await
        .expect("Failed to update artifact");

    assert_eq!(updated.r#ref, update_input.r#ref.unwrap());
    assert_eq!(updated.scope, update_input.scope.unwrap());
    assert_eq!(updated.owner, update_input.owner.unwrap());
    assert_eq!(updated.r#type, update_input.r#type.unwrap());
    assert_eq!(
        updated.retention_policy,
        update_input.retention_policy.unwrap()
    );
    assert_eq!(
        updated.retention_limit,
        update_input.retention_limit.unwrap()
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_artifact_no_changes() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("update_no_changes");
    let input = fixture.create_input("nochange");

    let created = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact");

    let update_input = UpdateArtifactInput::default();

    let updated = ArtifactRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update artifact");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.r#ref, created.r#ref);
    assert_eq!(updated.updated, created.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_artifact() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("delete");
    let input = fixture.create_input("delete");

    let created = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact");

    let deleted = ArtifactRepository::delete(&pool, created.id)
        .await
        .expect("Failed to delete artifact");

    assert!(deleted);

    let found = ArtifactRepository::find_by_id(&pool, created.id)
        .await
        .expect("Failed to query artifact");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_artifact_not_exists() {
    let pool = setup_db().await;
    let non_existent_id = 999_999_999_997i64;

    let deleted = ArtifactRepository::delete(&pool, non_existent_id)
        .await
        .expect("Failed to delete artifact");

    assert!(!deleted);
}

// ============================================================================
// Enum Type Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_all_types() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("all_types");

    let types = vec![
        ArtifactType::FileBinary,
        ArtifactType::FileDataTable,
        ArtifactType::FileImage,
        ArtifactType::FileText,
        ArtifactType::Other,
        ArtifactType::Progress,
        ArtifactType::Url,
    ];

    for artifact_type in types {
        let mut input = fixture.create_input(&format!("{:?}", artifact_type));
        input.r#type = artifact_type;

        let created = ArtifactRepository::create(&pool, input)
            .await
            .expect("Failed to create artifact");

        assert_eq!(created.r#type, artifact_type);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_all_scopes() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("all_scopes");

    let scopes = vec![
        OwnerType::System,
        OwnerType::Identity,
        OwnerType::Pack,
        OwnerType::Action,
        OwnerType::Sensor,
    ];

    for scope in scopes {
        let mut input = fixture.create_input(&format!("{:?}", scope));
        input.scope = scope;

        let created = ArtifactRepository::create(&pool, input)
            .await
            .expect("Failed to create artifact");

        assert_eq!(created.scope, scope);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_all_retention_policies() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("all_retention");

    let policies = vec![
        RetentionPolicyType::Versions,
        RetentionPolicyType::Days,
        RetentionPolicyType::Hours,
        RetentionPolicyType::Minutes,
    ];

    for policy in policies {
        let mut input = fixture.create_input(&format!("{:?}", policy));
        input.retention_policy = policy;

        let created = ArtifactRepository::create(&pool, input)
            .await
            .expect("Failed to create artifact");

        assert_eq!(created.retention_policy, policy);
    }
}

// ============================================================================
// Specialized Query Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_scope() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_scope");

    // Create artifacts with different scopes
    let mut identity_input = fixture.create_input("identity_scope");
    identity_input.scope = OwnerType::Identity;
    let identity_artifact = ArtifactRepository::create(&pool, identity_input)
        .await
        .expect("Failed to create identity artifact");

    let mut system_input = fixture.create_input("system_scope");
    system_input.scope = OwnerType::System;
    ArtifactRepository::create(&pool, system_input)
        .await
        .expect("Failed to create system artifact");

    // Find by identity scope
    let identity_artifacts = ArtifactRepository::find_by_scope(&pool, OwnerType::Identity)
        .await
        .expect("Failed to find by scope");

    assert!(identity_artifacts
        .iter()
        .any(|a| a.id == identity_artifact.id));
    assert!(identity_artifacts
        .iter()
        .all(|a| a.scope == OwnerType::Identity));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_owner() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_owner");

    let owner1 = fixture.unique_owner("owner1");
    let owner2 = fixture.unique_owner("owner2");

    // Create artifacts with different owners
    let mut input1 = fixture.create_input("owner1");
    input1.owner = owner1.clone();
    let artifact1 = ArtifactRepository::create(&pool, input1)
        .await
        .expect("Failed to create artifact 1");

    let mut input2 = fixture.create_input("owner2");
    input2.owner = owner2.clone();
    ArtifactRepository::create(&pool, input2)
        .await
        .expect("Failed to create artifact 2");

    // Find by owner1
    let owner1_artifacts = ArtifactRepository::find_by_owner(&pool, &owner1)
        .await
        .expect("Failed to find by owner");

    assert!(owner1_artifacts.iter().any(|a| a.id == artifact1.id));
    assert!(owner1_artifacts.iter().all(|a| a.owner == owner1));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_type() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_type");

    // Create artifacts with different types
    let mut image_input = fixture.create_input("image");
    image_input.r#type = ArtifactType::FileImage;
    let image_artifact = ArtifactRepository::create(&pool, image_input)
        .await
        .expect("Failed to create image artifact");

    let mut text_input = fixture.create_input("text");
    text_input.r#type = ArtifactType::FileText;
    ArtifactRepository::create(&pool, text_input)
        .await
        .expect("Failed to create text artifact");

    // Find by image type
    let image_artifacts = ArtifactRepository::find_by_type(&pool, ArtifactType::FileImage)
        .await
        .expect("Failed to find by type");

    assert!(image_artifacts.iter().any(|a| a.id == image_artifact.id));
    assert!(image_artifacts
        .iter()
        .all(|a| a.r#type == ArtifactType::FileImage));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_scope_and_owner() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_scope_and_owner");

    let pack_owner = fixture.unique_owner("pack");

    // Create artifact with pack scope and specific owner
    let mut pack_input = fixture.create_input("pack");
    pack_input.scope = OwnerType::Pack;
    pack_input.owner = pack_owner.clone();
    let pack_artifact = ArtifactRepository::create(&pool, pack_input)
        .await
        .expect("Failed to create pack artifact");

    // Create artifact with same scope but different owner
    let mut other_input = fixture.create_input("other");
    other_input.scope = OwnerType::Pack;
    other_input.owner = fixture.unique_owner("other");
    ArtifactRepository::create(&pool, other_input)
        .await
        .expect("Failed to create other artifact");

    // Find by scope and owner
    let artifacts =
        ArtifactRepository::find_by_scope_and_owner(&pool, OwnerType::Pack, &pack_owner)
            .await
            .expect("Failed to find by scope and owner");

    assert!(artifacts.iter().any(|a| a.id == pack_artifact.id));
    assert!(artifacts
        .iter()
        .all(|a| a.scope == OwnerType::Pack && a.owner == pack_owner));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_retention_policy() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("find_by_retention");

    // Create artifacts with different retention policies
    let mut days_input = fixture.create_input("days");
    days_input.retention_policy = RetentionPolicyType::Days;
    let days_artifact = ArtifactRepository::create(&pool, days_input)
        .await
        .expect("Failed to create days artifact");

    let mut hours_input = fixture.create_input("hours");
    hours_input.retention_policy = RetentionPolicyType::Hours;
    ArtifactRepository::create(&pool, hours_input)
        .await
        .expect("Failed to create hours artifact");

    // Find by days retention policy
    let days_artifacts =
        ArtifactRepository::find_by_retention_policy(&pool, RetentionPolicyType::Days)
            .await
            .expect("Failed to find by retention policy");

    assert!(days_artifacts.iter().any(|a| a.id == days_artifact.id));
    assert!(days_artifacts
        .iter()
        .all(|a| a.retention_policy == RetentionPolicyType::Days));
}

// ============================================================================
// Timestamp Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_timestamps_auto_set_on_create() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("timestamps_create");
    let input = fixture.create_input("timestamps");

    let artifact = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact");

    assert!(artifact.created.timestamp() > 0);
    assert!(artifact.updated.timestamp() > 0);
    assert_eq!(artifact.created, artifact.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_updated_timestamp_changes_on_update() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("timestamps_update");
    let input = fixture.create_input("update_time");

    let created = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact");

    // Small delay to ensure timestamp difference
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let update_input = UpdateArtifactInput {
        r#ref: Some(fixture.unique_ref("updated")),
        ..Default::default()
    };

    let updated = ArtifactRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update artifact");

    assert_eq!(updated.created, created.created);
    assert!(updated.updated > created.updated);
}

// ============================================================================
// Edge Cases and Validation Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_with_empty_owner() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("empty_owner");
    let mut input = fixture.create_input("empty");
    input.owner = String::new();

    let artifact = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact with empty owner");

    assert_eq!(artifact.owner, "");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_with_special_characters_in_ref() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("special_chars");
    let mut input = fixture.create_input("special");
    input.r#ref = format!(
        "{}_test/path/to/file-with-special_chars.txt",
        fixture.unique_ref("spec")
    );

    let artifact = ArtifactRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create artifact with special chars");

    assert_eq!(artifact.r#ref, input.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_with_zero_retention_limit() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("zero_retention");
    let mut input = fixture.create_input("zero");
    input.retention_limit = 0;

    let artifact = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact with zero retention limit");

    assert_eq!(artifact.retention_limit, 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_with_negative_retention_limit() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("negative_retention");
    let mut input = fixture.create_input("negative");
    input.retention_limit = -1;

    let artifact = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact with negative retention limit");

    assert_eq!(artifact.retention_limit, -1);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_with_large_retention_limit() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("large_retention");
    let mut input = fixture.create_input("large");
    input.retention_limit = i32::MAX;

    let artifact = ArtifactRepository::create(&pool, input)
        .await
        .expect("Failed to create artifact with large retention limit");

    assert_eq!(artifact.retention_limit, i32::MAX);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_artifact_with_long_ref() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("long_ref");
    let mut input = fixture.create_input("long");
    input.r#ref = format!("{}_{}", fixture.unique_ref("long"), "a".repeat(500));

    let artifact = ArtifactRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create artifact with long ref");

    assert_eq!(artifact.r#ref, input.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_multiple_artifacts_same_ref_allowed() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("duplicate_ref");
    let same_ref = fixture.unique_ref("same");

    // Create first artifact
    let mut input1 = fixture.create_input("dup1");
    input1.r#ref = same_ref.clone();
    let artifact1 = ArtifactRepository::create(&pool, input1)
        .await
        .expect("Failed to create first artifact");

    // Create second artifact with same ref (should be allowed)
    let mut input2 = fixture.create_input("dup2");
    input2.r#ref = same_ref.clone();
    let artifact2 = ArtifactRepository::create(&pool, input2)
        .await
        .expect("Failed to create second artifact with same ref");

    assert_ne!(artifact1.id, artifact2.id);
    assert_eq!(artifact1.r#ref, artifact2.r#ref);
}

// ============================================================================
// Query Result Ordering Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_scope_ordered_by_created() {
    let pool = setup_db().await;
    let fixture = ArtifactFixture::new("scope_ordering");

    // Create multiple artifacts with same scope
    let mut artifacts = Vec::new();
    for i in 0..3 {
        let mut input = fixture.create_input(&format!("order_{}", i));
        input.scope = OwnerType::Action;

        let artifact = ArtifactRepository::create(&pool, input)
            .await
            .expect("Failed to create artifact");
        artifacts.push(artifact);

        // Small delay to ensure different timestamps
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    let found = ArtifactRepository::find_by_scope(&pool, OwnerType::Action)
        .await
        .expect("Failed to find by scope");

    // Find our test artifacts in the results
    let test_artifacts: Vec<_> = found
        .iter()
        .filter(|a| artifacts.iter().any(|ta| ta.id == a.id))
        .collect();

    // Should be ordered by created DESC (newest first)
    for i in 0..test_artifacts.len().saturating_sub(1) {
        assert!(test_artifacts[i].created >= test_artifacts[i + 1].created);
    }
}
