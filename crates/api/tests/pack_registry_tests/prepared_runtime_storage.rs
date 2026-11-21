use super::*;
use attune_common::{
    auth::WorkerTokenProvider,
    blob_store::S3BlobStore,
    child_process_environment::ChildProcessEnvironment,
    config::BlobStorageConfig,
    models::{WorkerRole, WorkerStatus, WorkerType},
    pack_transport::{ApiPackTransport, PackFileTransport},
    repositories::{
        runtime::CreateWorkerInput,
        runtime_preparation::{PreparationJob, RuntimePreparationRepository},
        WorkerRepository,
    },
    runtime_lifecycle::{
        PreparationScope, PreparationTarget, PreparationWorkload, RuntimeRecipe,
        PREPARATION_TRIGGER,
    },
    workload_sandbox::WorkloadSandbox,
};
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires PostgreSQL, the owned versioned S3 harness, and Linux Bubblewrap"]
async fn prepared_runtime_moves_from_builder_to_sensor_via_versioned_s3() -> Result<()> {
    let mut context = TestContext::new().await?.with_admin_auth().await?;
    let result = exercise_transfer(&mut context).await;
    context.cleanup().await?;
    result
}

async fn exercise_transfer(ctx: &mut TestContext) -> Result<()> {
    let bucket = std::env::var("ATTUNE_TEST_S3_BUCKET")?;
    let endpoint = std::env::var("ATTUNE_TEST_S3_ENDPOINT")?;
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into());
    let prefix = format!(
        "{}/prepared/{}",
        std::env::var("ATTUNE_TEST_S3_PREFIX")?,
        Uuid::new_v4()
    );
    let storage = BlobStorageConfig::S3 {
        prefix: prefix.clone(),
        bucket: bucket.clone(),
        region: region.clone(),
        endpoint: Some(endpoint.clone()),
        kms_key: None,
    };

    let pack = create_test_pack_dir("prepared-storage", "1.0.0")?;
    fs::create_dir(pack.path().join("sensors"))?;
    fs::write(
        pack.path().join("sensors/run.sh"),
        "#!/bin/sh\ncat \"$1\"\n",
    )?;
    ctx.post(
        "/api/v1/packs/install",
        json!({"source": pack.path(), "skip_tests": true, "skip_deps": true}),
        ctx.token(),
    )
    .await?
    .assert_status(StatusCode::OK);
    let release = PackReleaseRepository::find_active_by_pack_ref(&ctx.pool, "prepared-storage")
        .await?
        .unwrap();
    let pool = &ctx.pool;
    let repository = PackInstallRepository::new(pool.clone());
    let activation = repository
        .create(
            "prepared-storage",
            "1.0.0",
            "update",
            Some(release.pack),
            None,
        )
        .await?;
    assert!(repository.begin_activation(activation.id).await?.is_some());

    let recipe: RuntimeRecipe = serde_json::from_value(json!({
        "prepare_at_install": true,
        "tools": [{"name": "shell", "binaries": ["/bin/sh"], "args": ["-c", "printf 1.0"],
                   "version_pattern": "(?<version>1\\.0)", "primary": true}],
        "prepare": [{"command": ["{tool_shell}", "-c", "printf prepared > \"$1\"", "sh", "{output_dir}/built"]}],
        "launch": {"command": ["{tool_shell}", "{source_file}", "{output_dir}/built"]}
    }))?;
    let target = PreparationTarget {
        kind: PreparationWorkload::Sensor,
        reference: "prepared-storage.fixture".into(),
        source: "sensors/run.sh".into(),
        runtime_ref: "prepared-storage.shell".into(),
        runtime_names: vec!["shell".into()],
        recipe_digest: recipe.digest()?,
    };
    let environment = ChildProcessEnvironment::from_parent(
        [
            (
                "ATTUNE_RUNTIME_RECIPE_DIGESTS".into(),
                target.recipe_digest.clone().into(),
            ),
            (
                "ATTUNE_RUNTIME_PREPARATION_REVISION".into(),
                "prepared-test-image".into(),
            ),
        ],
        &[],
    )?
    .with_workload_sandbox(WorkloadSandbox::Bubblewrap)
    .with_preparation_storage(&storage);
    let runtime_root = TempDir::new()?;
    let scope = PreparationScope::open(runtime_root.path(), &environment)?.unwrap();
    let jwt = JwtConfig {
        secret: ctx.state.config.security.jwt_secret.clone().unwrap(),
        access_token_expiration: 300,
        refresh_token_expiration: 3600,
    };
    let writer_token = Arc::new(WorkerTokenProvider::new(1, "unregistered", jwt.clone()));
    let reader_token = Arc::new(WorkerTokenProvider::new(1, "unregistered", jwt));
    let worker_input = |name: &str, instance: Uuid| CreateWorkerInput {
        name: name.into(),
        worker_type: WorkerType::Container,
        runtime: None,
        host: None,
        port: None,
        status: Some(WorkerStatus::Active),
        capabilities: Some(json!({
            "runtime_preparation_scope": scope,
            "runtime_recipe_digests": [target.recipe_digest],
            "log_delivery_instance": instance,
        })),
        meta: None,
    };
    let writer_incarnation = Uuid::new_v4();
    let writer = WorkerRepository::register_incarnation(
        pool,
        worker_input("portable-builder", writer_token.instance_id()),
        WorkerRole::Action,
        writer_incarnation,
    )
    .await?;
    let reader = WorkerRepository::register_incarnation(
        pool,
        worker_input("portable-sensor", reader_token.instance_id()),
        WorkerRole::Sensor,
        Uuid::new_v4(),
    )
    .await?;
    writer_token.set_worker_id(writer.id.to_string());
    reader_token.set_worker_id(reader.id.to_string());

    let attempt = repository
        .create("prepared-storage", "1.0.0", PREPARATION_TRIGGER, None, None)
        .await?;
    let job = PreparationJob {
        target: target.clone(),
        recipe: recipe.clone(),
        scopes: vec![scope.clone()],
        placements: Vec::new(),
    };
    let mut tx = pool.begin().await?;
    let candidate = ctx
        .state
        .blob_store
        .head(&ObjectKey::new(release.object_key.as_deref().unwrap())?)
        .await?
        .unwrap();
    assert!(PackInstallRepository::attach_candidate_object(&mut tx, attempt.id, &candidate).await?);
    RuntimePreparationRepository::create_attempt(
        &mut tx,
        attempt.id,
        activation.id,
        &release.digest,
        &job,
    )
    .await?;
    tx.commit().await?;
    let access = "test-preparation-attempt";
    let token_hash = hash_integration_token(access);
    assert!(repository
        .claim_worker(attempt.id, writer.id, Some(&token_hash))
        .await?
        .is_some());
    assert!(RuntimePreparationRepository::begin(
        pool,
        attempt.id,
        writer.id,
        writer_incarnation,
        &scope
    )
    .await?
    .is_some());
    let prepared = recipe
        .prepare_for_install(
            pack.path(),
            target.clone(),
            &release.digest,
            &scope,
            &environment,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await?;

    let mut config = (*ctx.state.config).clone();
    config.storage = storage;
    let replica = || -> Result<_> {
        let mut state = (*ctx.state).clone();
        state.config = Arc::new(config.clone());
        state.blob_store = Arc::new(S3BlobStore::new(
            &bucket,
            &region,
            &prefix,
            Some(&endpoint),
            None,
        )?);
        Ok(attune_api::server::Server::new(Arc::new(state)).router())
    };
    let (writer_url, writer_stop, writer_server) = serve(replica()?).await?;
    let (reader_url, reader_stop, reader_server) = serve(replica()?).await?;
    let result: Result<()> = async {
        let writer = ApiPackTransport::new_with_worker_token_provider(
            &writer_url,
            writer_token,
            runtime_root.path().to_str().unwrap(),
        )?;
        assert!(writer
            .publish_prepared_runtime(attempt.id, "wrong-attempt", &prepared)
            .await
            .is_err());
        writer
            .publish_prepared_runtime(attempt.id, access, &prepared)
            .await?;
        let mut tx = pool.begin().await?;
        RuntimePreparationRepository::bind_release(
            &mut tx,
            &[attempt.id],
            activation.id,
            &release,
            &[job],
        )
        .await?;
        tx.commit().await?;
        let receipt =
            RuntimePreparationRepository::ready_for(pool, release.id, &release.digest, &target)
                .await?
                .pop()
                .unwrap();
        assert!(receipt.archive().unwrap().version.starts_with("v:"));

        // Losing every local byte simulates a replacement Pod mounted at the same path.
        fs::remove_dir_all(runtime_root.path())?;
        assert_eq!(
            PreparationScope::open(runtime_root.path(), &environment)?,
            Some(scope)
        );
        let reader = ApiPackTransport::new_with_worker_token_provider(
            &reader_url,
            reader_token,
            runtime_root.path().to_str().unwrap(),
        )?;
        reader.sync_prepared_runtime(release.id, &receipt).await?;
        let invocation = recipe
            .open_installed(&receipt, runtime_root.path(), &environment)
            .await?;
        let mut command = std::process::Command::new(&invocation.command[0]);
        environment.apply(&mut command);
        command
            .args(&invocation.command[1..])
            .envs(&invocation.env)
            .current_dir(&invocation.working_dir);
        let output = invocation
            .sandbox_invocation(command, &environment)?
            .output()?;
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"prepared");
        invocation.cleanup().await?;
        Ok(())
    }
    .await;
    let _ = writer_stop.send(());
    let _ = reader_stop.send(());
    writer_server.await??;
    reader_server.await??;
    result
}

async fn serve(
    app: axum::Router,
) -> Result<(
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = format!("http://{}", listener.local_addr()?);
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    Ok((address, stop, server))
}
