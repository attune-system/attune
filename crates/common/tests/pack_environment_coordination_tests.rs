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
async fn coordinated_environment_claims_single_owner_and_retrys_after_failure() {
    let (pool, temp_dir, pack, runtime, runtime_version, worker_a, worker_b) =
        setup_runtime_fixture().await;
    let manager = PackEnvironmentManager::new(pool.clone());
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
        .claim_coordinated_environment(&target, worker_a.id, 300)
        .await
        .expect("First claim query failed");
    assert_eq!(
        first_claim.as_ref().and_then(|row| row.claimed_by_worker),
        Some(worker_a.id)
    );

    let second_claim = manager
        .claim_coordinated_environment(&target, worker_b.id, 300)
        .await
        .expect("Second claim query failed");
    assert!(
        second_claim.is_none(),
        "second worker should not steal an active claim"
    );

    assert!(manager
        .mark_coordinated_environment_failed(
            &target.env_key,
            worker_a.id,
            first_claim.as_ref().unwrap().claim_id.unwrap(),
            "install failed"
        )
        .await
        .expect("Failed to mark coordinated environment as failed"));

    let retried_claim = manager
        .claim_coordinated_environment(&target, worker_b.id, 300)
        .await
        .expect("Retry claim query failed")
        .expect("second worker should be able to claim after failure");
    assert_eq!(retried_claim.claimed_by_worker, Some(worker_b.id));
    drop(manager);
    pool.cleanup()
        .await
        .expect("owned environment fixture cleanup");
}

#[tokio::test]
async fn coordinated_environment_becomes_outdated_when_manifest_checksum_changes() {
    let (pool, temp_dir, pack, runtime, runtime_version, worker_a, _worker_b) =
        setup_runtime_fixture().await;
    let manager = PackEnvironmentManager::new(pool.clone());
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

    let claim = manager
        .claim_coordinated_environment(&target, worker_a.id, 300)
        .await
        .expect("Claim query failed")
        .expect("worker should claim target");

    assert!(manager
        .mark_coordinated_environment_ready(
            &target.env_key,
            worker_a.id,
            claim.claim_id.unwrap(),
            Some("checksum-a")
        )
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
    drop(manager);
    pool.cleanup()
        .await
        .expect("owned environment fixture cleanup");
}

#[tokio::test]
async fn expired_claim_cannot_finish_or_renew_a_new_same_worker_claim() {
    let (pool, temp_dir, pack, runtime, version, worker, _) = setup_runtime_fixture().await;
    let manager = PackEnvironmentManager::new(pool.clone());
    let path = temp_dir.path().join("native-runtime");
    let target = manager
        .upsert_coordinated_environment(UpsertCoordinatedEnvironmentInput {
            pack_id: pack.id,
            pack_ref: &pack.r#ref,
            runtime_id: runtime.id,
            runtime_ref: &runtime.r#ref,
            runtime_version: Some(&version),
            env_path: &path,
            manifest_checksum: Some("immutable-source"),
        })
        .await
        .unwrap();
    let first = manager
        .claim_coordinated_environment(&target, worker.id, 300)
        .await
        .unwrap()
        .unwrap();
    let old = first.claim_id.unwrap();
    sqlx::query("UPDATE pack_environment SET claim_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1")
        .bind(target.id).execute(&pool).await.unwrap();
    assert!(!manager
        .renew_coordinated_environment_claim(&target.env_key, worker.id, old, 300)
        .await
        .unwrap());
    assert!(!manager
        .mark_coordinated_environment_ready(
            &target.env_key,
            worker.id,
            old,
            Some("immutable-source")
        )
        .await
        .unwrap());
    let next = manager
        .claim_coordinated_environment(&target, worker.id, 300)
        .await
        .unwrap()
        .unwrap();
    let current = next.claim_id.unwrap();
    assert_ne!(old, current);
    assert!(!manager
        .renew_coordinated_environment_claim(&target.env_key, worker.id, old, 300)
        .await
        .unwrap());
    assert!(!manager
        .mark_coordinated_environment_ready(
            &target.env_key,
            worker.id,
            old,
            Some("immutable-source")
        )
        .await
        .unwrap());
    assert!(!manager
        .mark_coordinated_environment_failed(&target.env_key, worker.id, old, "late old failure")
        .await
        .unwrap());
    assert!(manager
        .renew_coordinated_environment_claim(&target.env_key, worker.id, current, 300)
        .await
        .unwrap());
    assert!(manager
        .mark_coordinated_environment_ready(
            &target.env_key,
            worker.id,
            current,
            Some("immutable-source")
        )
        .await
        .unwrap());
    drop(manager);
    pool.cleanup().await.expect("owned claim fixture cleanup");
}

#[tokio::test]
async fn source_replacement_and_restoration_never_restore_the_old_install_claim() {
    let (pool, temp_dir, pack, runtime, version, worker, _) = setup_runtime_fixture().await;
    let manager = PackEnvironmentManager::new(pool.clone());
    let path = temp_dir.path().join("native-runtime");
    let upsert = |checksum| UpsertCoordinatedEnvironmentInput {
        pack_id: pack.id,
        pack_ref: pack.r#ref.as_str(),
        runtime_id: runtime.id,
        runtime_ref: runtime.r#ref.as_str(),
        runtime_version: Some(&version),
        env_path: path.as_path(),
        manifest_checksum: Some(checksum),
    };
    let original = manager
        .upsert_coordinated_environment(upsert("source-a"))
        .await
        .unwrap();
    let first = manager
        .claim_coordinated_environment(&original, worker.id, 300)
        .await
        .unwrap()
        .unwrap();
    let old = first.claim_id.unwrap();
    let changed = manager
        .upsert_coordinated_environment(upsert("source-b"))
        .await
        .unwrap();
    assert_eq!(changed.status, PackEnvironmentStatus::Outdated);
    assert!(changed.claimed_by_worker.is_none() && changed.claim_id.is_none());
    assert!(manager
        .claim_coordinated_environment(&original, worker.id, 300)
        .await
        .unwrap()
        .is_none());
    let restored = manager
        .upsert_coordinated_environment(upsert("source-a"))
        .await
        .unwrap();
    assert!(!manager
        .renew_coordinated_environment_claim(&restored.env_key, worker.id, old, 300)
        .await
        .unwrap());
    assert!(!manager
        .mark_coordinated_environment_ready(&restored.env_key, worker.id, old, Some("source-a"))
        .await
        .unwrap());
    let current = manager
        .claim_coordinated_environment(&restored, worker.id, 300)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(current.claim_id, Some(old));
    assert!(manager
        .mark_coordinated_environment_ready(
            &restored.env_key,
            worker.id,
            current.claim_id.unwrap(),
            Some("source-a")
        )
        .await
        .unwrap());
    drop(manager);
    pool.cleanup()
        .await
        .expect("owned source replacement fixture cleanup");
}

#[tokio::test]
async fn expiry_while_waiting_for_target_rejects_both_renewal_and_ready(
) -> Result<(), Box<dyn std::error::Error>> {
    for renew in [true, false] {
        let (pool, temp_dir, pack, runtime, version, worker, _) = setup_runtime_fixture().await;
        let manager = PackEnvironmentManager::new(pool.clone());
        let path = temp_dir.path().join("native-runtime");
        let target = manager
            .upsert_coordinated_environment(UpsertCoordinatedEnvironmentInput {
                pack_id: pack.id,
                pack_ref: &pack.r#ref,
                runtime_id: runtime.id,
                runtime_ref: &runtime.r#ref,
                runtime_version: Some(&version),
                env_path: &path,
                manifest_checksum: Some("source"),
            })
            .await?;
        let claimed = manager
            .claim_coordinated_environment(&target, worker.id, 300)
            .await?
            .unwrap();
        let claim = claimed.claim_id.unwrap();
        let deadline: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "UPDATE pack_environment SET claim_expires_at=clock_timestamp()+interval '3 seconds' WHERE id=$1 RETURNING claim_expires_at"
        ).bind(target.id).fetch_one(&pool).await?;
        // Hold the existing tuple without updating it. A clock predicate that
        // ran before this wait would otherwise still authorize the old tuple.
        let mut barrier = pool.begin().await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let _: i64 = sqlx::query_scalar("SELECT id FROM pack_environment WHERE id=$1 FOR UPDATE")
            .bind(target.id)
            .fetch_one(&mut *barrier)
            .await?;
        let actor = manager.clone();
        let key = target.env_key.clone();
        let mut call = tokio::spawn(async move {
            if renew {
                actor
                    .renew_coordinated_environment_claim(&key, worker.id, claim, 300)
                    .await
            } else {
                actor
                    .mark_coordinated_environment_ready(&key, worker.id, claim, Some("source"))
                    .await
            }
        });
        let mut barrier = Some(barrier);
        let result = async {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))")
                        .bind(blocker).fetch_one(&pool).await?;
                    if waiting { return Ok::<_, sqlx::Error>(()); }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await??;
            assert!(!call.is_finished());
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>=$1").bind(deadline).fetch_one(&pool).await?;
                    if expired { return Ok::<_, sqlx::Error>(()); }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await??;
            barrier.take().unwrap().commit().await?;
            assert!(!(&mut call).await??);
            let retained = manager.get_coordinated_environment(&target.env_key).await?.unwrap();
            assert_eq!(retained.status, PackEnvironmentStatus::Installing);
            assert_eq!(retained.claim_id, Some(claim));
            assert_eq!(retained.claim_expires_at, Some(deadline));
            Ok::<_, Box<dyn std::error::Error>>(())
        }.await;
        if let Some(barrier) = barrier {
            barrier.rollback().await?;
        }
        if !call.is_finished() {
            call.abort();
            let _ = call.await;
        }
        drop(manager);
        pool.cleanup().await?;
        result?;
    }
    Ok(())
}

#[tokio::test]
async fn deleting_a_worker_retains_no_authority_and_does_not_block_a_new_claim() {
    let (pool, temp_dir, pack, runtime, version, worker, replacement) =
        setup_runtime_fixture().await;
    let manager = PackEnvironmentManager::new(pool.clone());
    let path = temp_dir.path().join("native-runtime");
    let target = manager
        .upsert_coordinated_environment(UpsertCoordinatedEnvironmentInput {
            pack_id: pack.id,
            pack_ref: &pack.r#ref,
            runtime_id: runtime.id,
            runtime_ref: &runtime.r#ref,
            runtime_version: Some(&version),
            env_path: &path,
            manifest_checksum: Some("source"),
        })
        .await
        .unwrap();
    let old = manager
        .claim_coordinated_environment(&target, worker.id, 300)
        .await
        .unwrap()
        .unwrap()
        .claim_id
        .unwrap();
    use attune_common::repositories::Delete;
    WorkerRepository::delete(&pool, worker.id).await.unwrap();
    assert!(!manager
        .mark_coordinated_environment_ready(&target.env_key, worker.id, old, Some("source"))
        .await
        .unwrap());
    let next = manager
        .claim_coordinated_environment(&target, replacement.id, 300)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(next.claim_id, Some(old));
    assert_eq!(next.claimed_by_worker, Some(replacement.id));
    drop(manager);
    pool.cleanup()
        .await
        .expect("owned deleted-worker fixture cleanup");
}
