//! Actual SensorManager acceptance. Selected explicitly by the existing owned
//! provider lanes; missing prerequisites fail, and no process/API doubles exist.
#[path = "../../../api/tests/helpers.rs"]
mod api_helpers;
#[path = "../../../common/tests/helpers.rs"]
mod helpers;

use super::*;
use anyhow::{ensure, Context as _};
use attune_common::{
    artifact_transport::ApiTransport,
    blob_store::{body_from_file, ObjectKey, ProviderVersion},
    config::Config,
    models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    pack_registry::storage::PackStorage,
    repositories::{
        artifact::{ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput},
        component_configuration::ComponentConfigurationRepository,
        component_lifecycle::PackProjectionIds,
        log_stream::LogStreamRepository,
        managed_sensor_metadata::ManagedSensorMetadataRepository,
        object_maintenance::ObjectMaintenanceRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        rule::{CreateRuleInput, RuleRepository},
        runtime::WorkerRepository,
        sensor_schema::SensorSchemaRepository,
        trigger::{SensorRepository, TriggerRepository, UpdateSensorInput, UpdateTriggerInput},
        Create, FindByRef, Patch,
    },
    workload_config::{WorkloadBackendConfig, WorkloadConfig, WorkloadStorageLimits},
};
use attune_workload_backend::{
    cache::{CacheLimits, CacheStore},
    configuration::{ConfiguredBackend, NativeBackend},
    engine::DockerEngine,
    kubernetes::{KubernetesLauncher, KubernetesPolicy},
};
use futures::TryStreamExt;
use helpers::{ActionFixture, PackFixture, RuntimeFixture, SensorFixture, TriggerFixture};
use std::{
    collections::BTreeSet,
    fs::File,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const BOOTSTRAP_BYTES: usize = 9 * 1024 * 1024;
const OUTPUT: &[u8] = b"owned-sensor-output";
const SOURCE: &str = r#"import hashlib,json,os,signal,sys,time,urllib.request
from pathlib import Path
assert Path('/usr/local/libexec/attune/echo').is_file()
for name in ['DATABASE_URL','JWT_SECRET','ENCRYPTION_KEY','ATTUNE_RUNTIME_RECIPE_DIGESTS','ATTUNE__DUMMY_IMAGE_CREDENTIAL']:
    assert name not in os.environ, name
assert not Path('/var/run/secrets/kubernetes.io/serviceaccount/token').exists()
assert not Path('/var/run/docker.sock').exists()
bootstrap=json.load(sys.stdin)
assert set(bootstrap)=={'config','triggers'}
assert len(bootstrap['config']['large'])==9*1024*1024
assert set(bootstrap['config']['large'])=={'x'}
assert bootstrap['triggers'], 'missing admitted initial rule bootstrap'
if bootstrap['config']['recipe']:
    assert Path('recipe-private-marker').read_text()=='private'
else:
    assert Path('/opt/attune/runtime/project/installed-marker').read_text()=='prepared'
    assert Path('/opt/attune/runtime/bin/python3').is_file()
api=os.environ['ATTUNE_API_URL']+'/api/v1/'
headers={'Authorization':'Bearer '+os.environ['ATTUNE_API_TOKEN'],'Content-Type':'application/json'}
for data in [b'owned-sensor-output',b'']:
    req=urllib.request.Request(api+'artifacts/'+str(bootstrap['config']['artifact_id'])+'/versions/file',data=b'{"content_type":"application/octet-stream"}',headers=headers,method='POST')
    with urllib.request.urlopen(req,timeout=30) as r:
        assert r.status==201
        path=json.load(r)['data']['file_path']
    h=dict(headers,**{'Content-Type':'application/octet-stream','x-attune-sha256':hashlib.sha256(data).hexdigest()})
    with urllib.request.urlopen(urllib.request.Request(api+'internal/files/'+path,data=data,headers=h,method='PUT'),timeout=30) as r:
        assert r.status==201
print('sensor-stdout-bootstrap:'+str(len(bootstrap['config']['large'])),flush=True)
print('sensor-stderr-separate',file=sys.stderr,flush=True)
signal.signal(signal.SIGTERM,lambda *_:sys.exit(0))
while True: time.sleep(1)
"#;

fn required(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing owning sensor prerequisite: {name}"))
}
fn image(name: &str) -> Result<PinnedImage> {
    let reference = required(name)?;
    let digest = reference
        .strip_prefix("sha256:")
        .or_else(|| reference.split_once("@sha256:").map(|(_, d)| d))
        .context("immutable owned image required")?;
    let sha256 = Sha256Digest::parse(digest)?;
    Ok(PinnedImage { reference, sha256 })
}
struct Provider {
    root: tempfile::TempDir,
    config: WorkloadConfig,
    configured: Arc<ConfiguredBackend>,
    kube: Option<kube::Client>,
}
async fn provider(docker: bool) -> Result<Provider> {
    let root = tempfile::Builder::new()
        .prefix("attune-owning-sensor-")
        .tempdir()?;
    let runner = PathBuf::from(required("ATTUNE_ISOLATION_TEST_RUNNER_PATH")?);
    let echo = PathBuf::from(required("ATTUNE_ISOLATION_TEST_ECHO_PATH")?);
    ensure!(
        runner.is_absolute() && echo.is_absolute() && runner.is_file() && echo.is_file(),
        "once-built absolute Runner/Echo required"
    );
    let platform = match required("ATTUNE_ISOLATION_TEST_PLATFORM")?.as_str() {
        "linux/amd64" => LaunchPlatform::LinuxAmd64,
        "linux/arm64" => LaunchPlatform::LinuxArm64,
        _ => anyhow::bail!("unsupported owned platform"),
    };
    let archive = root.path().join("runner.zip");
    let mut zip = zip::ZipWriter::new(File::create_new(&archive)?);
    zip.start_file(
        format!("linux-{}", platform.native_arch()),
        zip::write::SimpleFileOptions::default().unix_permissions(0o555),
    )?;
    zip.write_all(&std::fs::read(runner)?)?;
    zip.finish()?.sync_all()?;
    std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o444))?;
    let bytes = std::fs::read(&archive)?;
    let helper = PinnedLaunchInput {
        pin_id: 1,
        kind: LaunchInputKind::Runner,
        object_id: 1,
        object_version: format!("sensor-runner-{}", Sha256Digest::of_bytes(&bytes).as_str()),
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::of_bytes(&bytes),
        max_expanded_bytes: 128 << 20,
        max_files: 8,
    };
    let payload = image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE")?;
    let backend = if docker {
        let socket = PathBuf::from(required("ATTUNE_ISOLATION_TEST_DOCKER_SOCKET")?);
        ensure!(socket.is_absolute(), "explicit owned socket required");
        let binding = DockerEngine::connect(&socket)?.observed_binding(1).await?;
        ensure!(
            binding.platform == platform,
            "owned Engine platform mismatch"
        );
        WorkloadBackendConfig::Docker {
            socket,
            cache_volume: required("ATTUNE_ISOLATION_TEST_CACHE_VOLUME")?,
            network: required("ATTUNE_ISOLATION_TEST_NETWORK")?,
            binding,
            pids_limit: 128,
            helper_image: payload.clone(),
        }
    } else {
        WorkloadBackendConfig::Kubernetes {
            cluster: required("ATTUNE_ISOLATION_TEST_CLUSTER")?,
            namespace: required("ATTUNE_ISOLATION_TEST_NAMESPACE")?,
            helper_image: image("ATTUNE_ISOLATION_TEST_HELPER_IMAGE")?,
        }
    };
    let config = WorkloadConfig {
        image: payload,
        platform,
        cache_root: if docker {
            PathBuf::from(required("ATTUNE_ISOLATION_TEST_CACHE_ROOT")?)
        } else {
            root.path().join("cache")
        },
        memory_bytes: if docker { 512 << 20 } else { 128 << 20 },
        scratch_bytes: if docker { 128 << 20 } else { 64 << 20 },
        storage: WorkloadStorageLimits {
            compressed_bytes: 64 << 20,
            expanded_bytes: 128 << 20,
            entries: 4096,
            artifacts: Default::default(),
        },
        helper: helper.clone(),
        helper_archive: archive,
        backend,
    };
    config.validate()?;
    let mut kube_client = None;
    let configured = if let WorkloadBackendConfig::Kubernetes {
        cluster,
        namespace,
        helper_image,
    } = &config.backend
    {
        let path = PathBuf::from(required("ATTUNE_ISOLATION_TEST_KUBECONFIG")?);
        ensure!(path.is_absolute(), "explicit owned kubeconfig required");
        let client = kube::Client::try_from(
            kube::Config::from_custom_kubeconfig(
                kube::config::Kubeconfig::read_from(path)?,
                &kube::config::KubeConfigOptions {
                    context: Some(required("ATTUNE_ISOLATION_TEST_CONTEXT")?),
                    ..Default::default()
                },
            )
            .await?,
        )?;
        kube_client = Some(client.clone());
        let cache = Arc::new(CacheStore::open(
            config.cache_root.clone(),
            CacheLimits {
                compressed_bytes: 64 << 20,
                expanded_bytes: 128 << 20,
                files: 4096,
            },
        )?);
        cache.install_input(&helper, &mut File::open(&config.helper_archive)?)?;
        ConfiguredBackend {
            backend: NativeBackend::Kubernetes(Arc::new(KubernetesLauncher::new(
                client,
                KubernetesPolicy {
                    cluster: cluster.clone(),
                    namespace: namespace.clone(),
                    helper_image: helper_image.clone(),
                    allowed_images: BTreeSet::from([config.image.reference.clone()]),
                    memory_bytes: config.memory_bytes,
                    scratch_bytes: config.scratch_bytes,
                },
                cache.clone(),
            ))),
            cache,
            runner: helper,
        }
    } else {
        attune_workload_backend::configuration::configure(&config).await?
    };
    Ok(Provider {
        root,
        config,
        configured: Arc::new(configured),
        kube: kube_client,
    })
}
fn recipe() -> attune_common::runtime_lifecycle::RuntimeRecipe {
    serde_json::from_value(serde_json::json!({"workspace":true,"tools":[{"name":"python","binaries":["/usr/bin/python3"],"args":["--version"],"version_pattern":"Python (?<version>[0-9]+\\.[0-9]+\\.[0-9]+)","primary":true}],"prepare":[{"command":["{tool_python}","-c","from pathlib import Path; Path('recipe-private-marker').write_text('private')"]}],"launch":{"command":["{tool_python}","{source_file}"],"timeout_seconds":240}})).unwrap()
}
fn map_api<T>(result: api_helpers::Result<T>) -> Result<T> {
    result.map_err(|e| anyhow!(e.to_string()))
}
async fn wait_for<F, Fut>(label: &str, mut predicate: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            if predicate().await? {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .with_context(|| format!("owning sensor deadline: {label}"))?
}
async fn run_case(provider: &Provider, use_recipe: bool) -> Result<()> {
    let ctx = map_api(api_helpers::TestContext::new().await)?;
    let ctx = map_api(ctx.with_auth().await)?;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let host = required("ATTUNE_SENSOR_TEST_API_HOST")?;
    ensure!(
        !host.is_empty() && !host.contains(['/', ':', '@']),
        "explicit owned API host required"
    );
    let api = format!("http://{host}:{}", listener.local_addr()?.port());
    let server_stop = CancellationToken::new();
    let stop = server_stop.clone();
    let router = ctx.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
    });
    let result = case_body(&ctx, provider, use_recipe, api).await;
    server_stop.cancel();
    let served = server.await;
    let database_cleanup = map_api(ctx.cleanup().await);
    if let Err(error) = &database_cleanup {
        tracing::error!(%error,"owned sensor fixture cleanup failed");
    }
    result?;
    served??;
    database_cleanup?;
    Ok(())
}
async fn case_body(
    ctx: &api_helpers::TestContext,
    provider: &Provider,
    use_recipe: bool,
    api: String,
) -> Result<()> {
    let pack = PackFixture::new_unique("owning_sensor")
        .create(&ctx.pool)
        .await?;
    let owner = ctx.user.as_ref().context("owning API identity")?.id;
    attune_common::repositories::PackRepository::set_installed_by(&ctx.pool, pack.id, owner)
        .await?;
    let definition = recipe();
    let runtime_config = if use_recipe {
        serde_json::json!({"lifecycle":definition})
    } else {
        serde_json::json!({"interpreter":{"binary":"python3"},"environment":{"env_type":"virtualenv","dir_name":"venv","create_command":["python3","-m","venv","--copies","--without-pip","{env_dir}"],"interpreter_path":"{env_dir}/bin/python3"},"dependencies":{"manifest_file":"input.txt","install_command":["{interpreter}","-c","from pathlib import Path; assert Path('{manifest_path}').read_text()=='owned offline dependency'; Path('{pack_dir}/installed-marker').write_text('prepared')"]}})
    };
    let mut runtime = RuntimeFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "python");
    runtime.execution_config = runtime_config;
    runtime.distributions = serde_json::json!({"verification":{"commands":[{"binary":"python3","args":["--version"],"exit_code":0,"pattern":"Python [0-9]+","priority":1}]}});
    let runtime = runtime.create(&ctx.pool).await?;
    let sensor=SensorFixture::new(Some(pack.id),Some(pack.r#ref.clone()),runtime.id,runtime.r#ref.clone(),"probe").with_entrypoint("probe.py").with_param_schema(serde_json::json!({"large":{"type":"string","secret":true,"required":true},"artifact_id":{"type":"integer","required":true},"recipe":{"type":"boolean","required":true}})).create(&ctx.pool).await?;
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("sensor.{}.output", sensor.r#ref),
            scope: OwnerType::Sensor,
            owner: sensor.r#ref.clone(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::General,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 10,
            name: None,
            description: None,
            content_type: None,
            data: None,
        },
    )
    .await?;
    let sensor=SensorRepository::update(&ctx.pool,sensor.id,UpdateSensorInput{config:Some(serde_json::json!({"large":"x".repeat(BOOTSTRAP_BYTES),"artifact_id":artifact.id,"recipe":use_recipe})),..Default::default()}).await?;
    let trigger = TriggerFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "signal")
        .with_out_schema(serde_json::json!({"message":{"type":"string"}}))
        .create(&ctx.pool)
        .await?;
    let trigger = TriggerRepository::update(
        &ctx.pool,
        trigger.id,
        UpdateTriggerInput {
            sensor: Some(Patch::Set(sensor.id)),
            sensor_ref: Some(Patch::Set(sensor.r#ref.clone())),
            ..Default::default()
        },
    )
    .await?;
    let action = ActionFixture::new(pack.id, &pack.r#ref, "noop")
        .with_runtime(runtime.id)
        .with_entrypoint("noop.py")
        .create(&ctx.pool)
        .await?;
    let rule = RuleRepository::create(
        &ctx.pool,
        CreateRuleInput {
            r#ref: format!("{}.demand", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "owning sensor demand".into(),
            description: None,
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: serde_json::json!({}),
            action_params: serde_json::json!({}),
            trigger_params: serde_json::json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: Some(owner),
        },
    )
    .await?;
    let source = provider
        .root
        .path()
        .join(format!("source-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(source.join("sensors"))?;
    std::fs::create_dir_all(source.join("actions"))?;
    std::fs::write(
        source.join("actions/noop.yaml"),
        serde_json::to_vec(
            &serde_json::json!({"ref":action.r#ref,"runner_type":"python","entry_point":"noop.py"}),
        )?,
    )?;
    std::fs::write(
        source.join("actions/noop.py"),
        b"# rule demand only; never executed by this sensor fixture\n",
    )?;
    std::fs::create_dir_all(source.join("runtimes"))?;
    std::fs::write(
        source.join("pack.yaml"),
        format!("ref: {}\nversion: {}\n", pack.r#ref, pack.version),
    )?;
    std::fs::write(
        source.join("runtimes/python.yaml"),
        serde_json::to_vec(
            &serde_json::json!({"ref":runtime.r#ref,"name":runtime.name,"aliases":runtime.aliases,"execution_config":runtime.execution_config,"distributions":runtime.distributions}),
        )?,
    )?;
    std::fs::write(
        source.join("sensors/probe.yaml"),
        serde_json::to_vec(
            &serde_json::json!({"ref":sensor.r#ref,"entrypoint":sensor.entrypoint,"runtime":runtime.r#ref,"param_schema":sensor.param_schema}),
        )?,
    )?;
    std::fs::write(source.join("sensors/probe.py"), SOURCE)?;
    std::fs::write(source.join("input.txt"), b"owned offline dependency")?;
    let publication = ctx.test_packs_dir.clone();
    let pack_ref = pack.r#ref.clone();
    let version = pack.version.clone();
    let published = tokio::task::spawn_blocking(move || {
        PackStorage::new(publication).publish_release(&source, &pack_ref, &version)
    })
    .await??;
    let key = ObjectKey::new(format!("packs/blobs/{}.tar.gz", published.digest))?;
    let mut digest = [0; 32];
    for (i, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&published.digest[i * 2..i * 2 + 2], 16)?;
    }
    ObjectMaintenanceRepository::reserve_upload(&ctx.pool, key.as_str(), "pack").await?;
    let stored = ctx
        .state
        .blob_store
        .put(&key, body_from_file(&published.archive_path).await?, digest)
        .await?;
    ensure!(
        stored.size == published.archive_size && stored.sha256 == digest,
        "primary upload receipt mismatch"
    );
    let mut tx = ctx.pool.begin().await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version,
            digest: published.digest,
            object_key: key.as_str().into(),
            provider_version: stored.provider_version.as_stored().into(),
            content_path: published.pack_path.to_string_lossy().into_owned(),
            archive_size: published.archive_size as i64,
            manifest: serde_json::to_value(published.manifest)?,
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            rules: vec![rule.id],
            actions: vec![action.id],
            runtimes: vec![runtime.id],
            sensors: vec![sensor.id],
            triggers: vec![trigger.id],
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    let master = ctx
        .state
        .config
        .security
        .encryption_key
        .clone()
        .context("fixture encryption key")?;
    let mut tx = ctx.pool.begin().await?;
    ManagedSensorMetadataRepository::lock_mutation(&mut tx).await?;
    let current = SensorRepository::find_by_id(&mut *tx, sensor.id)
        .await?
        .context("published sensor")?;
    ComponentConfigurationRepository::bind_sensor(
        &mut tx,
        &current,
        current.config.as_ref(),
        Some(&master),
    )
    .await?;
    ManagedSensorMetadataRepository::readmit(&mut tx, &current, owner, Some(&master)).await?;
    tx.commit().await?;
    let tokens = Arc::new(WorkerTokenProvider::new(
        owner,
        "0",
        attune_common::auth::jwt::JwtConfig {
            secret: ctx
                .state
                .config
                .security
                .jwt_secret
                .clone()
                .context("fixture signing key")?,
            ..Default::default()
        },
    ));
    let captured = ChildProcessEnvironment::from_parent(
        [(
            "ATTUNE_RUNTIME_RECIPE_DIGESTS".into(),
            if use_recipe {
                definition.digest()?
            } else {
                String::new()
            }
            .into(),
        )],
        &[],
    )?
    .with_workload_sandbox(attune_common::workload_sandbox::WorkloadSandbox::NativeEffectsRequired);
    let cache_dir = provider
        .root
        .path()
        .join(format!("manager-{}", uuid::Uuid::new_v4()));
    let mut config: Config = ctx.state.config.as_ref().clone();
    config.workload = Some(provider.config.clone());
    config.sensor.as_mut().context("sensor config")?.worker_name =
        Some(format!("owning-sensor-{}", uuid::Uuid::new_v4()));
    let mut registration =
        crate::sensor_worker_registration::SensorWorkerRegistration::new(ctx.pool.clone(), &config)
            .with_incarnation(tokens.instance_id())
            .with_child_environment(captured.clone());
    let worker = registration.register(&config).await?;
    tokens.set_worker_id(worker.to_string());
    let transport = Arc::new(ApiTransport::new_with_worker_token_provider(
        &api,
        tokens.clone(),
        cache_dir
            .join("artifacts")
            .to_str()
            .context("artifact directory")?,
    )?);
    let manager = SensorManager::new(
        ctx.pool.clone(),
        SensorManagerConfig {
            native: Some(provider.configured.clone()),
            workload_config: Some(provider.config.clone()),
            encryption_key: Some(master.clone()),
            child_environment: captured,
            api_url: api.clone(),
            worker_token_provider: Some(tokens.clone()),
            notifier_ws_url: "ws://127.0.0.1:0".into(),
            allow_insecure_notifier_ws: false,
            mq_url: String::new(),
            log_level: "warn".into(),
            log_format: "json".into(),
            packs_base_dir: cache_dir.join("packs").to_string_lossy().into_owned(),
            runtime_envs_dir: cache_dir.join("runtimes").to_string_lossy().into_owned(),
            pack_transport: Arc::new(
                attune_common::pack_transport::ApiPackTransport::new_with_worker_token_provider(
                    &api,
                    tokens.clone(),
                    cache_dir.join("packs").to_str().context("pack cache")?,
                )?,
            ),
            artifact_transport: transport,
            sensor_log_config: crate::sensor_log::SensorLogConfig {
                segment_writer: attune_common::log_stream::SegmentedLogConfig {
                    initial_segment_bytes: 1024,
                    max_segment_bytes: 16384,
                    flush_interval_ms: 10,
                    ..ctx.state.config.artifacts.log_segment_writer_config()
                },
                ..Default::default()
            },
            mirror_runtime_stdout_to_stdio: false,
            mirror_runtime_stderr_to_stdio: false,
            worker_name: registration.worker_name().into(),
        },
    );
    manager.set_worker_id(worker);
    let heartbeat_stop = CancellationToken::new();
    let cancel = heartbeat_stop.clone();
    let pool = ctx.pool.clone();
    let incarnation = tokens.instance_id();
    let heartbeat = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {_=cancel.cancelled()=>return Ok::<_,attune_common::Error>(()),_=ticker.tick()=>{}}
            WorkerRepository::heartbeat_incarnation(&pool, worker, incarnation).await?;
        }
    });
    let mut observed = None;
    let result = async {
        registration
            .discover_native_capabilities(&config, &manager)
            .await?;
        registration.update_capabilities().await?;
        let live = WorkerRepository::find_by_id(&ctx.pool, worker)
            .await?
            .context("registered sensor worker")?;
        ensure!(
            live.meta.as_ref().context("registration metadata")?["registration_incarnation"]
                == incarnation.to_string(),
            "incarnation mismatch"
        );
        ensure!(
            live.capabilities.as_ref().context("capabilities")?["runtimes"]
                .as_array()
                .context("runtime names")?
                .iter()
                .any(|n| n == "python"),
            "actual native discovery did not prove Python"
        );
        manager.start().await?;
        let (workload, spec, resource) = {
            let instances = manager.inner.sensors.read().await;
            let instance = instances
                .get(&sensor.id)
                .context("actual SensorManager did not start sensor")?;
            let native = instance
                .native
                .as_ref()
                .context("sensor used a residual local launcher")?;
            (
                instance.workload.clone(),
                native.context.spec.clone(),
                native
                    .context
                    .resource
                    .get()
                    .context("native resource unconfirmed")?
                    .clone(),
            )
        };
        observed = Some((workload.workload_id, spec.clone(), resource.clone()));
        ensure!(
            spec.stage.is_none() && spec.pack_access == PackAccess::ReadOnly,
            "sensor mount policy changed"
        );
        ensure!(
            spec.inputs
                .iter()
                .any(|p| p.kind == LaunchInputKind::Runtime)
                != use_recipe,
            "private recipe/runtime input policy mismatch"
        );
        let retained = Ledger::native_effect(&ctx.pool, workload.workload_id)
            .await?
            .context("missing real retained effect")?;
        ensure!(
            retained.resource.as_ref() == Some(&resource) && retained.stdin_consumed,
            "resource/stdin was not genuinely admitted"
        );
        let decoded: Context = serde_json::from_str(&attune_common::crypto::decrypt(
            &retained.encrypted_context,
            &master,
        )?)?;
        ensure!(
            decoded.bootstrap_json.len() > 8 * 1024 * 1024,
            "bootstrap did not cross general stdin ceiling"
        );
        let client = reqwest::Client::new();
        wait_for("two actual current-generation uploaded files", || async {
            let versions =
                ArtifactVersionRepository::list_by_artifact(&ctx.pool, artifact.id).await?;
            Ok(versions.len() == 2
                && versions.iter().all(|v| {
                    v.body_state == Some(attune_common::models::enums::ArtifactBodyState::Ready)
                }))
        })
        .await?;
        let versions = ArtifactVersionRepository::list_by_artifact(&ctx.pool, artifact.id).await?;
        for version in &versions {
            ensure!(
                ArtifactVersionRepository::sensor_native_file_binding(&ctx.pool, version.id)
                    .await?
                    == Some((
                        workload.workload_id,
                        workload.generation,
                        spec.intent,
                        worker,
                        incarnation
                    )),
                "ordinary output is not the actual admitted generation/intent"
            );
        }
        // Wait for both real, separately allocated log streams before stop.
        wait_for("separate log segments", || async {
            for stream in ["stdout", "stderr"] {
                let a = ArtifactRepository::find_by_ref(
                    &ctx.pool,
                    &format!("sensor.{}.{stream}", sensor.r#ref),
                )
                .await?;
                let Some(a) = a else { return Ok(false) };
                let Some(v) = ArtifactVersionRepository::find_latest(&ctx.pool, a.id).await? else {
                    return Ok(false);
                };
                let Some(log) =
                    LogStreamRepository::find_by_artifact_version(&ctx.pool, v.id).await?
                else {
                    return Ok(false);
                };
                if LogStreamRepository::segments(&ctx.pool, log.id)
                    .await?
                    .is_empty()
                {
                    return Ok(false);
                }
            }
            Ok(true)
        })
        .await?;
        manager.stop().await?;
        ensure!(
            manager.inner.background_tasks.lock().await.is_empty(),
            "manager tasks not joined"
        );
        ensure!(
            Ledger::native_effect(&ctx.pool, workload.workload_id)
                .await?
                .is_none(),
            "capture/file obligations were cleared without acknowledgement or remained incomplete"
        );
        match (&provider.configured.backend, &resource) {
            (NativeBackend::Docker(backend), _) => ensure!(
                backend.removal_recorded(&spec, &resource).await?,
                "Docker removal lacks its exact positive adapter receipt"
            ),
            (
                NativeBackend::Kubernetes(_),
                ResourceIdentity::Kubernetes {
                    namespace,
                    pod_name,
                    ..
                },
            ) => {
                let pods: kube::Api<k8s_openapi::api::core::v1::Pod> = kube::Api::namespaced(
                    provider.kube.clone().context("owned Kubernetes client")?,
                    namespace,
                );
                // Absence is checked only AFTER positive production stop,
                // capture, API receipts and acknowledged ledger clearance.
                ensure!(
                    pods.get_opt(pod_name).await?.is_none(),
                    "acknowledged removal left the owned Pod present"
                );
            }
            _ => anyhow::bail!("provider resource mismatch"),
        }
        for version in &versions {
            let receipt: Option<i64> = sqlx::query_scalar(
                "SELECT sensor_publication_receipt_bytes FROM artifact_version WHERE id=$1",
            )
            .bind(version.id)
            .fetch_one(&ctx.pool)
            .await?;
            ensure!(
                receipt == version.size_bytes,
                "missing exact positive ordinary file receipt"
            );
        }
        ensure!(
            versions.iter().any(|v| v.size_bytes == Some(0))
                && versions
                    .iter()
                    .any(|v| v.size_bytes == Some(OUTPUT.len() as i64)),
            "empty/nonempty receipt coverage missing"
        );
        for (stream, needle, other) in [
            (
                "stdout",
                "sensor-stdout-bootstrap:9437184",
                "sensor-stderr-separate",
            ),
            (
                "stderr",
                "sensor-stderr-separate",
                "sensor-stdout-bootstrap",
            ),
        ] {
            let a = ArtifactRepository::find_by_ref(
                &ctx.pool,
                &format!("sensor.{}.{stream}", sensor.r#ref),
            )
            .await?
            .context("log artifact")?;
            let v = ArtifactVersionRepository::find_latest(&ctx.pool, a.id)
                .await?
                .context("log version")?;
            ensure!(
                a.classification == ArtifactClassification::RuntimeLog,
                "ordinary/log contracts mixed"
            );
            let (binding, _) = SensorSchemaRepository::find_log_generation(&ctx.pool, v.id)
                .await?
                .context("log generation binding")?;
            let expected =
                SensorSchemaRepository::find(&ctx.pool, workload.workload_id, workload.generation)
                    .await?
                    .context("admitted log generation")?;
            ensure!(binding == expected.0, "log generation differs");
            let log = LogStreamRepository::find_by_artifact_version(&ctx.pool, v.id)
                .await?
                .context("log stream")?;
            ensure!(
                log.sealed && !log.truncated,
                "capture was not positively sealed"
            );
            let response = client
                .get(format!(
                    "{api}/api/v1/sensors/{}/logs/{stream}",
                    sensor.r#ref
                ))
                .bearer_auth(ctx.token().context("viewer token")?)
                .send()
                .await?;
            ensure!(
                response.status().is_success(),
                "authorized sensor log read failed: {}",
                response.status()
            );
            let text = response.text().await?;
            ensure!(
                text.contains(needle) && !text.contains(other),
                "separate logs/9MiB forwarding failed"
            );
            for segment in LogStreamRepository::segments(&ctx.pool, log.id).await? {
                let bytes = ctx
                    .state
                    .blob_store
                    .get(
                        &ObjectKey::new(segment.object_key)?,
                        &ProviderVersion::from_stored(segment.provider_version)?,
                        None,
                    )
                    .await?
                    .try_fold(Vec::new(), |mut out, b| async move {
                        out.extend_from_slice(&b);
                        Ok(out)
                    })
                    .await?;
                ensure!(
                    !bytes.windows(needle.len()).any(|b| b == needle.as_bytes()),
                    "runtime log blob contains plaintext"
                );
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // Always stop/join the real manager and its capture owners before closing
    // the API/database. Failure-only provider teardown remains exact-resource.
    let stopped = manager.stop().await;
    let cleanup = async {
        // start() can fail after recording an exact resource but before adding
        // an instance. Recover only this registered instance's retained fact.
        if observed.is_none() {
            if let Some((workload, effect)) =
                Ledger::unsettled_native_for_sensor(&ctx.pool, sensor.id, incarnation).await?
            {
                let clear = attune_common::crypto::decrypt(&effect.encrypted_context, &master)?;
                let context: Context = serde_json::from_str(&clear)?;
                let spec = context.spec();
                ensure!(
                    spec.intent == effect.intent
                        && matches!(spec.target,
                    LaunchTarget::Sensor {sensor: id, workload: run, generation, ..}
                    if id == sensor.id && run == workload && generation == effect.generation),
                    "owned failure cleanup binding mismatch"
                );
                let resource = effect
                    .resource
                    .context("unknown sensor create retained; cannot claim provider cleanup")?;
                // This fixture uses the workload ID only for its retained-row
                // check below; never rebind or clear the publication ledger.
                observed = Some((workload, spec, resource));
            }
        }
        if let Some((workload, spec, resource)) = &observed {
            if Ledger::native_effect(&ctx.pool, *workload).await?.is_some() {
                provider
                    .configured
                    .backend
                    .stop_retained(spec, resource, LaunchStopReason::Shutdown)
                    .await?;
                provider
                    .configured
                    .backend
                    .remove_stopped(spec, resource)
                    .await?;
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    heartbeat_stop.cancel();
    let heartbeat_result = heartbeat.await;
    let deregistered = registration.deregister().await;
    drop(manager);
    result?;
    stopped?;
    cleanup?;
    heartbeat_result??;
    deregistered?;
    Ok(())
}

fn close_root(root: tempfile::TempDir) -> Result<()> {
    fn writable(path: &Path) -> std::io::Result<()> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.is_dir() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            for child in std::fs::read_dir(path)? {
                writable(&child?.path())?;
            }
        }
        Ok(())
    }
    writable(root.path())?;
    root.close()?;
    Ok(())
}
async fn owning(docker: bool) -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let provider = provider(docker).await?;
    let result = async {
        run_case(&provider, false).await?;
        run_case(&provider, true).await
    }
    .await;
    let Provider {
        root, configured, ..
    } = provider;
    drop(configured);
    let cleanup = close_root(root);
    result?;
    cleanup
}
#[tokio::test]
#[ignore = "requires explicit owned Docker socket/cache/network, pinned once-built images/Runner/Echo, reachable API host and owned PostgreSQL"]
async fn docker_native_sensor() -> Result<()> {
    owning(true).await
}
#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace/context/kubeconfig, pinned once-built images/Runner/Echo, reachable API host and owned PostgreSQL"]
async fn kubernetes_native_sensor() -> Result<()> {
    owning(false).await
}

fn lazy_manager() -> Result<SensorManager> {
    attune_common::auth::install_crypto_provider();
    let pool =
        sqlx::postgres::PgPoolOptions::new().connect_lazy("postgresql://localhost/unused-no-db")?;
    Ok(SensorManager::new(
        pool,
        SensorManagerConfig {
            native: None,
            workload_config: None,
            encryption_key: None,
            child_environment: ChildProcessEnvironment::from_parent([], &[])?,
            api_url: "http://127.0.0.1:1".into(),
            worker_token_provider: None,
            notifier_ws_url: "ws://127.0.0.1:1".into(),
            allow_insecure_notifier_ws: false,
            mq_url: String::new(),
            log_level: "warn".into(),
            log_format: "json".into(),
            packs_base_dir: "/unused".into(),
            runtime_envs_dir: "/unused".into(),
            pack_transport: Arc::new(attune_common::pack_transport::VolumePackTransport::new(
                "/unused",
            )),
            artifact_transport: Arc::new(attune_common::artifact_transport::VolumeTransport::new(
                "/unused",
            )),
            sensor_log_config: Default::default(),
            mirror_runtime_stdout_to_stdio: false,
            mirror_runtime_stderr_to_stdio: false,
            worker_name: "no-db-shutdown".into(),
        },
    ))
}
#[tokio::test]
async fn stop_joins_background_waiting_for_lifecycle_gate_and_propagates_panic() -> Result<()> {
    let manager = lazy_manager()?;
    *manager.inner.running.write().await = true;
    let gate = manager.inner.lifecycle_gate.write().await;
    let mut stopping = Box::pin(manager.stop());
    // Poll until shutdown has queued its writer behind our held guard. The
    // background reader below must queue behind that shutdown writer.
    ensure!(futures::poll!(stopping.as_mut()).is_pending());
    let owner = manager.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    manager
        .inner
        .background_tasks
        .lock()
        .await
        .push(tokio::spawn(async move {
            entered.send(()).unwrap();
            let _read = owner.inner.lifecycle_gate.read().await;
            assert!(!*owner.inner.running.read().await);
        }));
    ready.await?;
    drop(gate);
    tokio::time::timeout(Duration::from_secs(2), stopping).await??;
    ensure!(
        manager.inner.background_tasks.lock().await.is_empty(),
        "background owner not joined"
    );
    manager
        .inner
        .background_tasks
        .lock()
        .await
        .push(tokio::spawn(async {
            panic!("owned test background failure")
        }));
    ensure!(
        manager.stop().await.is_err(),
        "background panic became successful stop"
    );
    Ok(())
}
#[tokio::test]
async fn failed_stop_still_joins_waiter_and_both_capture_owners() -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Finished(Arc<AtomicUsize>);
    impl Drop for Finished {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let joined = Arc::new(AtomicUsize::new(0));
    let mut captures = Vec::new();
    for _ in 0..2 {
        let finished = Finished(joined.clone());
        captures.push(Some(tokio::spawn(async move {
            let _finished = finished;
            std::future::pending::<()>().await;
        })));
    }
    let finished = Finished(joined.clone());
    let mut waiter = Some(tokio::spawn(async move {
        let _finished = finished;
        std::future::pending::<attune_common::Result<LaunchExitOutcome>>().await
    }));
    let (a, b) = captures.split_at_mut(1);
    let (first, second) = tokio::join!(
        super::super::drain_sensor_capture(&mut a[0], Duration::ZERO),
        super::super::drain_sensor_capture(&mut b[0], Duration::ZERO)
    );
    ensure!(
        first.is_err() && second.is_err(),
        "aborted capture became EOF"
    );
    ensure!(
        join_native_startup_stop(
            async { Err(anyhow!("owned durable stop failure")) },
            &mut waiter,
            &mut None,
            &mut None,
        )
        .await
        .is_err(),
        "stop failure became success"
    );
    ensure!(
        joined.load(Ordering::SeqCst) == 3
            && waiter.is_none()
            && captures.iter().all(Option::is_none),
        "an owned waiter/capture escaped teardown"
    );
    Ok(())
}
