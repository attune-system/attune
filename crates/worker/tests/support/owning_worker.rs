//! Owning trusted-worker proof on an explicit disposable namespace. Provider
//! effects alone cannot establish admission, custody or atomic completion.
#![cfg(unix)]
#[path = "../../../common/tests/helpers.rs"]
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
    workload_config::WorkloadConfig,
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
    collections::BTreeMap,
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
pub fn pin(
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
pub fn runner_zip(path: &Path, program: &Path, platform: LaunchPlatform) -> Result<()> {
    let mut archive = zip::ZipWriter::new(File::create_new(path)?);
    archive.start_file(
        format!("linux-{}", platform.native_arch()),
        zip::write::SimpleFileOptions::default().unix_permissions(0o755),
    )?;
    archive.write_all(&std::fs::read(program)?)?;
    archive.finish()?.sync_all()?;
    Ok(())
}
pub async fn owned_kubernetes_client() -> Result<Client> {
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

pub struct NativeOwningFixture {
    pub root: tempfile::TempDir,
    pub config: WorkloadConfig,
    pub configured: ConfiguredBackend,
    pub echo: PathBuf,
    pub runner: PathBuf,
}
#[derive(Clone, Copy)]
pub enum OwningCase {
    Native,
    NativeCli,
    Command,
    Interpreted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InlineCase {
    ShellFile,
    Direct,
    TempFile,
    Recipe,
}

impl NativeOwningFixture {
    fn child(&self) -> Result<Self> {
        Ok(Self {
            root: tempfile::Builder::new()
                .prefix("owning-case-")
                .tempdir_in(self.root.path())?,
            config: self.config.clone(),
            configured: ConfiguredBackend {
                backend: self.configured.backend.clone(),
                cache: self.configured.cache.clone(),
                runner: self.configured.runner.clone(),
            },
            echo: self.echo.clone(),
            runner: self.runner.clone(),
        })
    }
}

/// One existing interpreted wrapper, one configured provider/cache/image. Every
/// scenario joins full custody/restart/removal before the next attempt starts.
pub async fn run_interpreted_suite(fixture: NativeOwningFixture) -> Result<()> {
    let result = async {
        run_native_fixture(fixture.child()?, OwningCase::Interpreted).await?;
        for (delivery, format) in [
            ("stdin", "yaml"),
            ("stdin", "dotenv"),
            ("file", "json"),
            ("file", "yaml"),
            ("file", "dotenv"),
        ] {
            run_native_parameter_fixture(fixture.child()?, delivery, format).await?;
        }
        for (case, delivery, format) in inline_scenarios() {
            owning_case(
                Some(fixture.child()?),
                false,
                false,
                false,
                Some((delivery, format)),
                Some(case),
            )
            .await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let NativeOwningFixture {
        root, configured, ..
    } = fixture;
    drop(configured);
    fn unseal(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::symlink_metadata(path)?.is_dir() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            for entry in std::fs::read_dir(path)? {
                unseal(&entry?.path())?;
            }
        }
        Ok(())
    }
    let cleanup = unseal(root.path()).and_then(|_| root.close());
    result?;
    cleanup?;
    Ok(())
}

fn inline_scenarios() -> [(InlineCase, &'static str, &'static str); 7] {
    [
        (InlineCase::ShellFile, "stdin", "json"),
        (InlineCase::Direct, "stdin", "json"),
        (InlineCase::Direct, "file", "yaml"),
        (InlineCase::Direct, "stdin", "dotenv"),
        (InlineCase::TempFile, "stdin", "json"),
        (InlineCase::TempFile, "file", "dotenv"),
        (InlineCase::Recipe, "stdin", "json"),
    ]
}

fn inline_recipe() -> attune_common::runtime_lifecycle::RuntimeRecipe {
    serde_json::from_value(json!({
        "workspace":true,"extensions":[".py"],
        "tools":[{"name":"python","binaries":["/usr/bin/python3"],"args":["--version"],"version_pattern":"Python (?<version>[0-9]+\\.[0-9]+\\.[0-9]+)","primary":true}],
        "prepare":[{"command":["{tool_python}","-c","import os,sys; from pathlib import Path; assert sys.stdin.read()==''; assert 'ATTUNE_API_TOKEN' not in os.environ; Path('recipe-private-marker').write_text('private')"]}],
        "launch":{"command":["{tool_python}","{source_file}"],"timeout_seconds":120}
    })).expect("fixed owning recipe")
}
fn inline_config(case: InlineCase) -> serde_json::Value {
    match case {
        InlineCase::Recipe => json!({"lifecycle":inline_recipe()}),
        InlineCase::Direct => {
            json!({"interpreter":{"binary":"/bin/bash","args":["--must-not-reach-direct","{pack_dir}"]},"inline_execution":{"strategy":"direct","extension":".ignored","inject_shell_helpers":true}})
        }
        InlineCase::ShellFile => {
            json!({"interpreter":{"binary":"/bin/bash","args":["-e"]},"inline_execution":{"strategy":"temp_file","extension":"sh","inject_shell_helpers":true}})
        }
        InlineCase::TempFile => {
            json!({"interpreter":{"binary":"/bin/bash","args":["--noprofile","--norc","--rcfile","literal{pack_dir}","-e"]},"inline_execution":{"strategy":"temp_file","extension":"sh","inject_shell_helpers":true}})
        }
    }
}
const INLINE_INPUT_READER: &str = r#"import json,os,sys,stat,shlex
from pathlib import Path
assert Path('/usr/local/libexec/attune/echo').is_file()
for name in ['DATABASE_URL','JWT_SECRET','ENCRYPTION_KEY','ATTUNE__DUMMY_IMAGE_CREDENTIAL','ATTUNE_RUNTIME_RECIPE_DIGESTS']:
    assert name not in os.environ,name
assert not Path('/var/run/secrets/kubernetes.io/serviceaccount/token').exists()
assert not Path('/var/run/docker.sock').exists()
assert not Path('/tmp/inline-injection').exists()
delivery=os.environ['ATTUNE_PARAMETER_DELIVERY']
format=os.environ['ATTUNE_PARAMETER_FORMAT']
if delivery=='file':
    assert sys.stdin.read()==''
    path=Path(os.environ['ATTUNE_PARAMETER_FILE'])
    assert path.parent==Path('/tmp') and stat.S_IMODE(path.stat().st_mode)==0o400
    raw=path.read_text()
else:
    assert delivery=='stdin' and 'ATTUNE_PARAMETER_FILE' not in os.environ
    raw=sys.stdin.read()
if format=='json': message=json.loads(raw)['message']
elif format=='yaml':
    assert raw.startswith('message: ')
    message=raw[len('message: '):].strip()
else:
    assert format=='dotenv'
    [field]=shlex.split(raw)
    assert field.startswith('message=')
    message=field[len('message='):]
sys.stdout.write(message)
"#;
fn inline_source(case: InlineCase) -> String {
    match case {
        InlineCase::ShellFile => "inline-probe.sh".into(),
        InlineCase::Direct => format!("exec /usr/bin/python3 -c '{}'",INLINE_INPUT_READER.replace('\'',"'\\''")),
        InlineCase::TempFile => r#"[[ "$0" == /tmp/attune-inline-*/source-*.sh ]]
[[ $(stat -c %a "$0") == 600 ]]
[[ "$PARAM_MESSAGE" == "$message" ]]
[[ -z $(printenv PARAM_MESSAGE || true) && -z $(printenv message || true) ]]
[[ ! -v ATTUNE__DUMMY_IMAGE_CREDENTIAL && ! -v ATTUNE_RUNTIME_RECIPE_DIGESTS ]]
[[ ! -e /var/run/docker.sock && ! -e /var/run/secrets/kubernetes.io/serviceaccount/token ]]
[[ ! -e /tmp/inline-injection ]]
if IFS= read -r unexpected; then exit 41; fi
if [[ "$ATTUNE_PARAMETER_DELIVERY" == file ]]; then
    [[ -f "$ATTUNE_PARAMETER_FILE" && $(stat -c %a "$ATTUNE_PARAMETER_FILE") == 400 ]]
fi
printf '%s' "$message"
"#.into(),
        InlineCase::Recipe => format!("from pathlib import Path\nassert Path(__file__).is_relative_to('/tmp/attune-lifecycle-generations')\nassert Path('recipe-private-marker').read_text()=='private'\n{INLINE_INPUT_READER}"),
    }
}

fn assert_inline_invocation(
    case: InlineCase,
    spec: &ProcessSpec,
    source: &str,
    message: &str,
) -> Result<()> {
    ensure!(
        spec.pack_access == PackAccess::ReadOnly
            && spec.stage.is_none()
            && spec.invocation.working_root == LaunchRoot::Pack,
        "inline source mount/root contract changed"
    );
    ensure!(
        spec.inputs
            .iter()
            .all(|pin| pin.kind != LaunchInputKind::Runtime),
        "private inline recipe/shell borrowed shared Runtime"
    );
    ensure!(
        !spec
            .invocation
            .argv
            .iter()
            .any(|value| value.contains(message))
            && !spec
                .invocation
                .env
                .values()
                .any(|value| value.contains(message)),
        "parameter value leaked into private launch argv/environment"
    );
    match case {
        InlineCase::ShellFile => {
            ensure!(
                spec.invocation.argv
                    == ["/bin/bash", "-e", &format!("{PACK_ROOT}/actions/{source}")],
                "real released shell file was reclassified as inline"
            );
        }
        InlineCase::Direct | InlineCase::TempFile => {
            ensure!(
                spec.invocation.argv.first()
                    == Some(&format!(
                        "{RUNNER_ROOT}/linux-{}",
                        spec.platform.native_arch()
                    ))
                    && spec.invocation.argv.get(1).map(String::as_str) == Some("run-inline-action"),
                "actual producer did not select fixed inline Runner"
            );
            let plan = attune_workload_backend::inline_action_plan::InlineActionPlan::decode(
                &spec.invocation.argv[2..],
            )?;
            ensure!(
                plan.source == source && plan.interpreter == "/bin/bash" && !plan.has_runtime,
                "inline literal/interpreter binding changed"
            );
            if case == InlineCase::Direct {
                ensure!(
                    plan.strategy
                        == attune_common::models::runtime::InlineExecutionStrategy::Direct
                        && plan.arguments.is_empty()
                        && !plan.shell_helpers
                        && plan.extension.is_empty(),
                    "Direct consumed TempFile args/helpers/extension"
                );
            } else {
                ensure!(
                    plan.strategy
                        == attune_common::models::runtime::InlineExecutionStrategy::TempFile
                        && plan.arguments
                            == [
                                "--noprofile",
                                "--norc",
                                "--rcfile",
                                "literal{pack_dir}",
                                "-e"
                            ]
                        && plan.shell_helpers
                        && plan.extension == ".sh",
                    "TempFile changed raw args/helpers/extension"
                );
            }
        }
        InlineCase::Recipe => {
            ensure!(
                spec.invocation.argv.get(1).map(String::as_str) == Some("run-runtime-recipe")
                    && spec.invocation.argv.get(2).map(String::as_str) == Some("inline"),
                "ad-hoc recipe did not use existing private-generation inline source"
            );
            let count: usize = spec
                .invocation
                .argv
                .get(3)
                .context("recipe source parts")?
                .parse()?;
            let selected = spec
                .invocation
                .argv
                .get(4..4 + count)
                .context("recipe source chunks")?
                .concat();
            let selected: String = serde_json::from_str(&selected)?;
            ensure!(selected == source, "recipe literal changed");
        }
    }
    Ok(())
}

pub async fn run_native_fixture(fixture: NativeOwningFixture, case: OwningCase) -> Result<()> {
    owning_case(
        Some(fixture),
        matches!(case, OwningCase::NativeCli | OwningCase::Command),
        matches!(case, OwningCase::Command),
        matches!(case, OwningCase::Interpreted),
        None,
        None,
    )
    .await
}
/// Sequential variants of the same real producer/custody/cleanup case. No
/// alternate preparer, resource policy or new provider prerequisite is used.
pub async fn run_native_parameter_fixture(
    fixture: NativeOwningFixture,
    delivery: &'static str,
    format: &'static str,
) -> Result<()> {
    ensure!(matches!(delivery, "stdin" | "file"));
    ensure!(matches!(format, "json" | "yaml" | "dotenv"));
    owning_case(
        Some(fixture),
        false,
        false,
        true,
        Some((delivery, format)),
        None,
    )
    .await
}
// Used by the original repository-double wrapper; the Docker-only target
// compiles the same support module without selecting that separate case.
#[allow(dead_code)]
pub async fn run_repository_fixture() -> Result<()> {
    owning_case(None, false, false, false, None, None).await
}

async fn owning_case(
    native: Option<NativeOwningFixture>,
    typed_cli: bool,
    command: bool,
    interpreted: bool,
    declared_input: Option<(&'static str, &'static str)>,
    inline: Option<InlineCase>,
) -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let real_provider = native.is_some();
    ensure!(
        inline.is_none() || real_provider,
        "inline acceptance requires the concrete provider"
    );
    let (root, platform, echo, runner, native_config, configured) = if let Some(fixture) = native {
        (
            fixture.root,
            fixture.config.platform,
            fixture.echo,
            fixture.runner,
            Some(fixture.config),
            Some(fixture.configured),
        )
    } else {
        let root = tempfile::tempdir()?;
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
        (root, LaunchPlatform::LinuxAmd64, echo, runner, None, None)
    };
    let payload_image = native_config
        .as_ref()
        .map(|c| c.image.clone())
        .unwrap_or_else(|| {
            let sha256 = Sha256Digest::of_bytes(b"explicit-mock-platform-image");
            PinnedImage {
                reference: format!("owned.test/native@sha256:{}", sha256.as_str()),
                sha256,
            }
        });
    let database = helpers::create_test_pool().await?;
    let cache = if let Some(configured) = &configured {
        configured.cache.clone()
    } else {
        Arc::new(CacheStore::open(
            root.path().join("cache"),
            CacheLimits {
                compressed_bytes: 64 << 20,
                expanded_bytes: 128 << 20,
                files: 4096,
            },
        )?)
    };
    let native_backend = configured.as_ref().map(|c| c.backend.clone());
    let launcher: Arc<dyn ProcessLauncher> = native_backend
        .as_ref()
        .map(NativeBackend::launcher)
        .unwrap_or_else(|| Arc::new(RepositoryOnlyLauncher));
    let owner = IdentityFixture::new_unique("isolated_owner")
        .create(&database)
        .await?;
    let pack = PackFixture::new_unique("isolated_native")
        .create(&database)
        .await?;
    let mut runtime = RuntimeFixture::new(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        if inline == Some(InlineCase::Recipe) {
            "inline_recipe"
        } else if inline.is_some() {
            "shell"
        } else if interpreted {
            "python"
        } else {
            "native"
        },
    );
    runtime.execution_config = if let Some(case) = inline {
        inline_config(case)
    } else if interpreted {
        json!({
            "interpreter":{"binary":"python3","args":["-u"],"file_extension":".py"},
            "environment":{"env_type":"virtualenv","dir_name":"venv","create_command":["python3","-m","venv","--copies","--without-pip","{env_dir}"],"interpreter_path":"{env_dir}/bin/python3"},
            "dependencies":{"manifest_file":"input.txt","install_command":["{interpreter}","-c","from pathlib import Path; assert Path('{manifest_path}').read_text()=='owned offline dependency'; Path('{pack_dir}/installed-marker').write_text('prepared')"]}
        })
    } else {
        json!({})
    };
    let runtime = runtime.create(&database).await?;
    let relative = inline.map(inline_source).unwrap_or_else(|| {
        if interpreted {
            "echo.py".into()
        } else {
            "echo".into()
        }
    });
    let binary_relative = format!("echo/linux-{}", platform.native_arch());
    let action = if inline == Some(InlineCase::Recipe) {
        // Real ad-hoc authoring input. The repository resolves this row against
        // its pack's active release; no released snapshot is fabricated/edited.
        use attune_common::repositories::action::{ActionRepository, CreateActionInput};
        ActionRepository::create(
            &database,
            CreateActionInput {
                r#ref: format!("{}.echo", pack.r#ref),
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                label: "owning inline recipe".into(),
                description: None,
                entrypoint: Some(relative.clone()),
                launch: None,
                parameter_delivery: None,
                parameter_format: None,
                runtime: Some(runtime.id),
                enabled: true,
                runtime_version_constraint: None,
                required_worker_runtimes: json!({}),
                worker_selector: json!({}),
                worker_tolerations: json!([]),
                worker_affinity: json!({}),
                param_schema: Some(json!({"message":{"type":"string","required":true}})),
                out_schema: None,
                output_policy: Default::default(),
                output_format: Default::default(),
                is_adhoc: true,
                accesses_mcp: false,
                default_execution_permission_set_refs: vec![],
                reference_visibility: Default::default(),
                reference_allowed_pack_refs: vec![],
                artifact_retention_policy: None,
                artifact_retention_limit: None,
                log_retention_policy: None,
                log_retention_limit: None,
                timeout_seconds: None,
            },
        )
        .await?
    } else {
        ActionFixture::new(pack.id, &pack.r#ref, "echo")
            .with_runtime(runtime.id)
            .with_entrypoint(&relative)
            .with_param_schema(json!({"message":{"type":"string","required":true}}))
            .create(&database)
            .await?
    };
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
    let action = if let Some((delivery, format)) = declared_input {
        use attune_common::repositories::action::{ActionRepository, UpdateActionInput};
        ActionRepository::update(
            &database,
            action.id,
            UpdateActionInput {
                parameter_delivery: Some(delivery.into()),
                parameter_format: Some(format.into()),
                ..Default::default()
            },
        )
        .await?
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
    if inline != Some(InlineCase::Recipe) {
        std::fs::write(
            source.join("actions/echo.yaml"),
            serde_json::to_vec(&json!({
                "ref":action.r#ref,"runner_type":runtime.name,"entry_point":relative,
                "param_schema":action.param_schema,
                "parameter_delivery":declared_input.map_or("stdin", |input|input.0),
                "parameter_format":declared_input.map_or("json", |input|input.1)
            }))?,
        )?;
    }
    if inline.is_some() {
        std::fs::create_dir_all(source.join("runtimes"))?;
        std::fs::write(
            source.join(format!("runtimes/{}.yaml", runtime.name)),
            serde_json::to_vec(&json!({
                "ref":runtime.r#ref,"name":runtime.name,"aliases":runtime.aliases,
                "execution_config":runtime.execution_config,"distributions":runtime.distributions
            }))?,
        )?;
    }
    if inline == Some(InlineCase::ShellFile) {
        std::fs::write(
            source.join("actions/inline-probe.sh"),
            format!(
                "[[ ! -v PARAM_MESSAGE ]]\nexec /usr/bin/python3 -c '{}'",
                INLINE_INPUT_READER.replace('\'', "'\\''")
            ),
        )?;
    }
    let executable = source.join("actions").join(&binary_relative);
    std::fs::copy(&echo, &executable)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
    if interpreted {
        std::fs::write(source.join("input.txt"), b"owned offline dependency")?;
        std::fs::write(
            source.join("actions/echo.py"),
            br#"import json,sys,os,stat
from pathlib import Path
assert Path('/opt/attune/runtime/project/installed-marker').read_text()=='prepared'
assert Path('/opt/attune/runtime/project/pack.yaml').is_file()
assert Path('/opt/attune/runtime/bin/python3').is_file()
delivery=os.environ.get('ATTUNE_PARAMETER_DELIVERY','stdin')
format=os.environ.get('ATTUNE_PARAMETER_FORMAT','json')
if delivery=='file':
    assert sys.stdin.read()==''
    selected=Path(os.environ['ATTUNE_PARAMETER_FILE'])
    assert selected.parent==Path('/tmp')
    assert stat.S_IMODE(selected.stat().st_mode)==0o400
    raw=selected.read_text()
else:
    assert delivery=='stdin' and 'ATTUNE_PARAMETER_FILE' not in os.environ
    raw=sys.stdin.read()
if format=='json':
    message=json.loads(raw)['message']
elif format=='yaml':
    assert raw.startswith('message: ')
    message=raw[len('message: '):].strip()
else:
    assert format=='dotenv' and raw.startswith("message='") and raw.endswith("'")
    message=raw[len("message='"):-1]
sys.stdout.write(message)
"#,
        )?;
    }
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
            // Ad-hoc definition remains the real authored row; only its pack
            // and runtime belong to this immutable provider release.
            actions: if inline == Some(InlineCase::Recipe) {
                vec![]
            } else {
                vec![action.id]
            },
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
    let runner_pin = if let (Some(config), Some(configured)) = (&native_config, &configured) {
        ensure!(
            serde_json::to_value(&config.helper)? == serde_json::to_value(&configured.runner)?,
            "configured Runner differs from the admitted operator pin"
        );
        config.helper.clone()
    } else {
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
        runner_pin
    };
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
    let message = if inline == Some(InlineCase::TempFile) {
        format!(
            "owned-captured-{}-'$(touch /tmp/inline-injection); tail",
            Uuid::new_v4()
        )
    } else {
        format!("owned-captured-{}", Uuid::new_v4())
    };
    let mut tx = database.begin().await?;
    let snapshot = ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id).await?;
    if let Some(case) = inline {
        ensure!(
            snapshot.release.id == release.id && snapshot.release.digest == release.digest,
            "snapshot release binding changed"
        );
        ensure!(
            snapshot.executable.action.entrypoint.as_deref() == Some(relative.as_str()),
            "literal source changed before admission"
        );
        ensure!(
            snapshot.executable.action.is_adhoc == (case == InlineCase::Recipe),
            "source classification was not genuinely authored"
        );
        let has_source = published
            .manifest
            .files
            .iter()
            .any(|file| file.path == format!("actions/{relative}"));
        ensure!(
            has_source == (case == InlineCase::ShellFile),
            "fixture cannot distinguish actual released file from inline literal"
        );
    }

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
    let executor = if inline.is_some() {
        let captured =
            attune_common::child_process_environment::ChildProcessEnvironment::from_parent(
                inline.filter(|case| *case == InlineCase::Recipe).map(|_| {
                    (
                        std::ffi::OsString::from("ATTUNE_RUNTIME_RECIPE_DIGESTS"),
                        std::ffi::OsString::from(
                            inline_recipe().digest().expect("fixed recipe digest"),
                        ),
                    )
                }),
                &[],
            )?
            .with_workload_sandbox(
                attune_common::workload_sandbox::WorkloadSandbox::NativeEffectsRequired,
            );
        executor.with_child_environment(captured)
    } else {
        executor
    };
    let executor = if let (Some(config), Some(configured)) = (native_config, configured) {
        executor.with_native_execution(config, configured)?
    } else {
        executor
    };
    executor.set_worker_id(worker.id);
    executor.set_process_incarnation(incarnation)?;
    ensure!(
        executor.set_process_incarnation(Uuid::new_v4()).is_err(),
        "native owner rebound"
    );
    let expected_image = payload_image.clone();
    let expected_runner = runner_pin.clone();
    let spec = (!real_provider).then(|| ProcessSpec {
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
    });
    let mut intent = spec
        .as_ref()
        .map(|spec| spec.intent)
        .unwrap_or_else(Uuid::nil);
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
        if interpreted {
            let setup=attune_worker::env_setup::scan_and_setup_all_environments(
                database.pool(),worker.id,None,&publication_root,&root.path().join("runtimes"),
                &attune_common::child_process_environment::ChildProcessEnvironment::from_parent(std::iter::empty::<(std::ffi::OsString,std::ffi::OsString)>(), &[])?.with_workload_sandbox(attune_common::workload_sandbox::WorkloadSandbox::NativeEffectsRequired),
                Some(&executor),
            ).await;
            ensure!(setup.errors.is_empty(),"native startup scan failed");
            ensure!(setup.environments_created==1,"startup did not create exactly one native environment");
        }

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
                        spec.context("repository double spec")?,
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
        if let Some(case) = inline {
            ensure!(retained.image == expected_image && retained.platform == platform,"inline image/platform pin changed");
            ensure!(retained.inputs.iter().find(|pin|pin.kind==LaunchInputKind::Runner)==Some(&expected_runner),"inline Runner pin changed");
            assert_inline_invocation(case,&retained,&relative,&message)?;
        }
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
        if interpreted {
            let pin=retained.inputs.iter().find(|p|p.kind==LaunchInputKind::Runtime).context("Setup Runtime pin missing")?;
            cache.open_input_archive(pin)?;
            let (setup,resource)=attune_common::repositories::pack_environment_process::PackEnvironmentProcessRepository::retained_output(database.pool(),pin,MASTER).await?;
            ensure!(setup.purpose==LaunchPurpose::Setup && setup.image==retained.image && setup.platform==retained.platform,"Setup source binding changed");
            ensure!(!setup.invocation.env.contains_key("ATTUNE_API_TOKEN"),"Setup received action API grant");
            let mut tx=database.begin().await?;
            ensure!(Effects::lock_completion(&mut tx,setup.intent,MASTER).await.is_err(),"Setup completed Action");
            tx.rollback().await?;
            // Reuse the durable exact removal ACK; never infer it from a 404.
            ensure!(attune_common::repositories::pack_environment_process::PackEnvironmentProcessRepository::removal_recorded(database.pool(),setup.intent,&resource).await?,"Setup cleanup receipt missing");
        }
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
        if let (Some(backend), Ok(Some(identity))) = (&native_backend, &identity) {
            if let NativeBackend::Docker(_) = backend {
                let retained =
                    Effects::retained_for_attempt(database.pool(), execution.id, attempt, MASTER)
                        .await?
                        .context("Docker cleanup lost retained native spec")?;
                if restart.is_ok() {
                    return verify_docker_removal(backend, &retained, identity).await;
                }
                backend
                    .stop_retained(&retained, identity, LaunchStopReason::Cancelled)
                    .await?;
                return backend
                    .remove_stopped(&retained, identity)
                    .await
                    .map_err(Into::into);
            }
            let client = owned_kubernetes_client().await?;
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
/// NativeBackend unchanged; this double never launches an executable.
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

async fn verify_docker_removal(
    backend: &NativeBackend,
    spec: &ProcessSpec,
    identity: &ResourceIdentity,
) -> Result<()> {
    let NativeBackend::Docker(backend) = backend else {
        anyhow::bail!("wrong Docker backend")
    };
    ensure!(
        backend.removal_recorded(spec, identity).await?,
        "trusted completion left Docker removal unacknowledged"
    );
    Ok(())
}
