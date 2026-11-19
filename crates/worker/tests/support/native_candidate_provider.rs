//! Actual candidate handler acceptance on the existing owned provider lane.
//! No process, transport, API or repository effects are substituted.
#[path = "../../../api/tests/helpers.rs"]
mod api_helpers;
#[path = "../../../common/tests/helpers.rs"]
mod helpers;

use super::handle;
use crate::{artifacts::ArtifactManager, runtime::RuntimeRegistry, ActionExecutor, SecretManager};
use anyhow::{ensure, Context, Result};
use attune_common::{
    artifact_transport::ApiTransport,
    auth::{hash_integration_token, jwt::JwtConfig, WorkerTokenProvider},
    blob_store::{body_from_file, hash_file, ObjectKey},
    child_process_environment::ChildProcessEnvironment,
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    models::{Pack, PackInstall, Runtime, WorkerType},
    mq::PackTestRequestedPayload,
    pack_registry::storage::PackStorage,
    repositories::{
        candidate_effect::CandidateEffectRepository,
        candidate_process::{CandidateCapture, CandidateSource},
        component_lifecycle::PackProjectionIds,
        execution_process::ExecutionProcessRepository,
        object_maintenance::ObjectMaintenanceRepository,
        pack::PackRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::{CreateWorkerInput, WorkerRepository},
        Create, PackInstallRepository, PackTestRepository,
    },
    test_executor::TestProcessInterruption,
    workload_config::{WorkloadBackendConfig, WorkloadConfig, WorkloadStorageLimits},
    workload_launch::*,
    workload_sandbox::WorkloadSandbox,
};
use attune_workload_backend::{
    cache::{CacheLimits, CacheStore},
    configuration::{ConfiguredBackend, NativeBackend},
    engine::DockerEngine,
    kubernetes::{KubernetesLauncher, KubernetesPolicy},
};
use helpers::{PackFixture, RuntimeFixture};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Json;
use std::{
    collections::BTreeSet,
    fs::File,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn required(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing owning candidate prerequisite: {name}"))
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
        .prefix("attune-owning-candidate-")
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
        object_version: format!(
            "candidate-runner-{}",
            Sha256Digest::of_bytes(&bytes).as_str()
        ),
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

#[derive(Clone, Copy, Debug)]
enum Case {
    Ordinary,
    MixedRecipe,
    PreparationFailure,
    MetadataOnly,
    NonzeroIgnored,
    ApiOptIn,
}
impl Case {
    fn opt_in(self) -> bool {
        matches!(self, Self::ApiOptIn)
    }
    fn fails_preparation(self) -> bool {
        matches!(self, Self::PreparationFailure)
    }
}
fn recipe() -> attune_common::runtime_lifecycle::RuntimeRecipe {
    serde_json::from_value(json!({
        "workspace":true,
        "tools":[{"name":"python","binaries":["/usr/bin/python3"],"args":["--version"],
            "version_pattern":"Python (?<version>[0-9]+\\.[0-9]+\\.[0-9]+)","primary":true}],
        "prepare":[{"command":["{tool_python}","-c",
            "import os; from pathlib import Path; assert not any(k.startswith('ATTUNE_LAUNCH_') for k in os.environ); assert 'ATTUNE_CANDIDATE_EFFECTS_TOKEN' not in os.environ; Path('recipe-private-marker').write_text('private')"]}],
        "launch":{"command":["{tool_python}","{source_file}"],"timeout_seconds":60}
    })).unwrap()
}
fn python_definition(case: Case) -> Value {
    let dependency = if case.fails_preparation() {
        "import sys; sys.exit(7)"
    } else {
        "import os,sys; from pathlib import Path; assert not any(k.startswith('ATTUNE_LAUNCH_') for k in os.environ); assert 'ATTUNE_CANDIDATE_EFFECTS_TOKEN' not in os.environ; assert 'ATTUNE_API_TOKEN' not in os.environ; assert Path('{manifest_path}').read_text()=='owned offline dependency'; Path(sys.prefix,'prepared-proof').write_text('private'); Path('{pack_dir}/dependency-prepared').write_text('private')"
    };
    json!({
        "interpreter":{"binary":"python3"},
        "environment":{"env_type":"virtualenv","dir_name":"venv",
            "create_command":["python3","-m","venv","--copies","--without-pip","{env_dir}"],
            "interpreter_path":"{env_dir}/bin/python3"},
        "dependencies":{"manifest_file":"input.txt","install_command":["{interpreter}","-c",dependency]}
    })
}
fn testing(case: Case) -> Value {
    let second = if matches!(case, Case::MixedRecipe) {
        json!({"type":"runtime","entry_point":"tests/recipe.py","timeout":60,"lifecycle":recipe()})
    } else if matches!(case, Case::MetadataOnly) {
        json!({"type":"script","entry_point":"tests/missing.sh","timeout":60})
    } else {
        json!({"type":"script","entry_point":"tests/check.sh","timeout":60})
    };
    let python_entry = if matches!(case, Case::MetadataOnly) {
        "tests/missing.py"
    } else {
        "tests/test_native.py"
    };
    json!({"enabled":true,"candidate_api_effects":case.opt_in(),
        "discovery":{"method":"explicit"},
        "runners":{"python":{"type":"unittest","entry_point":python_entry,"timeout":60},"second":second},
        "on_failure":if matches!(case,Case::PreparationFailure|Case::MetadataOnly|Case::NonzeroIgnored){"ignore"}else{"block"}})
}
const PYTHON_TEST: &str = r#"import json,os,subprocess,sys,unittest,urllib.request
from pathlib import Path
class NativeCandidate(unittest.TestCase):
    def test_private_python(self):
        assert sys.prefix.startswith('/tmp/attune-candidate-python-'), sys.prefix
        assert Path(sys.prefix,'prepared-proof').read_text()=='private'
        assert Path('dependency-prepared').read_text()=='private'
        for name in ['DATABASE_URL','JWT_SECRET','ENCRYPTION_KEY','ATTUNE_API_TOKEN','ATTUNE_RUNTIME_RECIPE_DIGESTS','ATTUNE__DUMMY_IMAGE_CREDENTIAL']:
            assert name not in os.environ, name
        assert not any(k.startswith('ATTUNE_LAUNCH_') for k in os.environ)
        assert not Path('/var/run/docker.sock').exists()
        assert not Path('/opt/attune/runtime').exists()
        assert sys.stdin.read()==''
        assert not Path('/var/run/secrets/kubernetes.io/serviceaccount/token').exists()
        assert ('ATTUNE_CANDIDATE_EFFECTS_TOKEN' in os.environ)==OPT_IN
        if OPT_IN:
            operation={'operation':'put_key','target':PACK_REF+'.proof','value':'isolated'}
            request=urllib.request.Request(os.environ['ATTUNE_CANDIDATE_EFFECTS_URL'],data=json.dumps(operation).encode(),headers={'Content-Type':'application/json','x-attune-candidate-effects':os.environ['ATTUNE_CANDIDATE_EFFECTS_TOKEN']},method='POST')
            with urllib.request.urlopen(request,timeout=30) as response:
                assert response.status==200
        echo=subprocess.run(['/usr/local/libexec/attune/echo'],input=json.dumps({'message':'candidate-python-private'}),text=True,capture_output=True,check=True)
        assert 'candidate-python-private' in echo.stdout
        print('candidate-python-private',flush=True)
"#;
const RECIPE_TEST: &str = r#"import os,sys
from pathlib import Path
assert not sys.prefix.startswith('/tmp/attune-candidate-python-')
assert not os.environ['PATH'].split(':')[0].startswith('/tmp/attune-candidate-python-')
assert Path('recipe-private-marker').read_text()=='private'
assert Path('dependency-prepared').read_text()=='private'
assert not any(k.startswith('ATTUNE_LAUNCH_') for k in os.environ)
assert 'ATTUNE_CANDIDATE_EFFECTS_TOKEN' not in os.environ
assert 'ATTUNE_API_TOKEN' not in os.environ
print('candidate-recipe-private',flush=True)
"#;
fn write_candidate(source: &Path, pack: &Pack, case: Case) -> Result<()> {
    std::fs::create_dir_all(source.join("tests"))?;
    std::fs::write(
        source.join("pack.yaml"),
        serde_json::to_vec(&json!({
            "ref":pack.r#ref,"version":pack.version,"testing":testing(case)
        }))?,
    )?;
    let python = format!(
        "OPT_IN={}\nPACK_REF={}\n{}",
        if case.opt_in() { "True" } else { "False" },
        serde_json::to_string(&pack.r#ref)?,
        PYTHON_TEST
    );
    std::fs::write(source.join("tests/test_native.py"), python)?;
    std::fs::write(source.join("tests/__init__.py"), b"")?;
    std::fs::write(source.join("tests/recipe.py"), RECIPE_TEST)?;
    let script = format!("set -eu\npython3 -c \"import sys; from pathlib import Path; assert sys.prefix.startswith('/tmp/attune-candidate-python-'); assert Path(sys.prefix,'prepared-proof').read_text()=='private'\"\nprintf '%s' '{{\"message\":\"candidate-script-private\"}}' | /usr/local/libexec/attune/echo\n{}\n", if matches!(case,Case::NonzeroIgnored){"exit 7"}else{"exit 0"});
    std::fs::write(source.join("tests/check.sh"), script)?;
    std::fs::write(source.join("input.txt"), b"owned offline dependency")?;
    Ok(())
}
fn map_api<T>(result: api_helpers::Result<T>) -> Result<T> {
    result.map_err(|e| anyhow::anyhow!(e.to_string()))
}

async fn publish_python(
    ctx: &api_helpers::TestContext,
    case: Case,
) -> Result<(Runtime, i64, String)> {
    let pack = PackFixture::new("core").create(&ctx.pool).await?;
    PackRepository::set_installed_by(
        &ctx.pool,
        pack.id,
        ctx.user.as_ref().context("API identity")?.id,
    )
    .await?;
    let mut runtime = RuntimeFixture::new(Some(pack.id), Some(pack.r#ref.clone()), "python");
    runtime.execution_config = python_definition(case);
    runtime.distributions = json!({"verification":{"always_available":true}});
    let runtime = runtime.create(&ctx.pool).await?;
    let source = ctx.test_packs_dir.join("provider-source");
    std::fs::create_dir_all(source.join("runtimes"))?;
    std::fs::write(
        source.join("pack.yaml"),
        format!("ref: core\nversion: {}\n", pack.version),
    )?;
    std::fs::write(
        source.join("runtimes/python.yaml"),
        serde_json::to_vec(&json!({
            "ref":"core.python","name":runtime.name,"aliases":runtime.aliases,
            "execution_config":runtime.execution_config,"distributions":runtime.distributions
        }))?,
    )?;
    let publication = ctx.test_packs_dir.clone();
    let version = pack.version.clone();
    let published = tokio::task::spawn_blocking(move || {
        PackStorage::new(publication).publish_release(&source, "core", &version)
    })
    .await??;
    let key = ObjectKey::new(format!("packs/blobs/{}.tar.gz", published.digest))?;
    let (_, digest) = hash_file(&published.archive_path).await?;
    ObjectMaintenanceRepository::reserve_upload(&ctx.pool, key.as_str(), "pack").await?;
    let stored = ctx
        .state
        .blob_store
        .put(&key, body_from_file(&published.archive_path).await?, digest)
        .await?;
    ensure!(
        stored.size == published.archive_size && stored.sha256 == digest,
        "provider upload receipt differs"
    );
    let mut tx = ctx.pool.begin().await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref,
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
            runtimes: vec![runtime.id],
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok((runtime, release.id, release.digest))
}

async fn publish_candidate(
    ctx: &api_helpers::TestContext,
    pack: &Pack,
    case: Case,
    worker: i64,
) -> Result<(PackInstall, PackTestRequestedPayload)> {
    let repository = PackInstallRepository::new(ctx.pool.clone());
    let install = repository
        .create(
            &pack.r#ref,
            &pack.version,
            "manual",
            Some(pack.id),
            ctx.user.as_ref().map(|user| user.id),
        )
        .await?;
    let source = ctx
        .test_packs_dir
        .join(format!(".pack-test-{}", install.id));
    write_candidate(&source, pack, case)?;
    // Exact actual candidate API gzip/TAR contract, not the primary release
    // manifest format. Publication uses the real owned object store and ledger.
    let archive = tempfile::NamedTempFile::new()?;
    let encoder = flate2::write::GzEncoder::new(archive.reopen()?, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    builder.append_dir_all(&pack.r#ref, &source)?;
    builder.finish()?;
    builder.into_inner()?.finish()?.sync_all()?;
    let key = ObjectKey::new(format!(
        "packs/candidates/{}/{}.tar.gz",
        install.id,
        Uuid::new_v4()
    ))?;
    let (size, digest) = hash_file(archive.path()).await?;
    ObjectMaintenanceRepository::reserve_upload(&ctx.pool, key.as_str(), "pack").await?;
    let stored = ctx
        .state
        .blob_store
        .put(&key, body_from_file(archive.path()).await?, digest)
        .await?;
    ensure!(
        stored.size == size && stored.sha256 == digest,
        "candidate upload receipt differs"
    );
    let mut tx = ctx.pool.begin().await?;
    ObjectMaintenanceRepository::record_uploaded(
        &mut *tx,
        key.as_str(),
        stored.provider_version.as_stored(),
        size as i64,
    )
    .await?;
    ensure!(PackInstallRepository::attach_candidate_object(&mut tx, install.id, &stored).await?);
    tx.commit().await?;
    let token = Uuid::new_v4().to_string();
    let install = repository
        .claim_worker(install.id, worker, Some(&hash_integration_token(&token)))
        .await?
        .context("candidate worker claim")?;
    let payload = PackTestRequestedPayload {
        pack_install_id: install.id,
        pack_ref: pack.r#ref.clone(),
        pack_version: pack.version.clone(),
        candidate_path: Some(source.to_string_lossy().into_owned()),
        candidate_access_token: Some(token),
        trigger_reason: "manual".into(),
        required_runtimes: vec!["python".into()],
        required_recipe_digests: if matches!(case, Case::MixedRecipe) {
            vec![recipe().digest()?]
        } else {
            vec![]
        },
        worker_selector: json!({}),
        worker_tolerations: json!([]),
        worker_affinity: json!({}),
    };
    // A source replica's staging directory is unavailable to the worker.
    std::fs::remove_dir_all(source)?;
    Ok((install, payload))
}

// Test-only inspection of the actual encrypted candidate context. This never
// reserves a replacement intent or fabricates provider/capture facts.
#[derive(Deserialize)]
struct StoredContext {
    source: CandidateSource,
    intent: Uuid,
    target: LaunchTarget,
    image: PinnedImage,
    platform: LaunchPlatform,
    inputs: Vec<PinnedLaunchInput>,
    timeout_ms: Option<u64>,
    invocation: LaunchInvocation,
}
impl StoredContext {
    fn belongs_to(&self, install: i64, intent: Uuid) -> bool {
        self.intent == intent
            && self.source.install == install
            && matches!(self.target,
            LaunchTarget::CandidateTest{install:id,test_job,attempt} if id==install && test_job>0 && attempt==self.source.instance)
    }
    fn spec(&self) -> ProcessSpec {
        ProcessSpec {
            intent: self.intent,
            purpose: LaunchPurpose::CandidateTest,
            target: self.target.clone(),
            image: self.image.clone(),
            platform: self.platform,
            inputs: self.inputs.clone(),
            pack_access: PackAccess::CandidateCopy,
            stage: None,
            timeout_ms: self.timeout_ms,
            invocation: self.invocation.clone(),
        }
    }
}
#[derive(sqlx::FromRow)]
struct Evidence {
    intent: Uuid,
    encrypted_context: String,
    create_submitted: bool,
    stdin_consumed: bool,
    stop_requested: bool,
    resource: Option<Json<ResourceIdentity>>,
    exit_outcome: Option<Json<LaunchExitOutcome>>,
    resource_removed: bool,
    candidate_capture: Option<String>,
}
async fn evidence(ctx: &api_helpers::TestContext, install: i64) -> Result<Vec<Evidence>> {
    Ok(sqlx::query_as("SELECT intent,encrypted_context,create_submitted,stdin_consumed,stop_requested,resource,exit_outcome,resource_removed,candidate_capture FROM execution_process_effect WHERE purpose='candidate_test' AND (target->>'install')::BIGINT=$1 ORDER BY id")
        .bind(install).fetch_all(&ctx.pool).await?)
}
fn retained_capture(value: Option<&str>, master: &str) -> Result<CandidateCapture> {
    let value = value.context("incomplete candidate capture retained")?;
    let capture: CandidateCapture =
        serde_json::from_str(&attune_common::crypto::decrypt(value, master)?)
            .context("unreadable candidate capture retained")?;
    ensure!(
        capture.stdout.len() <= attune_common::test_executor::MAX_TEST_CAPTURE_TEXT_BYTES
            && capture.stderr.len() <= attune_common::test_executor::MAX_TEST_CAPTURE_TEXT_BYTES,
        "unbounded candidate capture retained"
    );
    Ok(capture)
}
async fn cleanup_effects(
    ctx: &api_helpers::TestContext,
    provider: &Provider,
    install: i64,
    master: &str,
) -> Result<()> {
    for row in evidence(ctx, install).await? {
        if !row.create_submitted {
            continue;
        }
        let context: StoredContext = serde_json::from_str(&attune_common::crypto::decrypt(
            &row.encrypted_context,
            master,
        )?)?;
        ensure!(
            context.belongs_to(install, row.intent),
            "cleanup context changed"
        );
        let spec = context.spec();
        if row.resource_removed {
            let capture = retained_capture(row.candidate_capture.as_deref(), master)?;
            ensure!(
                row.stdin_consumed
                    && row.stop_requested
                    && row
                        .exit_outcome
                        .as_ref()
                        .is_some_and(|exit| exit.0.exit_code == capture.exit_code),
                "incomplete stopped-writer facts retained"
            );
            let resource = &row
                .resource
                .as_ref()
                .context("removed resource identity missing")?
                .0;
            if let NativeBackend::Docker(backend) = &provider.configured.backend {
                ensure!(
                    backend.removal_recorded(&spec, resource).await?,
                    "exact RemoveACK missing; evidence retained"
                );
            }
            continue;
        }
        let resource = match row.resource {
            Some(resource) => resource.0,
            None => provider
                .configured
                .backend
                .launcher()
                .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
                .await
                .context("unknown candidate create retained; no404/removal proof")?,
        };
        // Physical stop must progress independently of durable fence lock waits.
        let (fenced, exit) = tokio::join!(
            ExecutionProcessRepository::request_stop(&ctx.pool, spec.intent),
            provider
                .configured
                .backend
                .stop_retained(&spec, &resource, LaunchStopReason::Shutdown),
        );
        let exit = exit?;
        fenced?;
        // A stopped writer does not invent output/custody. Preserve the actual
        // stopped resource and SQL/input evidence when capture is incomplete.
        let capture = retained_capture(row.candidate_capture.as_deref(), master)?;
        ensure!(
            capture.exit_code == exit.exit_code,
            "terminal/capture mismatch retained"
        );
        provider
            .configured
            .backend
            .remove_stopped(&spec, &resource)
            .await?;
        // No capture, publication or terminal status is manufactured on failure.
    }
    Ok(())
}
fn temp_entries(root: &Path) -> Result<BTreeSet<std::ffi::OsString>> {
    Ok(std::fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<_>>()?)
}
struct PythonProof {
    runtime: Runtime,
    release: i64,
    digest: String,
}
async fn verify_case(
    ctx: &api_helpers::TestContext,
    provider: &Provider,
    install: &PackInstall,
    case: Case,
    python_proof: &PythonProof,
    master: &str,
) -> Result<()> {
    let runtime = &python_proof.runtime;
    let release = python_proof.release;
    let release_digest = &python_proof.digest;
    let terminal = PackInstallRepository::new(ctx.pool.clone())
        .find_by_id(install.id)
        .await?
        .context("terminal install")?;
    ensure!(
        terminal.finished_at.is_some() && terminal.candidate_access_token_hash.is_none(),
        "install not atomically finished"
    );
    let history = PackTestRepository::new(ctx.pool.clone())
        .list_by_pack(install.pack_id.context("candidate pack")?, 10, 0)
        .await?;
    let rows = evidence(ctx, install.id).await?;
    ensure!(
        rows.is_empty() == matches!(case, Case::MetadataOnly),
        "actual effect count differs from declared zero-effects case"
    );
    let mut captured_failure = false;
    for row in rows {
        ensure!(
            row.create_submitted
                && row.stdin_consumed
                && row.stop_requested
                && row.resource_removed,
            "candidate evidence incomplete"
        );
        let resource = row.resource.context("exact provider identity")?.0;
        let exit = row.exit_outcome.context("exact terminal observation")?.0;
        let context: StoredContext = serde_json::from_str(&attune_common::crypto::decrypt(
            &row.encrypted_context,
            master,
        )?)?;
        ensure!(
            context.belongs_to(install.id, row.intent)
                && Some(context.source.worker) == install.assigned_worker_id
                && context.source.pack_ref == install.pack_ref
                && context.source.pack_version == install.pack_version
                && Some(&context.source.archive_digest) == install.candidate_digest.as_ref()
                && Some(&context.source.archive_key) == install.candidate_object_key.as_ref()
                && Some(&context.source.archive_version)
                    == install.candidate_provider_version.as_ref()
                && context.image == provider.config.image
                && context.platform == provider.config.platform,
            "retained source differs"
        );
        let python = context
            .source
            .python
            .as_ref()
            .context("source-bound base Python absent")?;
        ensure!(
            python.runtime == runtime.id
                && python.release == release
                && python.release_digest.as_str() == release_digest.as_str()
                && python.runtime_config == runtime.execution_config,
            "base runtime/provider source differs"
        );
        ensure!(
            context.inputs.contains(&provider.config.helper)
                && context.inputs.len() == 2
                && context
                    .inputs
                    .iter()
                    .any(|pin| pin.kind == LaunchInputKind::Pack
                        && pin.object_id == install.id
                        && pin.object_version == context.source.archive_digest),
            "helper pin/input pair changed"
        );
        ensure!(
            context.invocation.argv.get(1).map(String::as_str) == Some("run-candidate-python"),
            "test bypassed fixed Python helper"
        );
        let cipher = row.candidate_capture.context("joined encrypted capture")?;
        ensure!(
            !cipher.contains("candidate-python-private")
                && !row.encrypted_context.contains("run-candidate-python"),
            "private evidence persisted as plaintext"
        );
        let capture: CandidateCapture =
            serde_json::from_str(&attune_common::crypto::decrypt(&cipher, master)?)?;
        ensure!(
            capture.exit_code == exit.exit_code,
            "capture did not preserve exact numeric outcome"
        );
        if case.fails_preparation() {
            ensure!(
                capture.interruption == Some(TestProcessInterruption::PreparationUnverified),
                "missing authenticated preparation accepted"
            );
            ensure!(
                !capture.stdout.contains("candidate-python-private")
                    && !capture.stdout.contains("candidate-script-private"),
                "suite started after failed global preparation"
            );
        } else {
            ensure!(
                capture.interruption.is_none(),
                "completed suite classified as interrupted"
            );
            ensure!(
                capture.stdout.contains("candidate-python-private")
                    || capture.stdout.contains("candidate-script-private")
                    || capture.stdout.contains("candidate-recipe-private"),
                "actual final-child output missing"
            );
            captured_failure |= capture.exit_code == 7;
        }
        if let NativeBackend::Docker(backend) = &provider.configured.backend {
            ensure!(
                backend.removal_recorded(&context.spec(), &resource).await?,
                "exact Docker RemoveACK absent"
            );
        } else {
            // resource_removed is written only after positive UID-preconditioned
            // Delete ACK + observed removal. This read is additional verification,
            // never permission to turn unknown Create/404 into absence.
            let ResourceIdentity::Kubernetes {
                namespace,
                pod_name,
                ..
            } = &resource
            else {
                anyhow::bail!("wrong provider identity")
            };
            ensure!(
                kube::Api::<k8s_openapi::api::core::v1::Pod>::namespaced(
                    provider.kube.clone().context("owned Kube client")?,
                    namespace
                )
                .get_opt(pod_name)
                .await?
                .is_none(),
                "acknowledged Pod still present"
            );
        }
        if let Some(effects) = &context.source.effects {
            ensure!(case.opt_in(), "default suite received API authority");
            let token = context
                .invocation
                .env
                .get("ATTUNE_CANDIDATE_EFFECTS_TOKEN")
                .context("opt-in runtime token")?;
            ensure!(
                !row.encrypted_context.contains(token),
                "runtime token persisted in plaintext"
            );
            ensure!(
                CandidateEffectRepository::authenticate(&ctx.pool, effects.lease, token)
                    .await
                    .is_err(),
                "finished candidate lease remained active"
            );
        } else {
            ensure!(!case.opt_in(), "opt-in context missing");
            ensure!(
                !context
                    .invocation
                    .env
                    .contains_key("ATTUNE_CANDIDATE_EFFECTS_TOKEN"),
                "default token minted"
            );
        }
    }
    if case.fails_preparation() || matches!(case, Case::MetadataOnly) {
        ensure!(
            terminal.status == "failed"
                && terminal.test_result.is_none()
                && terminal.test_execution_id.is_none()
                && history.is_empty(),
            "ignore fabricated history/result after global preparation failure"
        );
    } else {
        ensure!(
            terminal.status == "succeeded"
                && terminal.test_result.is_some()
                && terminal.test_execution_id.is_some()
                && history.len() == 1,
            "actual handler result transaction missing"
        );
        if matches!(case, Case::NonzeroIgnored) {
            ensure!(
                captured_failure
                    && terminal.test_result.as_ref().context("suite result")?["failed"]
                        .as_i64()
                        .unwrap_or(0)
                        > 0,
                "known final nonzero policy was lost"
            );
        }
    }
    let leases: Vec<(bool, i32)> =
        sqlx::query_as("SELECT revoked,operations FROM pack_candidate_context WHERE install_id=$1")
            .bind(install.id)
            .fetch_all(&ctx.pool)
            .await?;
    if case.opt_in() {
        ensure!(
            leases.len() == 1 && leases[0].0 && leases[0].1 > 0,
            "opt-in effect/closed lease evidence missing"
        );
    } else {
        ensure!(leases.is_empty(), "default candidate opened API lease");
    }
    let leaked:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM pack_candidate_resource r JOIN pack_candidate_context c ON c.lease=r.context WHERE c.install_id=$1)+(SELECT count(*) FROM pack_candidate_event e JOIN pack_candidate_context c ON c.lease=e.context WHERE c.install_id=$1)").bind(install.id).fetch_one(&ctx.pool).await?;
    ensure!(leaked == 0, "closed candidate left isolated resources");
    Ok(())
}

async fn case_body(
    ctx: &api_helpers::TestContext,
    provider: &Provider,
    case: Case,
    api: &str,
) -> Result<i64> {
    let (runtime, release, digest) = publish_python(ctx, case).await?;
    let python_proof = PythonProof {
        runtime,
        release,
        digest,
    };
    let pack = PackFixture::new_unique("native_candidate_owner")
        .create(&ctx.pool)
        .await?;
    let owner = ctx.user.as_ref().context("API owner")?.id;
    PackRepository::set_installed_by(&ctx.pool, pack.id, owner).await?;
    let jwt = JwtConfig {
        secret: ctx
            .state
            .config
            .security
            .jwt_secret
            .clone()
            .context("fixture signing key")?,
        ..Default::default()
    };
    let master = ctx
        .state
        .config
        .security
        .encryption_key
        .clone()
        .context("fixture encryption key")?;
    let tokens = Arc::new(WorkerTokenProvider::new(owner, "0", jwt.clone()));
    let incarnation = tokens.instance_id();
    let worker = WorkerRepository::create(
        &ctx.pool,
        CreateWorkerInput {
            name: format!("candidate-owner-{}", Uuid::new_v4()),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(attune_common::models::enums::WorkerStatus::Active),
            capabilities: Some(json!({"runtimes":["python"]})),
            meta: Some(json!({"registration_incarnation":incarnation.to_string()})),
        },
    )
    .await?;
    WorkerRepository::heartbeat_incarnation(&ctx.pool, worker.id, incarnation).await?;
    tokens.set_worker_id(worker.id.to_string());
    let (install, payload) = publish_candidate(ctx, &pack, case, worker.id).await?;
    let manager = provider.root.path().join(format!("manager-{}", install.id));
    let packs = manager.join("packs");
    let artifacts = manager.join("artifacts");
    std::fs::create_dir_all(&packs)?;
    std::fs::create_dir_all(&artifacts)?;
    let environment = ChildProcessEnvironment::from_parent(
        [(
            "ATTUNE_RUNTIME_RECIPE_DIGESTS".into(),
            recipe().digest()?.into(),
        )],
        &[],
    )?
    .with_workload_sandbox(WorkloadSandbox::NativeEffectsRequired);
    let executor = ActionExecutor::new(
        ctx.pool.clone(),
        RuntimeRegistry::new(),
        ArtifactManager::new(artifacts.clone()),
        SecretManager::new(ctx.pool.clone(), Some(master.clone()))?,
        65536,
        65536,
        false,
        false,
        worker.name,
        incarnation,
        ctx.state.config.artifacts.log_segment_writer_config(),
        None,
        None,
        packs.clone(),
        artifacts.clone(),
        manager.join("runtimes"),
        api.into(),
        jwt,
        Arc::new(ApiTransport::new_with_worker_token_provider(
            api,
            tokens.clone(),
            artifacts.to_str().context("artifact path")?,
        )?),
    )
    .with_worker_token_provider(tokens)
    .with_child_environment(environment)
    .with_native_execution(
        provider.config.clone(),
        ConfiguredBackend {
            backend: provider.configured.backend.clone(),
            cache: provider.configured.cache.clone(),
            runner: provider.configured.runner.clone(),
        },
    )?;
    executor.set_worker_id(worker.id);
    executor.set_process_incarnation(incarnation)?;
    let roots = PathBuf::from(required("ATTUNE_CANDIDATE_TEST_TMP_ROOT")?);
    ensure!(
        roots.is_absolute() && roots.canonicalize()? == std::env::temp_dir().canonicalize()?,
        "explicit owned manager TMP root required"
    );
    let before = temp_entries(&roots)?;
    let heartbeat_stop = CancellationToken::new();
    let stop = heartbeat_stop.clone();
    let pool = ctx.pool.clone();
    let worker_id = worker.id;
    let heartbeat = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {_=stop.cancelled()=>return Ok::<_,attune_common::Error>(()),_=ticker.tick()=>{}}
            WorkerRepository::heartbeat_incarnation(&pool, worker_id, incarnation).await?;
        }
    });
    let result = async {
        // Actual private production handler: parser/policy, require_result and
        // lease/history/install transaction all execute unchanged.
        handle(
            &ctx.pool,
            &executor,
            &payload,
            &install,
            &packs,
            api,
            CancellationToken::new(),
        )
        .await?;
        ensure!(
            !manager.join("runtimes").exists(),
            "candidate mutated shared runtime storage"
        );
        ensure!(
            temp_entries(&roots)? == before,
            "candidate handler left an unjoined private root"
        );
        verify_case(ctx, provider, &install, case, &python_proof, &master).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    heartbeat_stop.cancel();
    let joined = heartbeat.await;
    drop(executor);
    result?;
    joined??;
    Ok(install.id)
}
async fn run_case(provider: &Provider, case: Case) -> Result<()> {
    let ctx = map_api(api_helpers::TestContext::new().await)?;
    let ctx = map_api(ctx.with_auth().await)?;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let host = required("ATTUNE_CANDIDATE_TEST_API_HOST")?;
    ensure!(
        !host.is_empty() && !host.contains(['/', ':', '@']),
        "explicit owned API host required"
    );
    let api = format!("http://{host}:{}", listener.local_addr()?.port());
    let server_stop = CancellationToken::new();
    let stop = server_stop.clone();
    let app = ctx.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
    });
    let result = case_body(&ctx, provider, case, &api).await;
    // Always recover only this fixture's real recorded resources. Unknown
    // Create+404 cannot authorize dropping evidence or deleting a manager root.
    let cleaned = async {
        let master = ctx
            .state
            .config
            .security
            .encryption_key
            .as_deref()
            .context("fixture encryption key")?;
        let installs: Vec<i64> = sqlx::query_scalar("SELECT id FROM pack_install ORDER BY id")
            .fetch_all(&ctx.pool)
            .await?;
        for install in installs {
            cleanup_effects(&ctx, provider, install, master).await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    server_stop.cancel();
    let served = server.await;
    if let Err(error) = cleaned {
        // Keep exact retained SQL/provider facts and immutable inputs for the
        // owning lane to diagnose/recover; neither Drop nor404 proves cleanup.
        std::mem::forget(ctx);
        served??;
        return Err(error.context("owning candidate evidence retained after uncertain cleanup"));
    }
    let database_cleanup = map_api(ctx.cleanup().await);
    result?;
    served??;
    database_cleanup?;
    Ok(())
}
fn close_root(root: tempfile::TempDir) -> Result<()> {
    fn writable(path: &Path) -> std::io::Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            for entry in std::fs::read_dir(path)? {
                writable(&entry?.path())?;
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
        for case in [
            Case::Ordinary,
            Case::MixedRecipe,
            Case::PreparationFailure,
            Case::MetadataOnly,
            Case::NonzeroIgnored,
            Case::ApiOptIn,
        ] {
            run_case(&provider, case)
                .await
                .with_context(|| format!("owning candidate scenario {case:?}"))?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let Provider {
        root, configured, ..
    } = provider;
    drop(configured);
    if let Err(error) = result {
        let retained = root.keep();
        return Err(error.context(format!(
            "owned candidate fixture retained at {}",
            retained.display()
        )));
    }
    close_root(root)
}
#[tokio::test]
#[ignore = "requires explicit owned Docker socket/cache/network, once-built pinned images/Runner/Echo, real API and owned PostgreSQL"]
async fn docker_native_candidate() -> Result<()> {
    owning(true).await
}
#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace/context/kubeconfig, once-built pinned images/Runner/Echo, real API and owned PostgreSQL"]
async fn kubernetes_native_candidate() -> Result<()> {
    owning(false).await
}

#[test]
fn owning_declarations_use_real_preparation_and_explicit_api_opt_in() -> Result<()> {
    for case in [
        Case::Ordinary,
        Case::MixedRecipe,
        Case::PreparationFailure,
        Case::MetadataOnly,
        Case::NonzeroIgnored,
        Case::ApiOptIn,
    ] {
        let testing: attune_common::test_executor::TestConfig =
            serde_json::from_value(testing(case))?;
        testing.validate_policy()?;
        ensure!(testing.candidate_api_effects == case.opt_in());
        ensure!(testing
            .runners
            .values()
            .any(|runner| runner.r#type == "unittest"));
        ensure!(
            testing.required_recipe_digests()?.len()
                == usize::from(matches!(case, Case::MixedRecipe))
        );
        let config = serde_json::from_value(python_definition(case))?;
        let (plan, interpreter) = attune_workload_backend::planning::candidate_python_setup(
            &config,
            "/tmp/owned-candidate-fixture/env",
        )?;
        ensure!(
            !plan.steps.is_empty() && interpreter == "/tmp/owned-candidate-fixture/env/bin/python3"
        );
        ensure!(
            !plan.environment.contains_key("ATTUNE_API_TOKEN")
                && !plan
                    .environment
                    .contains_key("ATTUNE_CANDIDATE_EFFECTS_TOKEN")
        );
    }
    Ok(())
}
