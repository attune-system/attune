//! Integration tests for Runtime repository
//!
//! Tests cover CRUD operations, specialized queries, constraints,
//! enum handling, timestamps, and edge cases.

use attune_common::repositories::runtime::{
    CreateRuntimeInput, RuntimeRepository, UpdateRuntimeInput,
};
use attune_common::repositories::{Create, Delete, FindById, FindByRef, List, Patch, Update};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

mod helpers;
use helpers::{create_test_pool, database_clock, set_updated_for_test};

// Global counter for unique IDs across all tests
static GLOBAL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test fixture for creating unique runtime data
struct RuntimeFixture {
    sequence: AtomicU64,
    test_id: String,
}

impl RuntimeFixture {
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

    fn create_input(&self, ref_suffix: &str) -> CreateRuntimeInput {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let name = format!("test_runtime_{}_{}", ref_suffix, seq);
        let r#ref = format!("{}.{}", self.test_id, name);

        CreateRuntimeInput {
            r#ref,
            pack: None,
            pack_ref: None,
            description: Some(format!("Test runtime {}", seq)),
            name,
            aliases: vec![],
            distributions: json!({
                "linux": { "supported": true, "versions": ["ubuntu20.04", "ubuntu22.04"] },
                "darwin": { "supported": true, "versions": ["12", "13"] }
            }),
            installation: Some(json!({
                "method": "pip",
                "packages": ["requests", "pyyaml"]
            })),
            execution_config: json!({
                "interpreter": {
                    "binary": "python3",
                    "args": ["-u"],
                    "file_extension": ".py"
                }
            }),
            auto_detected: false,
            detection_config: json!({}),
        }
    }

    fn create_minimal_input(&self, ref_suffix: &str) -> CreateRuntimeInput {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let name = format!("minimal_{}_{}", ref_suffix, seq);
        let r#ref = format!("{}.{}", self.test_id, name);

        CreateRuntimeInput {
            r#ref,
            pack: None,
            pack_ref: None,
            description: None,
            name,
            aliases: vec![],
            distributions: json!({}),
            installation: None,
            execution_config: json!({
                "interpreter": {
                    "binary": "/bin/bash",
                    "args": [],
                    "file_extension": ".sh"
                }
            }),
            auto_detected: false,
            detection_config: json!({}),
        }
    }
}

async fn setup_db() -> attune_common::test_database::TestDatabase {
    create_test_pool()
        .await
        .expect("Failed to create test pool")
}

// ============================================================================
// Basic CRUD Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_runtime() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("create_runtime");
    let input = fixture.create_input("basic");

    let runtime = RuntimeRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create runtime");

    assert!(runtime.id > 0);
    assert_eq!(runtime.r#ref, input.r#ref);
    assert_eq!(runtime.pack, input.pack);
    assert_eq!(runtime.pack_ref, input.pack_ref);
    assert_eq!(runtime.description, input.description);
    assert_eq!(runtime.name, input.name);
    assert_eq!(runtime.distributions, input.distributions);
    assert_eq!(runtime.installation, input.installation);
    assert!(runtime.created > chrono::Utc::now() - chrono::Duration::seconds(5));
    assert!(runtime.updated > chrono::Utc::now() - chrono::Duration::seconds(5));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_runtime_minimal() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("create_runtime_minimal");
    let input = fixture.create_minimal_input("minimal");

    let runtime = RuntimeRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create minimal runtime");

    assert!(runtime.id > 0);
    assert_eq!(runtime.r#ref, input.r#ref);
    assert_eq!(runtime.description, None);
    assert_eq!(runtime.pack, None);
    assert_eq!(runtime.pack_ref, None);
    assert_eq!(runtime.installation, None);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_runtime_by_id() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("find_by_id");
    let input = fixture.create_input("findable");

    let created = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    let found = RuntimeRepository::find_by_id(&pool, created.id)
        .await
        .expect("Failed to find runtime")
        .expect("Runtime not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_runtime_by_id_not_found() {
    let pool = setup_db().await;

    let result = RuntimeRepository::find_by_id(&pool, 999999999)
        .await
        .expect("Query should succeed");

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_runtime_by_ref() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("find_by_ref");
    let input = fixture.create_input("reftest");

    let created = RuntimeRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create runtime");

    let found = RuntimeRepository::find_by_ref(&pool, &input.r#ref)
        .await
        .expect("Failed to find runtime")
        .expect("Runtime not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_runtime_by_ref_not_found() {
    let pool = setup_db().await;

    let result = RuntimeRepository::find_by_ref(&pool, "nonexistent.ref.999999")
        .await
        .expect("Query should succeed");

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_runtimes() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("list_runtimes");

    let input1 = fixture.create_input("list1");
    let input2 = fixture.create_input("list2");

    let created1 = RuntimeRepository::create(&pool, input1)
        .await
        .expect("Failed to create runtime 1");
    let created2 = RuntimeRepository::create(&pool, input2)
        .await
        .expect("Failed to create runtime 2");

    let list = RuntimeRepository::list(&pool)
        .await
        .expect("Failed to list runtimes");

    assert!(list.len() >= 2);
    assert!(list.iter().any(|r| r.id == created1.id));
    assert!(list.iter().any(|r| r.id == created2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_runtime() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("update_runtime");
    let input = fixture.create_input("update");

    let created = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    let update_input = UpdateRuntimeInput {
        description: Some(Patch::Set("Updated description".to_string())),
        name: Some("updated_name".to_string()),
        distributions: Some(json!({
            "linux": { "supported": false }
        })),
        installation: Some(Patch::Set(json!({
            "method": "npm"
        }))),
        execution_config: None,
        ..Default::default()
    };

    let updated = RuntimeRepository::update(&pool, created.id, update_input.clone())
        .await
        .expect("Failed to update runtime");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.description, Some("Updated description".to_string()));
    assert_eq!(updated.name, update_input.name.unwrap());
    assert_eq!(updated.distributions, update_input.distributions.unwrap());
    assert_eq!(updated.installation, Some(json!({ "method": "npm" })));
    assert!(updated.updated > created.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_runtime_partial() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("update_partial");
    let input = fixture.create_input("partial");

    let created = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    let update_input = UpdateRuntimeInput {
        description: Some(Patch::Set("Only description changed".to_string())),
        name: None,
        distributions: None,
        installation: None,
        execution_config: None,
        ..Default::default()
    };

    let updated = RuntimeRepository::update(&pool, created.id, update_input.clone())
        .await
        .expect("Failed to update runtime");

    assert_eq!(
        updated.description,
        Some("Only description changed".to_string())
    );
    assert_eq!(updated.name, created.name);
    assert_eq!(updated.distributions, created.distributions);
    assert_eq!(updated.installation, created.installation);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_runtime_empty() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("update_empty");
    let input = fixture.create_input("empty");

    let created = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    let update_input = UpdateRuntimeInput::default();

    let result = RuntimeRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update runtime");

    // Should return existing entity unchanged
    assert_eq!(result.id, created.id);
    assert_eq!(result.description, created.description);
    assert_eq!(result.name, created.name);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_runtime() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("delete_runtime");
    let input = fixture.create_input("deletable");

    let created = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    let deleted = RuntimeRepository::delete(&pool, created.id)
        .await
        .expect("Failed to delete runtime");

    assert!(deleted);

    let found = RuntimeRepository::find_by_id(&pool, created.id)
        .await
        .expect("Query should succeed");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_runtime_not_found() {
    let pool = setup_db().await;

    let deleted = RuntimeRepository::delete(&pool, 999999999)
        .await
        .expect("Delete should succeed");

    assert!(!deleted);
}

// ============================================================================
// Specialized Query Tests
// ============================================================================

// #[tokio::test]
// async fn test_find_by_type_action() {
//     // RuntimeType and find_by_type no longer exist
// }

// #[tokio::test]
// async fn test_find_by_type_sensor() {
//     // RuntimeType and find_by_type no longer exist
// }

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_pack() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("find_by_pack");

    // Create a pack first
    use attune_common::repositories::pack::{CreatePackInput, PackRepository};

    let pack_input = CreatePackInput {
        r#ref: fixture.unique_ref("testpack"),
        label: "Test Pack".to_string(),
        description: Some("Pack for runtime testing".to_string()),
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({
            "author": "Test Author",
            "email": "test@example.com"
        }),
        tags: vec!["test".to_string()],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    let pack = PackRepository::create(&pool, pack_input)
        .await
        .expect("Failed to create pack");

    // Create runtimes with and without pack association
    let mut input1 = fixture.create_input("with_pack1");
    input1.pack = Some(pack.id);
    input1.pack_ref = Some(pack.r#ref.clone());

    let mut input2 = fixture.create_input("with_pack2");
    input2.pack = Some(pack.id);
    input2.pack_ref = Some(pack.r#ref.clone());

    let input3 = fixture.create_input("without_pack");

    let created1 = RuntimeRepository::create(&pool, input1)
        .await
        .expect("Failed to create runtime 1");
    let created2 = RuntimeRepository::create(&pool, input2)
        .await
        .expect("Failed to create runtime 2");
    let _created3 = RuntimeRepository::create(&pool, input3)
        .await
        .expect("Failed to create runtime 3");

    let pack_runtimes = RuntimeRepository::find_by_pack(&pool, pack.id)
        .await
        .expect("Failed to find by pack");

    assert_eq!(pack_runtimes.len(), 2);
    assert!(pack_runtimes.iter().any(|r| r.id == created1.id));
    assert!(pack_runtimes.iter().any(|r| r.id == created2.id));
    assert!(pack_runtimes.iter().all(|r| r.pack == Some(pack.id)));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_pack_empty() {
    let pool = setup_db().await;

    let runtimes = RuntimeRepository::find_by_pack(&pool, 999999999)
        .await
        .expect("Failed to find by pack");

    assert_eq!(runtimes.len(), 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_runtime_created_successfully() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("created_test");
    let input = fixture.create_input("created");

    let runtime = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    let found = RuntimeRepository::find_by_id(&pool, runtime.id)
        .await
        .expect("Failed to find runtime")
        .expect("Runtime not found");

    assert_eq!(found.id, runtime.id);
}

// ============================================================================
// Edge Cases and Constraints
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_duplicate_ref_fails() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("duplicate_ref");
    let input = fixture.create_input("duplicate");

    RuntimeRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create first runtime");

    let result = RuntimeRepository::create(&pool, input).await;

    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_json_fields() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("json_fields");
    let input = fixture.create_input("json_test");

    let runtime = RuntimeRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create runtime");

    assert_eq!(runtime.distributions, input.distributions);
    assert_eq!(runtime.installation, input.installation);

    // Verify JSON structure
    assert_eq!(runtime.distributions["linux"]["supported"], json!(true));
    assert!(runtime.installation.is_some());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_empty_json_distributions() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("empty_json");
    let mut input = fixture.create_input("empty");
    input.distributions = json!({});
    input.installation = None;

    let runtime = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    assert_eq!(runtime.distributions, json!({}));
    assert_eq!(runtime.installation, None);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_ordering() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("list_ordering");

    let mut input1 = fixture.create_input("z_last");
    input1.r#ref = format!("{}.zzz", fixture.test_id);

    let mut input2 = fixture.create_input("a_first");
    input2.r#ref = format!("{}.aaa", fixture.test_id);

    let mut input3 = fixture.create_input("m_middle");
    input3.r#ref = format!("{}.mmm", fixture.test_id);

    RuntimeRepository::create(&pool, input1)
        .await
        .expect("Failed to create runtime 1");
    RuntimeRepository::create(&pool, input2)
        .await
        .expect("Failed to create runtime 2");
    RuntimeRepository::create(&pool, input3)
        .await
        .expect("Failed to create runtime 3");

    let list = RuntimeRepository::list(&pool)
        .await
        .expect("Failed to list runtimes");

    // Find our test runtimes in the list
    let test_runtimes: Vec<_> = list
        .iter()
        .filter(|r| r.r#ref.contains(&fixture.test_id))
        .collect();

    assert_eq!(test_runtimes.len(), 3);

    // Verify they are sorted by ref
    for i in 0..test_runtimes.len() - 1 {
        assert!(test_runtimes[i].r#ref <= test_runtimes[i + 1].r#ref);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_timestamps() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("timestamps");
    let input = fixture.create_input("timestamped");

    let before = database_clock(&pool).await;
    let runtime = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");
    let after = database_clock(&pool).await;

    assert!(runtime.created >= before);
    assert!(runtime.created <= after);
    assert!(runtime.updated >= before);
    assert!(runtime.updated <= after);
    assert_eq!(runtime.created, runtime.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_changes_timestamp() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("timestamp_update");
    let input = fixture.create_input("ts");

    let runtime = RuntimeRepository::create(&pool, input)
        .await
        .expect("Failed to create runtime");

    let original_updated = runtime.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "runtime", runtime.id, original_updated).await;

    let update_input = UpdateRuntimeInput {
        description: Some(Patch::Set("Updated".to_string())),
        ..Default::default()
    };

    let updated = RuntimeRepository::update(&pool, runtime.id, update_input)
        .await
        .expect("Failed to update runtime");

    assert_eq!(updated.created, runtime.created);
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_ref_without_pack_id() {
    let pool = setup_db().await;
    let fixture = RuntimeFixture::new("pack_ref_only");
    let mut input = fixture.create_input("packref");
    input.pack = None;
    input.pack_ref = Some("some.pack.ref".to_string());

    let runtime = RuntimeRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create runtime");

    assert_eq!(runtime.pack, None);
    assert_eq!(runtime.pack_ref, input.pack_ref);
}
