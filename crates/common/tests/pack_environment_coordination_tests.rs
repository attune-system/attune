//! Integration tests for coordinated pack environment claims.

use attune_common::models::enums::{WorkerStatus, WorkerType};
use attune_common::pack_environment::{
    PackEnvironmentManager, PackEnvironmentStatus, UpsertCoordinatedEnvironmentInput,
};
use attune_common::repositories::runtime::{
    CreateRuntimeInput, CreateWorkerInput, RuntimeRepository, WorkerRepository,
};
use attune_common::repositories::runtime_version::{
    CreateRuntimeVersionInput, RuntimeVersionRepository,
};
use attune_common::repositories::{Create, FindById};
use serde_json::json;
use tempfile::TempDir;

mod helpers;
use helpers::{create_test_pool, PackFixture};

async fn setup_runtime_fixture() -> (
    attune_common::test_database::TestDatabase,
    TempDir,
    attune_common::models::Pack,
    attune_common::models::Runtime,
    attune_common::models::RuntimeVersion,
    attune_common::models::Worker,
    attune_common::models::Worker,
) {
    let pool = create_test_pool()
        .await
        .expect("Failed to create test pool");
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let pack = PackFixture::new_unique("pack_env_coordination")
        .create(&pool)
        .await
        .expect("Failed to create pack");

    let runtime = RuntimeRepository::create(
        &pool,
        CreateRuntimeInput {
            r#ref: format!("{}.python", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            description: Some("Python runtime".to_string()),
            name: "Python".to_string(),
            aliases: vec!["python".to_string()],
            distributions: json!({}),
            installation: None,
            execution_config: json!({
                "interpreter": {
                    "binary": "python3",
                    "args": ["-u"],
                    "file_extension": ".py"
                },
                "environment": {
                    "env_type": "virtualenv",
                    "dir_name": ".venv",
                    "create_command": ["python3", "-m", "venv", "{env_dir}"],
                    "interpreter_path": "{env_dir}/bin/python3"
                },
                "dependencies": {
                    "manifest_file": "requirements.txt",
                    "install_command": ["{interpreter}", "-m", "pip", "install", "-r", "{manifest_path}"]
                }
            }),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await
    .expect("Failed to create runtime");

    let runtime_version = RuntimeVersionRepository::create(
        &pool,
        CreateRuntimeVersionInput {
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            version: "3.12.0".to_string(),
            version_major: Some(3),
            version_minor: Some(12),
            version_patch: Some(0),
            execution_config: runtime.execution_config.clone(),
            distributions: json!({}),
            is_default: true,
            available: true,
            meta: json!({}),
        },
    )
    .await
    .expect("Failed to create runtime version");

    let worker_a = WorkerRepository::create(
        &pool,
        CreateWorkerInput {
            name: format!("worker-a-{}", pack.r#ref),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await
    .expect("Failed to create worker A");

    let worker_b = WorkerRepository::create(
        &pool,
        CreateWorkerInput {
            name: format!("worker-b-{}", pack.r#ref),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await
    .expect("Failed to create worker B");

    (
        pool,
        temp_dir,
        pack,
        runtime,
        runtime_version,
        worker_a,
        worker_b,
    )
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn coordinated_environment_claims_single_owner_and_retrys_after_failure() {
    let (pool, temp_dir, pack, runtime, runtime_version, worker_a, worker_b) =
        setup_runtime_fixture().await;
    let manager =
        PackEnvironmentManager::with_base_path(pool.clone(), temp_dir.path().to_path_buf());
    let env_path = temp_dir.path().join("runtime_envs").join("python-3.12");

    let target = manager
        .upsert_coordinated_environment(UpsertCoordinatedEnvironmentInput {
            pack_id: pack.id,
            pack_ref: &pack.r#ref,
            runtime_id: runtime.id,
            runtime_ref: &runtime.r#ref,
            runtime_version: Some(&runtime_version),
            env_path: &env_path,
            manifest_checksum: Some("checksum-a"),
        })
        .await
        .expect("Failed to create coordinated environment");

    let first_claim = manager
        .claim_coordinated_environment(&target.env_key, worker_a.id, 300)
        .await
        .expect("First claim query failed");
    assert_eq!(
        first_claim.as_ref().and_then(|row| row.claimed_by_worker),
        Some(worker_a.id)
    );

    let second_claim = manager
        .claim_coordinated_environment(&target.env_key, worker_b.id, 300)
        .await
        .expect("Second claim query failed");
    assert!(
        second_claim.is_none(),
        "second worker should not steal an active claim"
    );

    assert!(manager
        .mark_coordinated_environment_failed(&target.env_key, worker_a.id, "install failed")
        .await
        .expect("Failed to mark coordinated environment as failed"));

    let retried_claim = manager
        .claim_coordinated_environment(&target.env_key, worker_b.id, 300)
        .await
        .expect("Retry claim query failed")
        .expect("second worker should be able to claim after failure");
    assert_eq!(retried_claim.claimed_by_worker, Some(worker_b.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn coordinated_environment_becomes_outdated_when_manifest_checksum_changes() {
    let (pool, temp_dir, pack, runtime, runtime_version, worker_a, _worker_b) =
        setup_runtime_fixture().await;
    let manager =
        PackEnvironmentManager::with_base_path(pool.clone(), temp_dir.path().to_path_buf());
    let env_path = temp_dir.path().join("runtime_envs").join("python-3.12");

    let target = manager
        .upsert_coordinated_environment(UpsertCoordinatedEnvironmentInput {
            pack_id: pack.id,
            pack_ref: &pack.r#ref,
            runtime_id: runtime.id,
            runtime_ref: &runtime.r#ref,
            runtime_version: Some(&runtime_version),
            env_path: &env_path,
            manifest_checksum: Some("checksum-a"),
        })
        .await
        .expect("Failed to create coordinated environment");

    manager
        .claim_coordinated_environment(&target.env_key, worker_a.id, 300)
        .await
        .expect("Claim query failed")
        .expect("worker should claim target");

    assert!(manager
        .mark_coordinated_environment_ready(&target.env_key, worker_a.id, Some("checksum-a"))
        .await
        .expect("Failed to mark coordinated environment ready"));

    let updated = manager
        .upsert_coordinated_environment(UpsertCoordinatedEnvironmentInput {
            pack_id: pack.id,
            pack_ref: &pack.r#ref,
            runtime_id: runtime.id,
            runtime_ref: &runtime.r#ref,
            runtime_version: Some(&runtime_version),
            env_path: &env_path,
            manifest_checksum: Some("checksum-b"),
        })
        .await
        .expect("Failed to update coordinated environment");

    assert_eq!(updated.status, PackEnvironmentStatus::Outdated);
    assert_eq!(updated.manifest_checksum.as_deref(), Some("checksum-b"));

    let fetched = manager
        .get_coordinated_environment(&updated.env_key)
        .await
        .expect("Failed to fetch updated coordinated environment")
        .expect("coordinated environment should exist");
    assert_eq!(fetched.status, PackEnvironmentStatus::Outdated);
    assert_eq!(fetched.manifest_checksum.as_deref(), Some("checksum-b"));

    let runtime_version_id = fetched
        .runtime_version
        .expect("runtime version should be stored");
    let stored_version = RuntimeVersionRepository::find_by_id(&pool, runtime_version_id)
        .await
        .expect("Failed to load runtime version")
        .expect("runtime version should exist");
    assert_eq!(stored_version.version, "3.12.0");
}
