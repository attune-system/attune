//! Owning trusted-worker proof on an explicit disposable namespace. Provider
//! effects alone cannot establish admission, custody or atomic completion.
#![cfg(unix)]
#[path = "../../common/tests/helpers.rs"]
mod helpers;
use anyhow::{ensure, Context, Result};
use attune_common::{
    artifact_transport::ApiTransport,
    auth::{jwt::JwtConfig, WorkerTokenProvider},
    config::LogDeliveryConfig,
    isolated_process::{ProcessLauncher, ProcessSpec, ResourceIdentity},
    log_stream::SegmentedLogConfig,
    models::{ExecutionStatus, WorkerRole, WorkerType},
    pack_format,
    pack_registry::storage::PackStorage,
    repositories::{
        artifact::ArtifactVersionRepository,
        component_lifecycle::PackProjectionIds,
        delegated_execution::DelegatedExecutionRepository as Delegated,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_process::ExecutionProcessRepository as Effects,
        log_delivery::{ControlDecision, LogDeliveryRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::{CreateWorkerInput, WorkerRepository},
        schema_contract::{BindingOwner, SchemaContractRepository},
        FindById, Update,
    },
    workload_archive::ArchivePolicy,
    workload_config::{WorkloadBackendConfig, WorkloadConfig, WorkloadStorageLimits},
    workload_launch::*,
    workload_pack_input::{release_zip, ReleaseInputPin},
};
use attune_worker::{
    artifacts::ArtifactManager, log_delivery::LogDeliveryManager,
    log_delivery_spool::LogDeliverySpool, runtime::RuntimeRegistry, ActionExecutor, SecretManager,
};
use attune_workload_backend::{
    cache::{CacheLimits, CacheStore},
    configuration::{ConfiguredBackend, NativeBackend},
    kubernetes::{KubernetesLauncher, KubernetesPolicy},
};
use helpers::{ActionFixture, IdentityFixture, PackFixture, RuntimeFixture};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{DeleteParams, Preconditions},
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
const MASTER: &str = "owned-worker-isolation-master-encryption-key";
fn required(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing owned fixture prerequisite: {name}"))
}
fn image(name: &str) -> Result<PinnedImage> {
    let reference = required(name)?;
    let (_, digest) = reference
        .split_once("@sha256:")
        .context("immutable image required")?;
    let sha256 = Sha256Digest::parse(digest)?;
    Ok(PinnedImage { reference, sha256 })
}
fn pin(
    kind: LaunchInputKind,
    object: i64,
    version: String,
    file: &Path,
    expanded: u64,
    files: u64,
) -> Result<PinnedLaunchInput> {
    let bytes = std::fs::read(file)?;
    Ok(PinnedLaunchInput {
        pin_id: object,
        kind,
        object_id: object,
        object_version: version,
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::of_bytes(&bytes),
        max_expanded_bytes: expanded,
        max_files: files,
    })
}
fn runner_zip(path: &Path, program: &Path, platform: LaunchPlatform) -> Result<()> {
    let mut archive = zip::ZipWriter::new(File::create_new(path)?);
    archive.start_file(
        format!("linux-{}", platform.native_arch()),
        zip::write::SimpleFileOptions::default().unix_permissions(0o755),
    )?;
    archive.write_all(&std::fs::read(program)?)?;
    archive.finish()?.sync_all()?;
    Ok(())
}
async fn client() -> Result<Client> {
    let path = PathBuf::from(required("ATTUNE_ISOLATION_TEST_KUBECONFIG")?);
    ensure!(path.is_absolute(), "owned kubeconfig must be absolute");
    let config = kube::Config::from_custom_kubeconfig(
        Kubeconfig::read_from(path)?,
        &KubeConfigOptions {
            context: Some(required("ATTUNE_ISOLATION_TEST_CONTEXT")?),
            ..Default::default()
        },
    )
    .await?;
    Ok(Client::try_from(config)?)
}
async fn remove_owned(client: Client, identity: &ResourceIdentity) -> Result<()> {
    let ResourceIdentity::Kubernetes {
        namespace,
        pod_name,
        pod_uid,
        ..
    } = identity
    else {
        anyhow::bail!("wrong provider identity")
    };
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    pods.delete(
        pod_name,
        &DeleteParams {
            preconditions: Some(Preconditions {
                uid: Some(pod_uid.clone()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match pods.get_opt(pod_name).await? {
                None => return Ok::<_, anyhow::Error>(()),
                Some(pod) => ensure!(
                    pod.metadata.uid.as_deref() == Some(pod_uid),
                    "owned resource replaced"
                ),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace, pinned binaries/images and disposable PostgreSQL"]
async fn kubernetes_native_action_custody_completion_and_restart() -> Result<()> {
    let _provider = KUBERNETES_CASE.lock().await;
    owning_case(true, false, false).await
}

// No provider effect is claimed: this catches actual preparer/custody/completion
// regressions on PostgreSQL before the owning namespace job executes the Pod.
#[tokio::test]
async fn trusted_worker_completion_and_custody_repository_case() -> Result<()> {
    owning_case(false, false, false).await
}

// Each real case requires the owned node's fixed payload CPU budget. Retain
// four database-test threads while serializing only these shared-provider cases.
static KUBERNETES_CASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace, pinned binaries/images and disposable PostgreSQL"]
async fn kubernetes_native_cli_action_custody_completion_and_restart() -> Result<()> {
    let _provider = KUBERNETES_CASE.lock().await;
    owning_case(true, true, false).await
}

#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace, pinned binaries/images and disposable PostgreSQL"]
async fn kubernetes_command_action_custody_completion_and_restart() -> Result<()> {
    let _provider = KUBERNETES_CASE.lock().await;
    owning_case(true, true, true).await
}

async fn owning_case(real_provider: bool, typed_cli: bool, command: bool) -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let root = tempfile::tempdir()?;
    let (platform, echo, runner, client, payload_image) = if real_provider {
        let platform = match required("ATTUNE_ISOLATION_TEST_PLATFORM")?.as_str() {
            "linux/amd64" => LaunchPlatform::LinuxAmd64,
            "linux/arm64" => LaunchPlatform::LinuxArm64,
            _ => anyhow::bail!("unsupported admitted platform"),
        };
        let echo = PathBuf::from(required("ATTUNE_ISOLATION_TEST_ECHO_PATH")?);
        let runner = PathBuf::from(required("ATTUNE_ISOLATION_TEST_RUNNER_PATH")?);
        ensure!(
            echo.is_absolute() && runner.is_absolute(),
            "owned binaries must be absolute"
        );
        (
            platform,
            echo,
            runner,
            Some(client().await?),
            image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE")?,
        )
    } else {
        let echo = root.path().join("public-echo-fixture");
        let runner = root.path().join("public-runner-fixture");
        std::fs::write(
            &echo,
            b"opaque public test executable; never executed on the host",
        )?;
        std::fs::write(
            &runner,
            b"opaque public test helper; never executed on the host",
        )?;
        let sha256 = Sha256Digest::of_bytes(b"explicit-mock-platform-image");
        (
            LaunchPlatform::LinuxAmd64,
            echo,
            runner,
            None,
            PinnedImage {
                reference: format!("owned.test/native@sha256:{}", sha256.as_str()),
                sha256,
            },
        )
    };
    let database = helpers::create_test_pool().await?;
    let cache = Arc::new(CacheStore::open(
        root.path().join("cache"),
        CacheLimits {
            compressed_bytes: 64 << 20,
            expanded_bytes: 128 << 20,
            files: 4096,
        },
    )?);
    let concrete = if let Some(client) = &client {
        let policy = KubernetesPolicy {
            cluster: required("ATTUNE_ISOLATION_TEST_CLUSTER")?,
            namespace: required("ATTUNE_ISOLATION_TEST_NAMESPACE")?,
            helper_image: image("ATTUNE_ISOLATION_TEST_HELPER_IMAGE")?,
            allowed_images: BTreeSet::from([payload_image.reference.clone()]),
            memory_bytes: 128 << 20,
            scratch_bytes: 64 << 20,
        };
        Some(Arc::new(KubernetesLauncher::new(
            client.clone(),
            policy,
            cache.clone(),
        )))
    } else {
        None
    };
    let launcher: Arc<dyn ProcessLauncher> = concrete
        .as_ref()
        .map(|launcher| launcher.clone() as Arc<dyn ProcessLauncher>)
        .unwrap_or_else(|| Arc::new(RepositoryOnlyLauncher));
    let owner = IdentityFixture::new_unique("isolated_owner")
        .create(&database)
        .await?;
    let pack = PackFixture::new_unique("isolated_native")
        .create(&database)
        .await?;
    let mut runtime = RuntimeFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "native");
    runtime.execution_config = json!({});
    let runtime = runtime.create(&database).await?;
    let relative = "echo";
    let binary_relative = format!("echo/linux-{}", platform.native_arch());
    let action = ActionFixture::new(pack.id, &pack.r#ref, "echo")
        .with_runtime(runtime.id)
        .with_entrypoint(relative)
        .with_param_schema(json!({"message":{"type":"string","required":true}}))
        .create(&database)
        .await?;
    let action = if typed_cli {
        use attune_common::repositories::{
            action::{ActionRepository, UpdateActionInput},
            Patch,
        };
        let launch = if command {
            json!({"type":"command","program":"/usr/local/libexec/attune/echo","arguments":[{"literal":"literal value; $(false)","order":0}]})
        } else {
            json!({"type":"native","artifact":"echo","arguments":[{"literal":"literal value; $(false)","order":0}]})
        };
        ActionRepository::update(&database, action.id, UpdateActionInput {
            entrypoint: Some(Patch::Clear),
            launch: Some(Patch::Set(serde_json::from_value(launch)?)),
            parameter_delivery: Some("stdin".into()), parameter_format: Some("json".into()),
            param_schema: Some(json!({"message":{"type":"string","required":true,"cli":{"kind":"positional","order":1}}})),
            ..Default::default()
        }).await?
    } else {
        action
    };
    // Use the primary upload/initialization publication contract, including
    // its sidecar format_version:1 inventory. The offline CLI release format
    // is deliberately not an alternate producer/reader in this owning case.
    let source = root.path().join("source");
    std::fs::create_dir_all(source.join("actions/echo"))?;
    std::fs::write(
        source.join("pack.yaml"),
        format!("ref: {}\nversion: {}\n", pack.r#ref, pack.version),
    )?;
    std::fs::write(
        source.join("actions/echo.yaml"),
        format!(
            "ref: {}\nrunner_type: native\nentry_point: {}\n",
            action.r#ref, relative
        ),
    )?;
    let executable = source.join("actions").join(&binary_relative);
    std::fs::copy(&echo, &executable)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
    if typed_cli {
        let bytes = std::fs::read(&executable)?;
        let path = format!("actions/{binary_relative}");
        let metadata = json!({
            "schema":"attune.pack.release/v1", "pack":{"ref":pack.r#ref,"version":pack.version},
            "requires":{"attune":"*","platform_catalog":"v1","runtimes":["native"]},
            "dependencies":{}, "evidence":{"sboms":[],"provenance":[]},
            "files":[{"path":path,"size":bytes.len(),"sha256":Sha256Digest::of_bytes(&bytes).as_str(),"mode":"0755"}],
            "artifacts":[{"kind":"native","id":"echo","component_refs":[action.r#ref],"variants":[{
                "id":format!("linux-{}-static", platform.native_arch()),"path":path,
                "target":{"os":"linux","arch":platform.native_arch(),"libc":"static"}
            }]}]
        });
        std::fs::create_dir_all(source.join(".attune"))?;
        std::fs::write(
            source.join(pack_format::MANIFEST_PATH),
            serde_json::to_vec(&metadata)?,
        )?;
        std::fs::write(
            source.join("actions/echo.yaml"),
            serde_json::to_vec(&json!({
                "ref":action.r#ref,"runner_type":"native","launch":action.launch,"param_schema":action.param_schema,
                "parameter_delivery":"stdin","parameter_format":"json"
            }))?,
        )?;
    }
    let publication_root = root.path().join("publication");
    let publish_root = publication_root.clone();
    let pack_ref = pack.r#ref.clone();
    let version = pack.version.clone();
    let published = tokio::task::spawn_blocking(move || {
        PackStorage::new(publish_root).publish_release(&source, &pack_ref, &version)
    })
    .await??;
    let zip = release_zip(
        File::open(&published.archive_path)?,
        &ReleaseInputPin {
            pack_ref: pack.r#ref.clone(),
            size_bytes: published.archive_size,
            sha256: Sha256Digest::parse(&published.digest)?,
        },
        &published.manifest,
        pack_format::Limits::default(),
        &ArchivePolicy {
            root: LaunchInputKind::Pack,
            max_compressed_bytes: 64 << 20,
            max_expanded_bytes: 128 << 20,
            max_files: 4096,
        },
    )?;
    let mut tx = database.begin().await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest: published.digest.clone(),
            object_key: format!("packs/blobs/{}.tar.gz", published.digest),
            provider_version: "e:owned-fixture".into(),
            content_path: published.pack_path.to_string_lossy().into_owned(),
            archive_size: i64::try_from(published.archive_size)?,
            manifest: serde_json::to_value(&published.manifest)?,
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id],
            runtimes: vec![runtime.id],
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    let pack_pin = PinnedLaunchInput {
        pin_id: release.id,
        kind: LaunchInputKind::Pack,
        object_id: release.id,
        object_version: release.digest.clone(),
        size_bytes: zip.size_bytes(),
        sha256: zip.sha256().clone(),
        max_expanded_bytes: 128 << 20,
        max_files: 4096,
    };
    // The real producer installs its own admitted pin, including observed ZIP
    // bounds. Only the process double uses this manually supplied Pack pin.
    if !real_provider {
        cache.install_input(&pack_pin, zip.into_file().as_file_mut())?;
    }
    let runner_archive = root.path().join("runner.zip");
    runner_zip(&runner_archive, &runner, platform)?;
    let runner_pin = pin(
        LaunchInputKind::Runner,
        1,
        "owned-once-built-helper".into(),
        &runner_archive,
        64 << 20,
        16,
    )?;
    cache.install_input(&runner_pin, &mut File::open(&runner_archive)?)?;
    let jwt = JwtConfig {
        secret: "owned-worker-isolation-signing-key".into(),
        ..Default::default()
    };
    let tokens = Arc::new(WorkerTokenProvider::new(owner.id, "0", jwt.clone()));
    let incarnation = Uuid::new_v4();
    let worker = WorkerRepository::register_incarnation(
        &database,
        CreateWorkerInput {
            name: format!("isolated-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: None,
            capabilities: Some(json!({"log_delivery_instance":tokens.instance_id()})),
            meta: None,
        },
        WorkerRole::Action,
        incarnation,
    )
    .await?;
    tokens.set_worker_id(worker.id.to_string());
    WorkerRepository::heartbeat_incarnation(&database, worker.id, incarnation).await?;
    let message = format!("owned-captured-{}", Uuid::new_v4());
    let mut tx = database.begin().await?;
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id).await?;
    let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref,
            executor: Some(owner.id),
            config: Some(json!({"message":message})),
            timeout_seconds: Some(300),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await?;
    Delegated::select(&mut tx, execution.id, None).await?;
    ExecutionRepository::update(
        &mut *tx,
        execution.id,
        UpdateExecutionInput {
            worker: Some(worker.id),
            status: Some(ExecutionStatus::Scheduled),
            ..Default::default()
        },
    )
    .await?;
    let attempt = Delegated::start(&mut tx, execution.id, worker.id).await?;
    let execution = ExecutionRepository::transition_worker(
        &mut *tx,
        execution.id,
        Some(worker.id),
        ExecutionStatus::Running,
        None,
    )
    .await?
    .execution;
    tx.commit().await?;
    // Explicit CP outage. This is not an object-store/API success substitute:
    // the actual repository handoff must retain finalizing encrypted custody.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let api = format!("http://{}", listener.local_addr()?);
    let server_stop = CancellationToken::new();
    let shutdown = server_stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
    });
    let artifacts = root.path().join("artifacts");
    let spool_base = root.path().join("custody");
    let delivery = LogDeliveryConfig {
        spool_dir: Some(spool_base.to_string_lossy().into_owned()),
        max_spool_bytes: 32 << 20,
        max_finalizers: 4,
        ..Default::default()
    };
    let executor = ActionExecutor::new(
        database.pool().clone(),
        RuntimeRegistry::new(),
        ArtifactManager::new(artifacts.clone()),
        SecretManager::new(database.pool().clone(), Some(MASTER.into()))?,
        65536,
        65536,
        false,
        false,
        worker.name.clone(),
        tokens.instance_id(),
        SegmentedLogConfig {
            initial_segment_bytes: 1024,
            max_segment_bytes: 16384,
            flush_interval_ms: 10,
            retry_max_attempts: 1,
            retry_attempt_timeout_ms: 500,
            retry_initial_backoff_ms: 1,
            retry_max_backoff_ms: 1,
            finalization_timeout_ms: 5000,
        },
        None,
        None,
        publication_root.clone(),
        artifacts.clone(),
        root.path().join("runtimes"),
        api.clone(),
        jwt.clone(),
        Arc::new(ApiTransport::new_with_worker_token_provider(
            &api,
            tokens.clone(),
            artifacts.to_str().context("artifacts path")?,
        )?),
    )
    .with_worker_token_provider(tokens);
    let executor = if let Some(backend) = &concrete {
        // Real provider proof uses the same trusted producer as the service,
        // not the hand-built ProcessSpec used by the process-double case.
        executor.with_native_execution(
            WorkloadConfig {
                image: payload_image.clone(),
                platform,
                cache_root: root.path().join("cache"),
                memory_bytes: 128 << 20,
                scratch_bytes: 64 << 20,
                storage: WorkloadStorageLimits {
                    compressed_bytes: 64 << 20,
                    expanded_bytes: 128 << 20,
                    entries: 4096,
                    artifacts: Default::default(),
                },
                helper: runner_pin.clone(),
                helper_archive: runner_archive,
                backend: WorkloadBackendConfig::Kubernetes {
                    cluster: required("ATTUNE_ISOLATION_TEST_CLUSTER")?,
                    namespace: required("ATTUNE_ISOLATION_TEST_NAMESPACE")?,
                    helper_image: image("ATTUNE_ISOLATION_TEST_HELPER_IMAGE")?,
                },
            },
            ConfiguredBackend {
                backend: NativeBackend::Kubernetes(backend.clone()),
                cache: cache.clone(),
                runner: runner_pin.clone(),
            },
        )?
    } else {
        executor
    };
    executor.set_worker_id(worker.id);
    executor.set_process_incarnation(incarnation)?;
    ensure!(
        executor.set_process_incarnation(Uuid::new_v4()).is_err(),
        "native owner rebound"
    );
    let spec = ProcessSpec {
        intent: Uuid::new_v4(),
        purpose: LaunchPurpose::Action,
        target: LaunchTarget::Execution {
            execution: execution.id,
            worker: worker.id,
            worker_instance: incarnation,
            attempt,
        },
        image: payload_image,
        platform,
        inputs: vec![pack_pin, runner_pin],
        pack_access: PackAccess::ReadOnly,
        stage: None,
        timeout_ms: Some(300_000),
        invocation: LaunchInvocation {
            argv: Vec::new(),
            env: BTreeMap::new(),
            working_root: LaunchRoot::Pack,
        },
    };
    let mut intent = spec.intent;
    let heartbeat_stop = CancellationToken::new();
    let heartbeat_cancel = heartbeat_stop.clone();
    let heartbeat_pool = database.pool().clone();
    let worker_id = worker.id;
    let heartbeat = tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! { _ = heartbeat_cancel.cancelled() => return Ok::<_, attune_common::Error>(()), _ = timer.tick() => {} }
            WorkerRepository::heartbeat_incarnation(&heartbeat_pool, worker_id, incarnation)
                .await?;
        }
    });
    let result = async {
        executor.acquire_log_custody(&delivery, worker.id).await?;
        executor.start_log_delivery(delivery.clone()).await?;
        let outcome = tokio::time::timeout(Duration::from_secs(240), async {
            if real_provider {
                executor
                    .execute_started_native(execution.clone(), CancellationToken::new())
                    .await
            } else {
                executor
                    .execute_started_isolated(
                        execution.clone(),
                        CancellationToken::new(),
                        spec,
                        launcher.clone(),
                    )
                    .await
            }
        })
        .await??;
        let retained =
            Effects::retained_for_attempt(database.pool(), execution.id, attempt, MASTER)
                .await?
                .context("trusted producer did not retain its native intent")?;
        intent = retained.intent;
        if typed_cli {
            ensure!(
                retained.invocation.argv.len() == 7
                    && retained.invocation.argv[5] == "literal value; $(false)"
                    && retained.invocation.argv[6] == message,
                "native CLI argv differs from admitted arguments"
            );
        }
        if command {
            let probe = Effects::retained_for_purpose(
                database.pool(),
                execution.id,
                attempt,
                LaunchPurpose::RuntimeProbe,
                MASTER,
            )
            .await?
            .context("command probe receipt missing")?;
            ensure!(
                probe.image == retained.image
                    && probe.platform == retained.platform
                    && probe.inputs == retained.inputs
                    && probe.invocation.argv[1] == "inspect-command-program"
                    && !probe.invocation.env.contains_key("ATTUNE_API_TOKEN"),
                "command inspection binding differs"
            );
            let mut tx = database.begin().await?;
            ensure!(
                Effects::lock_completion(&mut tx, probe.intent, MASTER)
                    .await
                    .is_err(),
                "probe must never complete the action"
            );
            tx.rollback().await?;
        }
        let retained_pack = retained
            .inputs
            .iter()
            .find(|input| input.kind == LaunchInputKind::Pack)
            .context("retained native intent has no Pack pin")?;
        cache.open_input_archive(retained_pack)?;
        ensure!(outcome.exit_code == 0, "native action failed");
        let stored = ExecutionRepository::find_by_id(&database, execution.id)
            .await?
            .context("stored execution")?;
        ensure!(
            stored.status == ExecutionStatus::Completed,
            "completion was not durable"
        );
        ensure!(
            stored
                .result
                .as_ref()
                .and_then(|v| v.get("stdout"))
                .and_then(|v| v.as_str())
                == Some(&message),
            "wrong admitted output"
        );
        let mut tx = database.begin().await?;
        let historical = Effects::lock_completion(&mut tx, intent, MASTER)
            .await?
            .context("missing historical ACK")?;
        ensure!(
            historical.execution.result == stored.result,
            "completion replay changed output"
        );
        tx.commit().await?;
        ensure!(
            Effects::consume_stdin(
                database.pool(),
                intent,
                &Effects::retained_resource(database.pool(), intent)
                    .await?
                    .context("native identity")?,
                MASTER
            )
            .await
            .is_err(),
            "stdin replay succeeded"
        );
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM log_delivery_intent WHERE execution=$1 AND state='active'",
        )
        .bind(execution.id)
        .fetch_one(&database)
        .await?;
        ensure!(
            pending == 1,
            "stdout custody was not accepted independently of API outage"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // Recover ownership for failure teardown too, including errors before the
    // normal completion acknowledgement exposed the producer-selected intent.
    if let Ok(Some(retained)) =
        Effects::retained_for_attempt(database.pool(), execution.id, attempt, MASTER).await
    {
        intent = retained.intent;
    }
    // Attempt/capture producers have joined. Stop transport without consuming
    // the custody deadline; no uncertain remote effect is treated as absence.
    executor.stop_log_delivery().await;
    drop(executor);
    heartbeat_stop.cancel();
    let heartbeat_result = heartbeat.await;
    let identity = Effects::retained_resource(database.pool(), intent).await;
    let restart = async {
        result?;
        let spool = LogDeliverySpool::open(
            spool_base.join(worker.id.to_string()),
            worker.id,
            32 << 20,
            8 << 20,
            1 << 20,
        )
        .await?;
        let versions = spool.versions().await?;
        ensure!(versions.len() == 1, "restart lost encrypted stdout custody");
        let entry = spool.load(versions[0]).await?;
        ensure!(
            entry.execution == execution.id && entry.attempt == attempt,
            "custody crossed attempts"
        );
        let bytes: Vec<u8> = entry
            .segments
            .iter()
            .flat_map(|s| s.bytes.iter().copied())
            .collect();
        ensure!(
            !bytes
                .windows(message.len())
                .any(|part| part == message.as_bytes()),
            "protected log contains plaintext"
        );
        let plaintext = attune_common::runtime_read_log::decode_complete(
            MASTER,
            entry
                .authenticated_manifest
                .as_deref()
                .context("missing final manifest")?,
            execution.id,
            entry.version,
            &bytes,
        )?;
        ensure!(
            plaintext == message.as_bytes(),
            "encrypted custody roundtrip changed capture"
        );
        let custody_key =
            std::fs::read(spool_base.join(worker.id.to_string()).join("custody.key"))?;
        drop(spool);
        // Genuine registration replacement, distinct from merely reopening a
        // reader. It invalidates the old bearer but preserves retained custody.
        let next_tokens = Arc::new(WorkerTokenProvider::new(
            owner.id,
            worker.id.to_string(),
            jwt,
        ));
        let next_incarnation = Uuid::new_v4();
        let registered = WorkerRepository::register_incarnation(
            &database,
            CreateWorkerInput {
                name: worker.name.clone(),
                worker_type: WorkerType::Local,
                runtime: None,
                host: None,
                port: None,
                status: None,
                capabilities: Some(json!({"log_delivery_instance":next_tokens.instance_id()})),
                meta: None,
            },
            WorkerRole::Action,
            next_incarnation,
        )
        .await?;
        ensure!(
            registered.id == worker.id,
            "restart changed worker identity"
        );
        WorkerRepository::heartbeat_incarnation(&database, worker.id, next_incarnation).await?;
        let mut tx = database.begin().await?;
        ensure!(
            LogDeliveryRepository::require_instance(&mut tx, worker.id, entry.source_instance)
                .await
                .is_err(),
            "old delivery instance remained current"
        );
        tx.rollback().await?;
        let binding = attune_common::log_delivery::ControlRequest {
            execution: entry.execution,
            version: entry.version,
            stream: entry.stream,
            attempt: entry.attempt,
            source_instance: entry.source_instance,
            custody: entry.custody,
        };
        let decision = LogDeliveryRepository::reconcile_control(
            database.pool(),
            worker.id,
            next_tokens.instance_id(),
            &binding,
            chrono::Utc::now().timestamp() + 60,
        )
        .await?;
        match decision {
            ControlDecision::Deliver(fence) => ensure!(
                fence.intent.delivery_instance == next_tokens.instance_id()
                    && fence.intent.source_instance == entry.source_instance,
                "custody handover rebound source"
            ),
            _ => anyhow::bail!("restart lost active delivery authority"),
        }
        let recovered = LogDeliveryManager::open(
            database.pool().clone(),
            spool_base.join(worker.id.to_string()),
            worker.id,
            next_tokens.instance_id(),
            next_tokens.clone(),
            api.clone(),
            delivery.clone(),
            attune_common::runtime_read_log::maximum_encoded_bytes(65536)?,
            16384,
        )
        .await?;
        let overlapping = LogDeliverySpool::open(
            spool_base.join(worker.id.to_string()),
            worker.id,
            32 << 20,
            8 << 20,
            1 << 20,
        )
        .await;
        recovered.stop_and_join().await;
        drop(recovered);
        ensure!(
            overlapping.is_err(),
            "restart allowed simultaneous custody owners"
        );
        ensure!(
            std::fs::read(spool_base.join(worker.id.to_string()).join("custody.key"))?
                == custody_key,
            "restart rotated custody key"
        );
        let mut tx = database.begin().await?;
        let historical = Effects::lock_completion(&mut tx, intent, MASTER)
            .await?
            .context("restart lost committed completion")?;
        ensure!(
            historical.execution.status == ExecutionStatus::Completed,
            "restart changed terminal outcome"
        );
        tx.commit().await?;
        // A replacement/missing volume has no ciphertext. Reconciliation must
        // publish explicit incomplete under the existing custody contract.
        let lost = LogDeliveryManager::open(
            database.pool().clone(),
            root.path().join("replacement-empty-volume"),
            worker.id,
            next_tokens.instance_id(),
            next_tokens.clone(),
            api.clone(),
            delivery,
            attune_common::runtime_read_log::maximum_encoded_bytes(65536)?,
            16384,
        )
        .await?;
        lost.stop_and_join().await;
        drop(lost);
        let intent_row = LogDeliveryRepository::find(&database, entry.version)
            .await?
            .context("lost custody intent")?;
        ensure!(
            intent_row.state == "incomplete"
                && intent_row.failure.as_deref() == Some("custody_lost"),
            "volume loss did not become incomplete"
        );
        let stored = ExecutionRepository::find_by_id(&database, execution.id)
            .await?
            .context("execution after restart")?;
        let availability = {
            let mut connection = database.acquire().await?;
            LogDeliveryRepository::availability(&mut connection, &stored).await?
        };
        ensure!(
            availability["stdout"].state
                == attune_common::models::log_availability::LogAvailabilityState::Incomplete,
            "lost volume remained ready/finalizing"
        );
        ensure!(
            ArtifactVersionRepository::find_ordinary_file_versions_by_execution(
                &database,
                execution.id
            )
            .await?
            .is_empty(),
            "cleanup preceded ordinary file publication"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // Normal no-file cleanup uses the same concrete effect as the owning
    // trusted loop, after terminal/custody settlement and exact inventory.
    // Failure teardown still deletes only this test's UID-bound resource.
    let cleanup = async {
        if let (Some(client), Ok(Some(identity))) = (client, &identity) {
            if restart.is_ok() {
                let ResourceIdentity::Kubernetes {
                    namespace,
                    pod_name,
                    pod_uid,
                    ..
                } = identity
                else {
                    anyhow::bail!("wrong provider identity")
                };
                let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
                if let Some(pod) = pods.get_opt(pod_name).await? {
                    ensure!(
                        pod.metadata.uid.as_deref() == Some(pod_uid),
                        "owned resource replaced"
                    );
                    remove_owned(client, identity).await?;
                    Err(anyhow::anyhow!(
                        "trusted completion left its stopped Pod unremoved"
                    ))
                } else {
                    Ok(())
                }
            } else {
                remove_owned(client, identity).await
            }
        } else {
            Ok(())
        }
    }
    .await;
    server_stop.cancel();
    let server_result = server.await;
    let db_cleanup = database.cleanup().await;
    let publication_cleanup = tokio::task::spawn_blocking(move || {
        PackStorage::new(publication_root)
            .remove_release_tree(&published.digest, &published.pack_path.to_string_lossy())
    })
    .await;
    restart?;
    identity?;
    heartbeat_result??;
    cleanup?;
    server_result??;
    db_cleanup?;
    ensure!(
        publication_cleanup??,
        "owned publication cleanup was not acknowledged"
    );
    Ok(())
}

/// In-memory effects double only. The owning real-provider case above uses
/// KubernetesLauncher unchanged; this double never launches an executable.
struct RepositoryOnlyLauncher;
struct RepositoryOnlyControl {
    stdout: tokio::sync::Mutex<Option<tokio::io::DuplexStream>>,
    exited: std::sync::atomic::AtomicBool,
    changed: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl attune_common::isolated_process::ProcessControl for RepositoryOnlyControl {
    async fn write_stdin_once(
        &self,
        payload: attune_common::workload_stdin::LaunchStdinPayload,
    ) -> attune_common::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut output = self
            .stdout
            .lock()
            .await
            .take()
            .ok_or_else(|| attune_common::Error::invalid_state("test stdin replay"))?;
        let value: serde_json::Value = serde_json::from_slice(payload.as_bytes())?;
        let message = value["message"]
            .as_str()
            .ok_or_else(|| attune_common::Error::validation("test input mismatch"))?;
        output
            .write_all(message.as_bytes())
            .await
            .map_err(|error| attune_common::Error::Io(error.to_string()))?;
        output
            .shutdown()
            .await
            .map_err(|error| attune_common::Error::Io(error.to_string()))?;
        self.exited
            .store(true, std::sync::atomic::Ordering::Release);
        self.changed.notify_waiters();
        Ok(())
    }
    async fn wait(&self) -> attune_common::Result<LaunchExitOutcome> {
        loop {
            let changed = self.changed.notified();
            if self.exited.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(LaunchExitOutcome {
                    exit_code: 0,
                    oom_killed: false,
                });
            }
            changed.await;
        }
    }
    async fn stop(&self, _reason: LaunchStopReason) -> attune_common::Result<LaunchExitOutcome> {
        self.stdout.lock().await.take();
        self.exited
            .store(true, std::sync::atomic::Ordering::Release);
        self.changed.notify_waiters();
        Ok(LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        })
    }
}
#[async_trait::async_trait]
impl ProcessLauncher for RepositoryOnlyLauncher {
    async fn prepare(
        &self,
        spec: &ProcessSpec,
        disposition: attune_common::isolated_process::ProcessCreateDisposition,
    ) -> attune_common::Result<ResourceIdentity> {
        if disposition != attune_common::isolated_process::ProcessCreateDisposition::CreateOnce {
            return Err(attune_common::Error::invalid_state(
                "test unexpected resubmit",
            ));
        }
        if spec.invocation.argv.get(1).map(String::as_str) != Some("exec")
            || !spec.invocation.env.contains_key("ATTUNE_EXEC_ID")
            || spec.invocation.env.contains_key("ENCRYPTION_KEY")
            || spec.invocation.env.contains_key("DATABASE_URL")
        {
            return Err(attune_common::Error::invalid_state(
                "trusted native invocation missing",
            ));
        }
        Ok(ResourceIdentity::Kubernetes {
            cluster: "repository-only-double".into(),
            namespace: "no-cluster-used".into(),
            pod_name: spec.intent.to_string(),
            pod_uid: spec.intent.to_string(),
        })
    }
    async fn start(
        &self,
        _spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> attune_common::Result<attune_common::isolated_process::ProcessIo> {
        let (stdout, writer) = tokio::io::duplex(65536);
        let (stderr, closed) = tokio::io::duplex(16);
        drop(closed);
        Ok(attune_common::isolated_process::ProcessIo {
            resource: resource.clone(),
            stdout: Box::pin(stdout),
            stderr: Box::pin(stderr),
            control: Arc::new(RepositoryOnlyControl {
                stdout: tokio::sync::Mutex::new(Some(writer)),
                exited: std::sync::atomic::AtomicBool::new(false),
                changed: tokio::sync::Notify::new(),
            }),
        })
    }
}
