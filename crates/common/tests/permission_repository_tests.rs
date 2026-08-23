//! Integration tests for Permission repositories (PermissionSet and PermissionAssignment)

use attune_common::{
    models::identity::*,
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
            UpdatePermissionSetInput,
        },
        pack::{CreatePackInput, PackRepository},
        Create, Delete, FindById, List, Update,
    },
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};

mod helpers;
use helpers::{create_test_pool, database_clock, set_created_for_test, set_updated_for_test};

static PERMISSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test fixture for creating unique permission sets
struct PermissionSetFixture {
    pool: PgPool,
    id_suffix: String,
    internal_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl PermissionSetFixture {
    fn new(pool: PgPool) -> Self {
        let counter = PERMISSION_COUNTER.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Hash the thread ID to get a unique number
        let thread_id = std::thread::current().id();
        let thread_hash = format!("{:?}", thread_id)
            .chars()
            .filter(|c| c.is_numeric())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0);
        // Add random component for absolute uniqueness
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hash, Hasher};
        let random_state = RandomState::new();
        let mut hasher = random_state.build_hasher();
        timestamp.hash(&mut hasher);
        counter.hash(&mut hasher);
        thread_hash.hash(&mut hasher);
        let random_hash = hasher.finish();
        // Create a unique lowercase alphanumeric suffix combining all sources of uniqueness
        let id_suffix = format!("{:x}", random_hash);
        Self {
            pool,
            id_suffix,
            internal_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    fn unique_ref(&self, base: &str) -> String {
        let seq = self.internal_counter.fetch_add(1, Ordering::SeqCst);
        format!("test.{}_{}_{}", base, self.id_suffix, seq)
    }

    async fn create_pack(&self) -> i64 {
        let seq = self.internal_counter.fetch_add(1, Ordering::SeqCst);
        let pack_ref = format!("testpack_{}_{}", self.id_suffix, seq);
        let input = CreatePackInput {
            r#ref: pack_ref,
            version: "1.0.0".to_string(),
            label: "Test Pack".to_string(),
            description: Some("Test pack for permissions".to_string()),
            tags: vec![],
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        };
        PackRepository::create(&self.pool, input)
            .await
            .expect("Failed to create pack")
            .id
    }

    async fn create_identity(&self) -> i64 {
        let seq = self.internal_counter.fetch_add(1, Ordering::SeqCst);
        let login = format!("testuser_{}_{}", self.id_suffix, seq);
        let input = CreateIdentityInput {
            login,
            display_name: Some("Test User".to_string()),
            attributes: json!({}),
            password_hash: None,
        };
        IdentityRepository::create(&self.pool, input)
            .await
            .expect("Failed to create identity")
            .id
    }

    async fn create_permission_set(
        &self,
        ref_name: &str,
        pack_id: Option<i64>,
        pack_ref: Option<String>,
        grants: serde_json::Value,
    ) -> PermissionSet {
        let input = CreatePermissionSetInput {
            r#ref: ref_name.to_string(),
            pack: pack_id,
            pack_ref,
            label: Some("Test Permission Set".to_string()),
            description: Some("Test description".to_string()),
            grants,
        };

        PermissionSetRepository::create(&self.pool, input)
            .await
            .expect("Failed to create permission set")
    }

    async fn create_default(&self) -> PermissionSet {
        let ref_name = self.unique_ref("permset");
        self.create_permission_set(&ref_name, None, None, json!([]))
            .await
    }

    async fn create_with_pack(&self) -> (i64, PermissionSet) {
        let pack_id = self.create_pack().await;
        let ref_name = self.unique_ref("permset");
        // Get the pack_ref from the last created pack - extract from pack
        let pack = PackRepository::find_by_id(&self.pool, pack_id)
            .await
            .expect("Failed to find pack")
            .expect("Pack not found");
        let pack_ref = pack.r#ref;
        let permset = self
            .create_permission_set(&ref_name, Some(pack_id), Some(pack_ref), json!([]))
            .await;
        (pack_id, permset)
    }

    async fn create_with_grants(&self, grants: serde_json::Value) -> PermissionSet {
        let ref_name = self.unique_ref("permset");
        self.create_permission_set(&ref_name, None, None, grants)
            .await
    }

    async fn create_assignment(&self, identity_id: i64, permset_id: i64) -> PermissionAssignment {
        let input = CreatePermissionAssignmentInput {
            identity: identity_id,
            permset: permset_id,
        };
        PermissionAssignmentRepository::create(&self.pool, input)
            .await
            .expect("Failed to create permission assignment")
    }
}

// ============================================================================
// PermissionSet Repository Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_permission_set_minimal() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let ref_name = fixture.unique_ref("minimal");
    let input = CreatePermissionSetInput {
        r#ref: ref_name.clone(),
        pack: None,
        pack_ref: None,
        label: Some("Minimal Permission Set".to_string()),
        description: None,
        grants: json!([]),
    };

    let permset = PermissionSetRepository::create(&pool, input)
        .await
        .expect("Failed to create permission set");

    assert!(permset.id > 0);
    assert_eq!(permset.r#ref, ref_name);
    assert_eq!(permset.label, Some("Minimal Permission Set".to_string()));
    assert!(permset.description.is_none());
    assert_eq!(permset.grants, json!([]));
    assert!(permset.pack.is_none());
    assert!(permset.pack_ref.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_permission_set_with_pack() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let pack_id = fixture.create_pack().await;
    let ref_name = fixture.unique_ref("with_pack");
    let pack_ref = format!("testpack_{}", fixture.id_suffix);

    let input = CreatePermissionSetInput {
        r#ref: ref_name.clone(),
        pack: Some(pack_id),
        pack_ref: Some(pack_ref.clone()),
        label: Some("Pack Permission Set".to_string()),
        description: Some("Permission set from pack".to_string()),
        grants: json!([
            {"resource": "actions", "permission": "read"},
            {"resource": "actions", "permission": "execute"}
        ]),
    };

    let permset = PermissionSetRepository::create(&pool, input)
        .await
        .expect("Failed to create permission set");

    assert_eq!(permset.pack, Some(pack_id));
    assert_eq!(permset.pack_ref, Some(pack_ref));
    assert!(permset.grants.is_array());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_permission_set_with_complex_grants() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let _ref_name = fixture.unique_ref("complex");
    let grants = json!([
        {
            "resource": "executions",
            "permissions": ["read", "write", "delete"],
            "filters": {"pack": "core"}
        },
        {
            "resource": "actions",
            "permissions": ["execute"],
            "filters": {"tags": ["safe"]}
        }
    ]);

    let permset = fixture.create_with_grants(grants.clone()).await;

    assert_eq!(permset.grants, grants);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_set_ref_format_validation() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    // Valid format: pack.name
    let valid_ref = fixture.unique_ref("valid");
    let input = CreatePermissionSetInput {
        r#ref: valid_ref,
        pack: None,
        pack_ref: None,
        label: None,
        description: None,
        grants: json!([]),
    };
    let result = PermissionSetRepository::create(&pool, input).await;
    assert!(result.is_ok());

    // Invalid format: no dot
    let invalid_ref = format!("nodot_{}", fixture.id_suffix);
    let input = CreatePermissionSetInput {
        r#ref: invalid_ref,
        pack: None,
        pack_ref: None,
        label: None,
        description: None,
        grants: json!([]),
    };
    let result = PermissionSetRepository::create(&pool, input).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_set_ref_lowercase() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    // Create with uppercase - should fail due to CHECK constraint
    let upper_ref = format!("Test.UPPERCASE_{}", fixture.id_suffix);
    let input = CreatePermissionSetInput {
        r#ref: upper_ref,
        pack: None,
        pack_ref: None,
        label: None,
        description: None,
        grants: json!([]),
    };
    let result = PermissionSetRepository::create(&pool, input).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_set_duplicate_ref() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let ref_name = fixture.unique_ref("duplicate");
    let input = CreatePermissionSetInput {
        r#ref: ref_name.clone(),
        pack: None,
        pack_ref: None,
        label: None,
        description: None,
        grants: json!([]),
    };

    // First create should succeed
    let result1 = PermissionSetRepository::create(&pool, input.clone()).await;
    assert!(result1.is_ok());

    // Second create with same ref should fail
    let result2 = PermissionSetRepository::create(&pool, input).await;
    assert!(result2.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_permission_set_by_id() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let found = PermissionSetRepository::find_by_id(&pool, created.id)
        .await
        .expect("Failed to find permission set")
        .expect("Permission set not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
    assert_eq!(found.label, created.label);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_permission_set_by_id_not_found() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    let result = PermissionSetRepository::find_by_id(&pool, 999_999_999)
        .await
        .expect("Query should succeed");

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_permission_sets() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let p1 = fixture.create_default().await;
    let p2 = fixture.create_default().await;
    let p3 = fixture.create_default().await;

    let permsets = PermissionSetRepository::list(&pool)
        .await
        .expect("Failed to list permission sets");

    let ids: Vec<i64> = permsets.iter().map(|p| p.id).collect();
    assert!(ids.contains(&p1.id));
    assert!(ids.contains(&p2.id));
    assert!(ids.contains(&p3.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_permission_set_label() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let update_input = UpdatePermissionSetInput {
        label: Some("Updated Label".to_string()),
        description: None,
        grants: None,
    };

    let updated = PermissionSetRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update permission set");

    assert_eq!(updated.label, Some("Updated Label".to_string()));
    assert_eq!(updated.description, created.description);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_permission_set_grants() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let created = fixture.create_with_grants(json!([])).await;

    let new_grants = json!([
        {"resource": "packs", "permission": "read"},
        {"resource": "actions", "permission": "execute"}
    ]);

    let update_input = UpdatePermissionSetInput {
        label: None,
        description: None,
        grants: Some(new_grants.clone()),
    };

    let updated = PermissionSetRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update permission set");

    assert_eq!(updated.grants, new_grants);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_permission_set_all_fields() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let new_grants = json!([{"resource": "all", "permission": "admin"}]);
    let update_input = UpdatePermissionSetInput {
        label: Some("New Label".to_string()),
        description: Some("New Description".to_string()),
        grants: Some(new_grants.clone()),
    };

    let updated = PermissionSetRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update permission set");

    assert_eq!(updated.label, Some("New Label".to_string()));
    assert_eq!(updated.description, Some("New Description".to_string()));
    assert_eq!(updated.grants, new_grants);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_permission_set_no_changes() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let update_input = UpdatePermissionSetInput {
        label: None,
        description: None,
        grants: None,
    };

    let updated = PermissionSetRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update permission set");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.r#ref, created.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_permission_set_timestamps() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let created = fixture.create_default().await;
    let created_timestamp = created.created;
    let original_updated = created.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "permission_set", created.id, original_updated).await;

    let update_input = UpdatePermissionSetInput {
        label: Some("Updated".to_string()),
        description: None,
        grants: None,
    };

    let updated = PermissionSetRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update permission set");

    assert_eq!(updated.created, created_timestamp);
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_permission_set() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let created = fixture.create_default().await;

    let deleted = PermissionSetRepository::delete(&pool, created.id)
        .await
        .expect("Failed to delete permission set");

    assert!(deleted);

    let found = PermissionSetRepository::find_by_id(&pool, created.id)
        .await
        .expect("Query should succeed");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_permission_set_not_found() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    let deleted = PermissionSetRepository::delete(&pool, 999_999_999)
        .await
        .expect("Delete should succeed");

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_set_cascade_from_pack() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let (pack_id, permset) = fixture.create_with_pack().await;

    // Delete pack - permission set should be cascade deleted
    let deleted = PackRepository::delete(&pool, pack_id)
        .await
        .expect("Failed to delete pack");
    assert!(deleted);

    // Permission set should no longer exist
    let found = PermissionSetRepository::find_by_id(&pool, permset.id)
        .await
        .expect("Query should succeed");
    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_set_timestamps_auto_set() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let before = database_clock(&pool).await;
    let permset = fixture.create_default().await;
    let after = database_clock(&pool).await;

    assert!(permset.created >= before);
    assert!(permset.created <= after);
    assert!(permset.updated >= before);
    assert!(permset.updated <= after);
}

// ============================================================================
// PermissionAssignment Repository Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_permission_assignment() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let permset = fixture.create_default().await;

    let assignment = fixture.create_assignment(identity_id, permset.id).await;

    assert!(assignment.id > 0);
    assert_eq!(assignment.identity, identity_id);
    assert_eq!(assignment.permset, permset.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_permission_assignment_duplicate() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let permset = fixture.create_default().await;

    // First assignment should succeed
    let result1 = fixture.create_assignment(identity_id, permset.id).await;
    assert!(result1.id > 0);

    // Second assignment with same identity+permset should fail (unique constraint)
    let input = CreatePermissionAssignmentInput {
        identity: identity_id,
        permset: permset.id,
    };
    let result2 = PermissionAssignmentRepository::create(&pool, input).await;
    assert!(result2.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_permission_assignment_invalid_identity() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let permset = fixture.create_default().await;

    let input = CreatePermissionAssignmentInput {
        identity: 999_999_999,
        permset: permset.id,
    };

    let result = PermissionAssignmentRepository::create(&pool, input).await;
    assert!(result.is_err()); // Foreign key violation
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_permission_assignment_invalid_permset() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;

    let input = CreatePermissionAssignmentInput {
        identity: identity_id,
        permset: 999_999_999,
    };

    let result = PermissionAssignmentRepository::create(&pool, input).await;
    assert!(result.is_err()); // Foreign key violation
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_permission_assignment_by_id() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let permset = fixture.create_default().await;
    let created = fixture.create_assignment(identity_id, permset.id).await;

    let found = PermissionAssignmentRepository::find_by_id(&pool, created.id)
        .await
        .expect("Failed to find assignment")
        .expect("Assignment not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.identity, identity_id);
    assert_eq!(found.permset, permset.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_permission_assignment_by_id_not_found() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    let result = PermissionAssignmentRepository::find_by_id(&pool, 999_999_999)
        .await
        .expect("Query should succeed");

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_permission_assignments() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let p1 = fixture.create_default().await;
    let p2 = fixture.create_default().await;

    let a1 = fixture.create_assignment(identity_id, p1.id).await;
    let a2 = fixture.create_assignment(identity_id, p2.id).await;

    let assignments = PermissionAssignmentRepository::list(&pool)
        .await
        .expect("Failed to list assignments");

    let ids: Vec<i64> = assignments.iter().map(|a| a.id).collect();
    assert!(ids.contains(&a1.id));
    assert!(ids.contains(&a2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_assignments_by_identity() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity1 = fixture.create_identity().await;
    let identity2 = fixture.create_identity().await;
    let p1 = fixture.create_default().await;
    let p2 = fixture.create_default().await;

    let a1 = fixture.create_assignment(identity1, p1.id).await;
    let a2 = fixture.create_assignment(identity1, p2.id).await;
    let _a3 = fixture.create_assignment(identity2, p1.id).await;

    let assignments = PermissionAssignmentRepository::find_by_identity(&pool, identity1)
        .await
        .expect("Failed to find assignments");

    assert_eq!(assignments.len(), 2);
    let ids: Vec<i64> = assignments.iter().map(|a| a.id).collect();
    assert!(ids.contains(&a1.id));
    assert!(ids.contains(&a2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_assignments_by_identity_empty() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;

    let assignments = PermissionAssignmentRepository::find_by_identity(&pool, identity_id)
        .await
        .expect("Failed to find assignments");

    assert!(assignments.is_empty());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_permission_assignment() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let permset = fixture.create_default().await;
    let created = fixture.create_assignment(identity_id, permset.id).await;

    let deleted = PermissionAssignmentRepository::delete(&pool, created.id)
        .await
        .expect("Failed to delete assignment");

    assert!(deleted);

    let found = PermissionAssignmentRepository::find_by_id(&pool, created.id)
        .await
        .expect("Query should succeed");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_permission_assignment_not_found() {
    let pool = create_test_pool().await.expect("Failed to create pool");

    let deleted = PermissionAssignmentRepository::delete(&pool, 999_999_999)
        .await
        .expect("Delete should succeed");

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_assignment_cascade_from_identity() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let permset = fixture.create_default().await;
    let assignment = fixture.create_assignment(identity_id, permset.id).await;

    // Delete identity - assignment should be cascade deleted
    let deleted = IdentityRepository::delete(&pool, identity_id)
        .await
        .expect("Failed to delete identity");
    assert!(deleted);

    // Assignment should no longer exist
    let found = PermissionAssignmentRepository::find_by_id(&pool, assignment.id)
        .await
        .expect("Query should succeed");
    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_assignment_cascade_from_permset() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let permset = fixture.create_default().await;
    let assignment = fixture.create_assignment(identity_id, permset.id).await;

    // Delete permission set - assignment should be cascade deleted
    let deleted = PermissionSetRepository::delete(&pool, permset.id)
        .await
        .expect("Failed to delete permission set");
    assert!(deleted);

    // Assignment should no longer exist
    let found = PermissionAssignmentRepository::find_by_id(&pool, assignment.id)
        .await
        .expect("Query should succeed");
    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_assignment_timestamp_auto_set() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let permset = fixture.create_default().await;

    let before = database_clock(&pool).await;
    let assignment = fixture.create_assignment(identity_id, permset.id).await;
    let after = database_clock(&pool).await;

    assert!(assignment.created >= before);
    assert!(assignment.created <= after);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_multiple_identities_same_permset() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity1 = fixture.create_identity().await;
    let identity2 = fixture.create_identity().await;
    let identity3 = fixture.create_identity().await;
    let permset = fixture.create_default().await;

    let a1 = fixture.create_assignment(identity1, permset.id).await;
    let a2 = fixture.create_assignment(identity2, permset.id).await;
    let a3 = fixture.create_assignment(identity3, permset.id).await;

    // All should have same permset
    assert_eq!(a1.permset, permset.id);
    assert_eq!(a2.permset, permset.id);
    assert_eq!(a3.permset, permset.id);

    // But different identities
    assert_eq!(a1.identity, identity1);
    assert_eq!(a2.identity, identity2);
    assert_eq!(a3.identity, identity3);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_one_identity_multiple_permsets() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let p1 = fixture.create_default().await;
    let p2 = fixture.create_default().await;
    let p3 = fixture.create_default().await;

    let a1 = fixture.create_assignment(identity_id, p1.id).await;
    let a2 = fixture.create_assignment(identity_id, p2.id).await;
    let a3 = fixture.create_assignment(identity_id, p3.id).await;

    // All should have same identity
    assert_eq!(a1.identity, identity_id);
    assert_eq!(a2.identity, identity_id);
    assert_eq!(a3.identity, identity_id);

    // But different permsets
    assert_eq!(a1.permset, p1.id);
    assert_eq!(a2.permset, p2.id);
    assert_eq!(a3.permset, p3.id);

    // Query by identity should return all 3
    let assignments = PermissionAssignmentRepository::find_by_identity(&pool, identity_id)
        .await
        .expect("Failed to find assignments");

    assert_eq!(assignments.len(), 3);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_set_ordering() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let ref1 = fixture.unique_ref("aaa");
    let ref2 = fixture.unique_ref("bbb");
    let ref3 = fixture.unique_ref("ccc");

    let _p1 = fixture
        .create_permission_set(&ref1, None, None, json!([]))
        .await;
    let _p2 = fixture
        .create_permission_set(&ref2, None, None, json!([]))
        .await;
    let _p3 = fixture
        .create_permission_set(&ref3, None, None, json!([]))
        .await;

    let permsets = PermissionSetRepository::list(&pool)
        .await
        .expect("Failed to list permission sets");

    // Should be ordered by ref ASC
    let our_sets: Vec<&PermissionSet> = permsets
        .iter()
        .filter(|p| p.r#ref.starts_with("test."))
        .filter(|p| p.r#ref == ref1 || p.r#ref == ref2 || p.r#ref == ref3)
        .collect();

    if our_sets.len() == 3 {
        let pos1 = permsets.iter().position(|p| p.r#ref == ref1).unwrap();
        let pos2 = permsets.iter().position(|p| p.r#ref == ref2).unwrap();
        let pos3 = permsets.iter().position(|p| p.r#ref == ref3).unwrap();

        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_permission_assignment_ordering() {
    let pool = create_test_pool().await.expect("Failed to create pool");
    let fixture = PermissionSetFixture::new(pool.clone());

    let identity_id = fixture.create_identity().await;
    let p1 = fixture.create_default().await;
    let p2 = fixture.create_default().await;
    let p3 = fixture.create_default().await;

    let a1 = fixture.create_assignment(identity_id, p1.id).await;
    let a2 = fixture.create_assignment(identity_id, p2.id).await;
    let a3 = fixture.create_assignment(identity_id, p3.id).await;
    set_created_for_test(
        &pool,
        "permission_assignment",
        &[a1.id, a2.id, a3.id],
        chrono::Utc::now() - chrono::Duration::seconds(1),
    )
    .await;

    let assignments = PermissionAssignmentRepository::list(&pool)
        .await
        .expect("Failed to list assignments");

    // Equal creation timestamps are ordered by ID DESC.
    let ids: Vec<i64> = assignments.iter().map(|a| a.id).collect();
    if ids.contains(&a1.id) && ids.contains(&a2.id) && ids.contains(&a3.id) {
        let pos1 = ids.iter().position(|&id| id == a1.id).unwrap();
        let pos2 = ids.iter().position(|&id| id == a2.id).unwrap();
        let pos3 = ids.iter().position(|&id| id == a3.id).unwrap();

        // Newest (a3) should come before older ones
        assert!(pos3 < pos2);
        assert!(pos2 < pos1);
    }
}
