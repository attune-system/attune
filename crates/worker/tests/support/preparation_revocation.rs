use super::*;
use attune_common::{
    artifact_transport::VolumeTransport,
    auth::jwt::JwtConfig,
    log_stream::SegmentedLogConfig,
    models::{WorkerStatus, WorkerType},
    repositories::{
        delegated_execution::DelegatedExecutionRepository as Delegated,
        runtime::{CreateWorkerInput, WorkerRepository},
        runtime_version::{CreateRuntimeVersionInput, RuntimeVersionRepository},
        FindById,
    },
};
use attune_worker::{
    artifacts::ArtifactManager,
    runtime::{process::ProcessRuntime, RuntimeRegistry},
    ActionExecutor,
};
use helpers::RuntimeFixture;
use std::{sync::Arc, time::Duration};

async fn wait_for_blocked_query(
    pool: &sqlx::PgPool,
    blocker: i32,
    fragment: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tick.tick().await;
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
                 AND $1=ANY(pg_blocking_pids(pid)) AND position($2 in query)>0)",
            )
            .bind(blocker)
            .bind(fragment)
            .fetch_one(pool)
            .await?;
            if blocked {
                return Ok::<_, sqlx::Error>(());
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("preparation never blocked on {fragment}"))??;
    Ok(())
}

async fn launch_after_worker_lookup(revoke: bool) {
    let db = create_test_pool().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("child-ran");
    let packs = directory.path().join("packs");
    let artifacts = directory.path().join("artifacts");
    let envs = directory.path().join("environments");
    let pack = PackFixture::new_unique("launch_fence")
        .create(&db)
        .await
        .unwrap();
    let owner = IdentityFixture::new_unique("launch_owner")
        .create(&db)
        .await
        .unwrap();
    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "launch")
        .create(&db)
        .await
        .unwrap();
    let version = RuntimeVersionRepository::create(
        &db,
        CreateRuntimeVersionInput {
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            version: "1.0.0".into(),
            version_major: Some(1),
            version_minor: Some(0),
            version_patch: Some(0),
            execution_config: runtime.execution_config.clone(),
            distributions: json!({}),
            is_default: true,
            available: true,
            meta: json!({}),
        },
    )
    .await
    .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "silent")
        .with_runtime(runtime.id)
        .with_entrypoint("silent.sh")
        .with_param_schema(json!({}))
        .create(&db)
        .await
        .unwrap();
    let worker = WorkerRepository::create(
        &db,
        CreateWorkerInput {
            name: "launch-fence-worker".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await
    .unwrap();
    let digest = "d".repeat(64);
    let pack_dir =
        attune_common::pack_transport::release_cache_path(&packs, &pack.r#ref, &digest).unwrap();
    std::fs::create_dir_all(pack_dir.join("actions")).unwrap();
    std::fs::write(
        pack_dir.join("actions/silent.sh"),
        format!(
            "IFS= read -r input || true\ntest -z \"${{ATTUNE_API_TOKEN+x}}\" || exit 42\nprintf ran > '{}'\n",
            marker.display(),
        ),
    )
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest,
            object_key: "owned/launch.tar.gz".into(),
            provider_version: "e:launch".into(),
            content_path: pack_dir.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            runtimes: vec![runtime.id],
            runtime_versions: vec![version.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id)
        .await
        .unwrap();
    assert_eq!(snapshot.executable.runtime_versions.len(), 1);
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(owner.id),
            config: Some(json!({})),
            timeout_seconds: Some(30),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await
        .unwrap();
    Delegated::select(&mut tx, execution.id, None)
        .await
        .unwrap();
    // Admission precedes assignment, just as in production dispatch.
    ExecutionRepository::update(
        &mut *tx,
        execution.id,
        UpdateExecutionInput {
            worker: Some(worker.id),
            status: Some(ExecutionStatus::Scheduled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut registry = RuntimeRegistry::new();
    registry.register(Box::new(ProcessRuntime::new(
        runtime.name.to_lowercase(),
        serde_json::from_value(runtime.execution_config).unwrap(),
        packs.clone(),
        envs.clone(),
    )));
    let executor = Arc::new(ActionExecutor::new(
        db.pool().clone(),
        registry,
        ArtifactManager::new(artifacts.clone()),
        SecretManager::new(db.pool().clone(), Some(ENCRYPTION_KEY.into())).unwrap(),
        1024,
        1024,
        false,
        false,
        "launch-fence-worker".into(),
        uuid::Uuid::new_v4(),
        SegmentedLogConfig {
            initial_segment_bytes: 1024,
            max_segment_bytes: 1024,
            flush_interval_ms: 100,
            retry_max_attempts: 1,
            retry_attempt_timeout_ms: 100,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 1,
            finalization_timeout_ms: 100,
        },
        None,
        None,
        packs,
        artifacts.clone(),
        envs,
        "http://127.0.0.1:1".into(),
        JwtConfig::default(),
        Arc::new(VolumeTransport::new(artifacts.to_str().unwrap())),
    ));
    executor.set_worker_id(worker.id);

    // First gate is after claim/initial authorization, but before input validation.
    // It lets us lock worker only after claim, so the second gate is precisely the
    // intervening runtime-version worker lookup, not a startup/claim query.
    let mut input_gate = Some(db.begin().await.unwrap());
    sqlx::query("LOCK TABLE execution_secret_value IN ACCESS EXCLUSIVE MODE")
        .execute(&mut **input_gate.as_mut().unwrap())
        .await
        .unwrap();
    let input_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **input_gate.as_mut().unwrap())
        .await
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let child_executor = executor.clone();
    let child_cancel = cancel.clone();
    let mut task = tokio::spawn(async move {
        child_executor
            .execute_with_cancel(execution.id, child_cancel)
            .await
    });
    let mut worker_gate = None;
    let phase: anyhow::Result<()> = async {
        wait_for_blocked_query(db.pool(), input_pid, "FROM execution_secret_value").await?;
        let mut gate = db.begin().await?;
        sqlx::query("LOCK TABLE worker IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *gate)
            .await?;
        let worker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *gate)
            .await?;
        worker_gate = Some(gate);
        input_gate.take().unwrap().commit().await?;
        wait_for_blocked_query(db.pool(), worker_pid, "FROM worker WHERE id").await?;
        if revoke {
            IdentityRepository::update(
                &db,
                owner.id,
                UpdateIdentityInput {
                    frozen: Some(true),
                    ..Default::default()
                },
            )
            .await?;
        }
        worker_gate.take().unwrap().commit().await?;
        Ok(())
    }
    .await;
    // Release both owned gates and join the executor even when coordination fails.
    if let Some(gate) = worker_gate {
        gate.rollback().await.unwrap();
    }
    if let Some(gate) = input_gate {
        gate.rollback().await.unwrap();
    }
    if phase.is_err() {
        cancel.cancel();
    }
    let joined = match tokio::time::timeout(Duration::from_secs(40), &mut task).await {
        Ok(result) => result,
        Err(_) => {
            cancel.cancel();
            match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
                Ok(result) => result,
                Err(_) => {
                    task.abort();
                    task.await
                }
            }
        }
    };
    let ran = marker.exists();
    let stored = ExecutionRepository::find_by_id(&db, execution.id)
        .await
        .unwrap()
        .unwrap();
    drop(executor);
    db.cleanup().await.unwrap();
    directory.close().unwrap();
    phase.unwrap();
    let outcome = joined.unwrap();
    if revoke {
        assert!(outcome.is_err(), "revoked preparation must reject launch");
        assert!(
            !ran,
            "silent tokenless child performed an external effect after revocation"
        );
        assert_eq!(stored.status, ExecutionStatus::Failed);
    } else {
        assert!(
            outcome.unwrap().is_success(),
            "positive control must execute successfully"
        );
        assert!(
            ran,
            "positive control must prove that this fixture can launch the child"
        );
        assert_eq!(stored.status, ExecutionStatus::Completed);
    }
}

#[tokio::test]
async fn freeze_during_runtime_worker_lookup_prevents_silent_tokenless_child() {
    launch_after_worker_lookup(true).await;
}

#[tokio::test]
async fn active_owner_after_runtime_worker_lookup_launches_silent_tokenless_child() {
    launch_after_worker_lookup(false).await;
}
