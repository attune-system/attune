//! Integration tests for Worker repository
//!
//! Tests cover CRUD operations, specialized queries, constraints,
//! enum handling, timestamps, heartbeat updates, and edge cases.

use attune_common::models::enums::{WorkerStatus, WorkerType};
use attune_common::repositories::runtime::{
    CreateRuntimeInput, CreateWorkerInput, RuntimeRepository, UpdateWorkerInput, WorkerRepository,
};
use attune_common::repositories::{Create, Delete, FindById, List, Update};

use serde_json::json;
use sqlx::PgPool;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

mod helpers;
use helpers::create_test_pool;

// Global counter for unique IDs across all tests
static GLOBAL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test fixture for creating unique worker data
struct WorkerFixture {
    sequence: AtomicU64,
    test_id: String,
}

impl WorkerFixture {
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

    fn unique_name(&self, prefix: &str) -> String {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        format!("{}_{}_worker_{}", prefix, self.test_id, seq)
    }

    fn create_input(&self, name_suffix: &str, worker_type: WorkerType) -> CreateWorkerInput {
        CreateWorkerInput {
            name: self.unique_name(name_suffix),
            worker_type,
            runtime: None,
            host: Some("localhost".to_string()),
            port: Some(8080),
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({
                "cpu": "x86_64",
                "memory": "8GB",
                "python": ["3.9", "3.10", "3.11"],
                "node": ["16", "18", "20"]
            })),
            meta: Some(json!({
                "region": "us-west-2",
                "environment": "test"
            })),
        }
    }

    fn create_minimal_input(&self, name_suffix: &str) -> CreateWorkerInput {
        CreateWorkerInput {
            name: self.unique_name(name_suffix),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: None,
            meta: None,
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
async fn test_create_worker() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("create_worker");
    let input = fixture.create_input("basic", WorkerType::Local);

    let worker = WorkerRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create worker");

    assert!(worker.id > 0);
    assert_eq!(worker.name, input.name);
    assert_eq!(worker.worker_type, input.worker_type);
    assert_eq!(worker.runtime, input.runtime);
    assert_eq!(worker.host, input.host);
    assert_eq!(worker.port, input.port);
    assert_eq!(worker.status, input.status);
    assert_eq!(worker.capabilities, input.capabilities);
    assert_eq!(worker.meta, input.meta);
    assert_eq!(worker.last_heartbeat, None);
    assert!(worker.created > chrono::Utc::now() - chrono::Duration::seconds(5));
    assert!(worker.updated > chrono::Utc::now() - chrono::Duration::seconds(5));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_worker_minimal() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("create_worker_minimal");
    let input = fixture.create_minimal_input("minimal");

    let worker = WorkerRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create minimal worker");

    assert!(worker.id > 0);
    assert_eq!(worker.name, input.name);
    assert_eq!(worker.worker_type, WorkerType::Local);
    assert_eq!(worker.host, None);
    assert_eq!(worker.port, None);
    assert_eq!(worker.status, Some(WorkerStatus::Active));
    assert_eq!(worker.capabilities, None);
    assert_eq!(worker.meta, None);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_worker_by_id() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("find_by_id");
    let input = fixture.create_input("findable", WorkerType::Remote);

    let created = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    let found = WorkerRepository::find_by_id(&pool, created.id)
        .await
        .expect("Failed to find worker")
        .expect("Worker not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.name, created.name);
    assert_eq!(found.worker_type, created.worker_type);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_worker_by_id_not_found() {
    let pool = setup_db().await;

    let result = WorkerRepository::find_by_id(&pool, 999999999)
        .await
        .expect("Query should succeed");

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_worker_by_name() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("find_by_name");
    let input = fixture.create_input("nametest", WorkerType::Container);

    let created = WorkerRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create worker");

    let found = WorkerRepository::find_by_name(&pool, &input.name)
        .await
        .expect("Failed to find worker")
        .expect("Worker not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.name, created.name);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_worker_by_name_not_found() {
    let pool = setup_db().await;

    let result = WorkerRepository::find_by_name(&pool, "nonexistent_worker_999999")
        .await
        .expect("Query should succeed");

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_workers() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("list_workers");

    let input1 = fixture.create_input("list1", WorkerType::Local);
    let input2 = fixture.create_input("list2", WorkerType::Remote);

    let created1 = WorkerRepository::create(&pool, input1)
        .await
        .expect("Failed to create worker 1");
    let created2 = WorkerRepository::create(&pool, input2)
        .await
        .expect("Failed to create worker 2");

    let list = WorkerRepository::list(&pool)
        .await
        .expect("Failed to list workers");

    assert!(list.len() >= 2);
    assert!(list.iter().any(|w| w.id == created1.id));
    assert!(list.iter().any(|w| w.id == created2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_worker() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("update_worker");
    let input = fixture.create_input("update", WorkerType::Local);

    let created = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    let update_input = UpdateWorkerInput {
        name: Some("updated_worker_name".to_string()),
        status: Some(WorkerStatus::Busy),
        capabilities: Some(json!({
            "updated": true
        })),
        meta: Some(json!({
            "version": "2.0"
        })),
        host: Some("updated-host".to_string()),
        port: Some(9090),
    };

    let updated = WorkerRepository::update(&pool, created.id, update_input.clone())
        .await
        .expect("Failed to update worker");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.name, update_input.name.unwrap());
    assert_eq!(updated.status, update_input.status);
    assert_eq!(updated.capabilities, update_input.capabilities);
    assert_eq!(updated.meta, update_input.meta);
    assert_eq!(updated.host, update_input.host);
    assert_eq!(updated.port, update_input.port);
    assert!(updated.updated > created.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_worker_partial() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("update_partial");
    let input = fixture.create_input("partial", WorkerType::Remote);

    let created = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    let update_input = UpdateWorkerInput {
        status: Some(WorkerStatus::Inactive),
        name: None,
        capabilities: None,
        meta: None,
        host: None,
        port: None,
    };

    let updated = WorkerRepository::update(&pool, created.id, update_input.clone())
        .await
        .expect("Failed to update worker");

    assert_eq!(updated.status, update_input.status);
    assert_eq!(updated.name, created.name);
    assert_eq!(updated.capabilities, created.capabilities);
    assert_eq!(updated.meta, created.meta);
    assert_eq!(updated.host, created.host);
    assert_eq!(updated.port, created.port);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_worker_empty() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("update_empty");
    let input = fixture.create_input("empty", WorkerType::Container);

    let created = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    let update_input = UpdateWorkerInput::default();

    let result = WorkerRepository::update(&pool, created.id, update_input)
        .await
        .expect("Failed to update worker");

    // Should return existing entity unchanged
    assert_eq!(result.id, created.id);
    assert_eq!(result.name, created.name);
    assert_eq!(result.status, created.status);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_worker() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("delete_worker");
    let input = fixture.create_input("delete", WorkerType::Local);

    let created = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    let deleted = WorkerRepository::delete(&pool, created.id)
        .await
        .expect("Failed to delete worker");

    assert!(deleted);

    let found = WorkerRepository::find_by_id(&pool, created.id)
        .await
        .expect("Query should succeed");

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_worker_not_found() {
    let pool = setup_db().await;

    let deleted = WorkerRepository::delete(&pool, 999999999)
        .await
        .expect("Delete should succeed");

    assert!(!deleted);
}

// ============================================================================
// Specialized Query Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_status_active() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("find_by_status_active");

    let mut input1 = fixture.create_input("active1", WorkerType::Local);
    input1.status = Some(WorkerStatus::Active);

    let mut input2 = fixture.create_input("active2", WorkerType::Remote);
    input2.status = Some(WorkerStatus::Active);

    let mut input3 = fixture.create_input("busy", WorkerType::Container);
    input3.status = Some(WorkerStatus::Busy);

    let created1 = WorkerRepository::create(&pool, input1)
        .await
        .expect("Failed to create active worker 1");
    let created2 = WorkerRepository::create(&pool, input2)
        .await
        .expect("Failed to create active worker 2");
    let _created3 = WorkerRepository::create(&pool, input3)
        .await
        .expect("Failed to create busy worker");

    let active_workers = WorkerRepository::find_by_status(&pool, WorkerStatus::Active)
        .await
        .expect("Failed to find by status");

    assert!(active_workers.iter().any(|w| w.id == created1.id));
    assert!(active_workers.iter().any(|w| w.id == created2.id));
    assert!(active_workers
        .iter()
        .all(|w| w.status == Some(WorkerStatus::Active)));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_status_all_statuses() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("find_by_status_all");

    let statuses = vec![
        WorkerStatus::Active,
        WorkerStatus::Inactive,
        WorkerStatus::Busy,
        WorkerStatus::Error,
    ];

    for status in &statuses {
        let mut input = fixture.create_input(&format!("{:?}", status), WorkerType::Local);
        input.status = Some(*status);

        let created = WorkerRepository::create(&pool, input)
            .await
            .expect("Failed to create worker");

        let found = WorkerRepository::find_by_status(&pool, *status)
            .await
            .expect("Failed to find by status");

        assert!(found.iter().any(|w| w.id == created.id));
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_type_local() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("find_by_type_local");

    let input1 = fixture.create_input("local1", WorkerType::Local);
    let input2 = fixture.create_input("local2", WorkerType::Local);
    let input3 = fixture.create_input("remote", WorkerType::Remote);

    let created1 = WorkerRepository::create(&pool, input1)
        .await
        .expect("Failed to create local worker 1");
    let created2 = WorkerRepository::create(&pool, input2)
        .await
        .expect("Failed to create local worker 2");
    let _created3 = WorkerRepository::create(&pool, input3)
        .await
        .expect("Failed to create remote worker");

    let local_workers = WorkerRepository::find_by_type(&pool, WorkerType::Local)
        .await
        .expect("Failed to find by type");

    assert!(local_workers.iter().any(|w| w.id == created1.id));
    assert!(local_workers.iter().any(|w| w.id == created2.id));
    assert!(local_workers
        .iter()
        .all(|w| w.worker_type == WorkerType::Local));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_type_all_types() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("find_by_type_all");

    let types = vec![WorkerType::Local, WorkerType::Remote, WorkerType::Container];

    for worker_type in &types {
        let input = fixture.create_input(&format!("{:?}", worker_type), *worker_type);

        let created = WorkerRepository::create(&pool, input)
            .await
            .expect("Failed to create worker");

        let found = WorkerRepository::find_by_type(&pool, *worker_type)
            .await
            .expect("Failed to find by type");

        assert!(found.iter().any(|w| w.id == created.id));
        assert!(found.iter().all(|w| w.worker_type == *worker_type));
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_heartbeat() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("update_heartbeat");
    let input = fixture.create_input("heartbeat", WorkerType::Local);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.last_heartbeat, None);

    WorkerRepository::update(
        &pool,
        worker.id,
        UpdateWorkerInput {
            status: Some(WorkerStatus::Inactive),
            ..Default::default()
        },
    )
    .await
    .expect("Failed to mark worker inactive");

    let before = chrono::Utc::now();
    WorkerRepository::update_heartbeat(&pool, worker.id)
        .await
        .expect("Failed to update heartbeat");
    let after = chrono::Utc::now();

    let updated = WorkerRepository::find_by_id(&pool, worker.id)
        .await
        .expect("Failed to find worker")
        .expect("Worker not found");

    assert!(updated.last_heartbeat.is_some());
    assert_eq!(updated.status, Some(WorkerStatus::Active));
    let heartbeat = updated.last_heartbeat.unwrap();
    assert!(heartbeat >= before);
    assert!(heartbeat <= after);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_heartbeat_multiple_times() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("heartbeat_multiple");
    let input = fixture.create_input("multi", WorkerType::Remote);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    WorkerRepository::update_heartbeat(&pool, worker.id)
        .await
        .expect("Failed to update heartbeat 1");

    let first = WorkerRepository::find_by_id(&pool, worker.id)
        .await
        .expect("Failed to find worker")
        .expect("Worker not found")
        .last_heartbeat
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    WorkerRepository::update_heartbeat(&pool, worker.id)
        .await
        .expect("Failed to update heartbeat 2");

    let second = WorkerRepository::find_by_id(&pool, worker.id)
        .await
        .expect("Failed to find worker")
        .expect("Worker not found")
        .last_heartbeat
        .unwrap();

    assert!(second > first);
}

// ============================================================================
// Runtime Association Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_with_runtime() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("with_runtime");

    // Create a runtime first
    let runtime_input = CreateRuntimeInput {
        r#ref: format!("{}.test_runtime", fixture.test_id),
        pack: None,
        pack_ref: None,
        description: Some("Test runtime".to_string()),
        name: "test_runtime".to_string(),
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
    };

    let runtime = RuntimeRepository::create(&pool, runtime_input)
        .await
        .expect("Failed to create runtime");

    // Create worker with runtime association
    let mut input = fixture.create_input("with_rt", WorkerType::Local);
    input.runtime = Some(runtime.id);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.runtime, Some(runtime.id));

    let found = WorkerRepository::find_by_id(&pool, worker.id)
        .await
        .expect("Failed to find worker")
        .expect("Worker not found");

    assert_eq!(found.runtime, Some(runtime.id));
}

// ============================================================================
// Enum Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_type_local() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("type_local");
    let input = fixture.create_input("local", WorkerType::Local);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.worker_type, WorkerType::Local);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_type_remote() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("type_remote");
    let input = fixture.create_input("remote", WorkerType::Remote);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.worker_type, WorkerType::Remote);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_type_container() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("type_container");
    let input = fixture.create_input("container", WorkerType::Container);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.worker_type, WorkerType::Container);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_status_active() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("status_active");
    let mut input = fixture.create_input("active", WorkerType::Local);
    input.status = Some(WorkerStatus::Active);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.status, Some(WorkerStatus::Active));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_status_inactive() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("status_inactive");
    let mut input = fixture.create_input("inactive", WorkerType::Local);
    input.status = Some(WorkerStatus::Inactive);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.status, Some(WorkerStatus::Inactive));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_status_busy() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("status_busy");
    let mut input = fixture.create_input("busy", WorkerType::Local);
    input.status = Some(WorkerStatus::Busy);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.status, Some(WorkerStatus::Busy));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_status_error() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("status_error");
    let mut input = fixture.create_input("error", WorkerType::Local);
    input.status = Some(WorkerStatus::Error);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.status, Some(WorkerStatus::Error));
}

// ============================================================================
// Edge Cases and Constraints
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_duplicate_name_allowed() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("duplicate_name");

    // Use a fixed name for both workers
    let name = format!("{}_duplicate", fixture.test_id);

    let mut input1 = fixture.create_input("dup1", WorkerType::Local);
    input1.name = name.clone();

    let mut input2 = fixture.create_input("dup2", WorkerType::Remote);
    input2.name = name.clone();

    let worker1 = WorkerRepository::create(&pool, input1)
        .await
        .expect("Failed to create first worker");

    let duplicate = WorkerRepository::create(&pool, input2).await;

    assert!(duplicate.is_err());
    assert_eq!(worker1.name, name);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_json_fields() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("json_fields");
    let input = fixture.create_input("json", WorkerType::Container);

    let worker = WorkerRepository::create(&pool, input.clone())
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.capabilities, input.capabilities);
    assert_eq!(worker.meta, input.meta);

    // Verify JSON structure
    let caps = worker.capabilities.unwrap();
    assert_eq!(caps["cpu"], json!("x86_64"));
    assert_eq!(caps["memory"], json!("8GB"));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_null_json_fields() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("null_json");
    let input = fixture.create_minimal_input("nulljson");

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.capabilities, None);
    assert_eq!(worker.meta, None);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_null_status() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("null_status");
    let mut input = fixture.create_input("nostatus", WorkerType::Local);
    input.status = None;

    let result = WorkerRepository::create(&pool, input).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_ordering() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("list_ordering");

    let mut input1 = fixture.create_input("z", WorkerType::Local);
    input1.name = format!("{}_zzz_worker", fixture.test_id);

    let mut input2 = fixture.create_input("a", WorkerType::Remote);
    input2.name = format!("{}_aaa_worker", fixture.test_id);

    let mut input3 = fixture.create_input("m", WorkerType::Container);
    input3.name = format!("{}_mmm_worker", fixture.test_id);

    WorkerRepository::create(&pool, input1)
        .await
        .expect("Failed to create worker 1");
    WorkerRepository::create(&pool, input2)
        .await
        .expect("Failed to create worker 2");
    WorkerRepository::create(&pool, input3)
        .await
        .expect("Failed to create worker 3");

    let list = WorkerRepository::list(&pool)
        .await
        .expect("Failed to list workers");

    // Find our test workers in the list
    let test_workers: Vec<_> = list
        .iter()
        .filter(|w| w.name.contains(&fixture.test_id))
        .collect();

    assert_eq!(test_workers.len(), 3);

    // Verify they are sorted by name
    for i in 0..test_workers.len() - 1 {
        assert!(test_workers[i].name <= test_workers[i + 1].name);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_timestamps() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("timestamps");
    let input = fixture.create_input("time", WorkerType::Local);

    let before = chrono::Utc::now();
    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");
    let after = chrono::Utc::now();

    assert!(worker.created >= before);
    assert!(worker.created <= after);
    assert!(worker.updated >= before);
    assert!(worker.updated <= after);
    assert_eq!(worker.created, worker.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_changes_timestamp() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("timestamp_update");
    let input = fixture.create_input("ts", WorkerType::Remote);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let update_input = UpdateWorkerInput {
        status: Some(WorkerStatus::Busy),
        ..Default::default()
    };

    let updated = WorkerRepository::update(&pool, worker.id, update_input)
        .await
        .expect("Failed to update worker");

    assert_eq!(updated.created, worker.created);
    assert!(updated.updated > worker.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_heartbeat_updates_timestamp() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("heartbeat_updates");
    let input = fixture.create_input("hb", WorkerType::Container);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    let original_updated = worker.updated;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    WorkerRepository::update_heartbeat(&pool, worker.id)
        .await
        .expect("Failed to update heartbeat");

    let after_heartbeat = WorkerRepository::find_by_id(&pool, worker.id)
        .await
        .expect("Failed to find worker")
        .expect("Worker not found");

    // Heartbeat should update both last_heartbeat and updated timestamp (due to trigger)
    assert!(after_heartbeat.last_heartbeat.is_some());
    assert!(after_heartbeat.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_port_range() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("port_range");

    // Test various port numbers
    let ports = vec![1, 80, 443, 8080, 65535];

    for port in ports {
        let mut input = fixture.create_input(&format!("port{}", port), WorkerType::Local);
        input.port = Some(port);

        let worker = WorkerRepository::create(&pool, input)
            .await
            .unwrap_or_else(|_| panic!("Failed to create worker with port {}", port));

        assert_eq!(worker.port, Some(port));
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_status_lifecycle() {
    let pool = setup_db().await;
    let fixture = WorkerFixture::new("status_lifecycle");
    let mut input = fixture.create_input("lifecycle", WorkerType::Local);
    input.status = Some(WorkerStatus::Inactive);

    let worker = WorkerRepository::create(&pool, input)
        .await
        .expect("Failed to create worker");

    assert_eq!(worker.status, Some(WorkerStatus::Inactive));

    // Transition to Active
    let update1 = UpdateWorkerInput {
        status: Some(WorkerStatus::Active),
        ..Default::default()
    };
    let worker = WorkerRepository::update(&pool, worker.id, update1)
        .await
        .expect("Failed to update to Active");
    assert_eq!(worker.status, Some(WorkerStatus::Active));

    // Transition to Busy
    let update2 = UpdateWorkerInput {
        status: Some(WorkerStatus::Busy),
        ..Default::default()
    };
    let worker = WorkerRepository::update(&pool, worker.id, update2)
        .await
        .expect("Failed to update to Busy");
    assert_eq!(worker.status, Some(WorkerStatus::Busy));

    // Transition to Error
    let update3 = UpdateWorkerInput {
        status: Some(WorkerStatus::Error),
        ..Default::default()
    };
    let worker = WorkerRepository::update(&pool, worker.id, update3)
        .await
        .expect("Failed to update to Error");
    assert_eq!(worker.status, Some(WorkerStatus::Error));

    // Back to Inactive
    let update4 = UpdateWorkerInput {
        status: Some(WorkerStatus::Inactive),
        ..Default::default()
    };
    let worker = WorkerRepository::update(&pool, worker.id, update4)
        .await
        .expect("Failed to update back to Inactive");
    assert_eq!(worker.status, Some(WorkerStatus::Inactive));
}
