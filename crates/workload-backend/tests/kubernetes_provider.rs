//! Real, explicit, disposable-cluster effects. Missing prerequisites FAIL. The
//! owning runner supplies a run-owned namespace and namespace-limited identity;
//! this test never infers a developer/default context or provisions a cluster.
//! These are provider effects, not trusted-worker custody/completion evidence.
use attune_common::{
    isolated_process::{ProcessCreateDisposition, ProcessLauncher, ProcessSpec, ResourceIdentity},
    workload_launch::*,
    workload_stdin::LaunchStdinPayload,
};
use attune_workload_backend::{
    cache::{CacheLimits, CacheStore},
    kubernetes::{KubernetesLauncher, KubernetesPolicy},
};
use k8s_openapi::api::core::v1::{Pod, Secret};
use kube::{
    api::{DeleteParams, ListParams, Preconditions},
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client, Config,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;
use uuid::Uuid;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[path = "support/runtime_recipe.rs"]
mod runtime_recipe;
fn required(name: &str) -> TestResult<String> {
    std::env::var(name).map_err(|_| format!("missing owned provider prerequisite: {name}").into())
}
fn image(name: &str) -> TestResult<PinnedImage> {
    let reference = required(name)?;
    let (_, digest) = reference
        .split_once("@sha256:")
        .ok_or("provider fixture image must use immutable name@sha256")?;
    let sha256 = Sha256Digest::parse(digest)?;
    Ok(PinnedImage { reference, sha256 })
}
struct Fixture {
    client: Client,
    launcher: KubernetesLauncher,
    policy: KubernetesPolicy,
    cache: Arc<CacheStore>,
    _root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        let path = PathBuf::from(required("ATTUNE_ISOLATION_TEST_KUBECONFIG")?);
        if !path.is_absolute() {
            return Err("owned kubeconfig must be absolute".into());
        }
        let context = required("ATTUNE_ISOLATION_TEST_CONTEXT")?;
        let config = Kubeconfig::read_from(path)?;
        // Explicit, namespace-scoped credential only; never Client::try_default.
        let config = Config::from_custom_kubeconfig(
            config,
            &KubeConfigOptions {
                context: Some(context),
                ..Default::default()
            },
        )
        .await?;
        let client = Client::try_from(config)?;
        let payload = image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE")?;
        let policy = KubernetesPolicy {
            cluster: required("ATTUNE_ISOLATION_TEST_CLUSTER")?,
            namespace: required("ATTUNE_ISOLATION_TEST_NAMESPACE")?,
            helper_image: image("ATTUNE_ISOLATION_TEST_HELPER_IMAGE")?,
            allowed_images: BTreeSet::from([payload.reference.clone()]),
            memory_bytes: 128 * 1024 * 1024,
            scratch_bytes: 64 * 1024 * 1024,
        };
        let root = tempfile::tempdir()?;
        let cache = CacheStore::open(
            root.path().join("cache"),
            CacheLimits {
                compressed_bytes: 64 * 1024 * 1024,
                expanded_bytes: 128 * 1024 * 1024,
                files: 4096,
            },
        )?;
        let cache = Arc::new(cache);
        let launcher = KubernetesLauncher::new(client.clone(), policy.clone(), cache.clone());
        Ok(Self {
            client,
            launcher,
            policy,
            cache,
            _root: root,
        })
    }
    fn spec(&self, argv: Vec<String>) -> TestResult<ProcessSpec> {
        let platform = match required("ATTUNE_ISOLATION_TEST_PLATFORM")?.as_str() {
            "linux/amd64" => LaunchPlatform::LinuxAmd64,
            "linux/arm64" => LaunchPlatform::LinuxArm64,
            _ => return Err("unsupported fixture platform".into()),
        };
        Ok(ProcessSpec {
            intent: Uuid::new_v4(),
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution: 1,
                worker: 2,
                worker_instance: Uuid::new_v4(),
                attempt: Uuid::new_v4(),
            },
            image: image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE")?,
            platform,
            inputs: vec![],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(30_000),
            invocation: LaunchInvocation {
                argv,
                env: BTreeMap::from([("SELECTED".into(), "owned-selected-value".into())]),
                working_root: LaunchRoot::Scratch,
            },
        })
    }
    fn input(
        &self,
        kind: LaunchInputKind,
        files: Vec<(&str, Vec<u8>, bool)>,
    ) -> TestResult<PinnedLaunchInput> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Seek, Write};
        let mut body = tempfile::tempfile()?;
        {
            let mut zip = zip::ZipWriter::new(&mut body);
            for (name, bytes, executable) in files {
                zip.start_file(
                    name,
                    zip::write::SimpleFileOptions::default().unix_permissions(if executable {
                        0o755
                    } else {
                        0o644
                    }),
                )?;
                zip.write_all(&bytes)?;
            }
            zip.finish()?;
        }
        body.rewind()?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let count = body.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        body.rewind()?;
        let pin = PinnedLaunchInput {
            pin_id: 1,
            object_id: 1,
            object_version: Uuid::new_v4().to_string(),
            kind,
            size_bytes: body.metadata()?.len(),
            sha256: Sha256Digest::parse(
                hash.finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            )?,
            max_expanded_bytes: 128 * 1024 * 1024,
            max_files: 4096,
        };
        self.cache.install_input(&pin, &mut body)?;
        Ok(pin)
    }
    async fn cleanup(&self, resource: &ResourceIdentity) -> TestResult {
        let ResourceIdentity::Kubernetes {
            namespace,
            pod_name,
            pod_uid,
            ..
        } = resource
        else {
            return Err("wrong resource kind".into());
        };
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        // Positive Delete ACK + exact UID precondition BEFORE observing absence.
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
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                match pods.get_opt(pod_name).await? {
                    None => return Ok::<_, kube::Error>(()),
                    Some(pod) if pod.metadata.uid.as_deref() != Some(pod_uid) => {
                        return Err(kube::Error::Service(Box::new(std::io::Error::other(
                            "owned Pod replaced during cleanup",
                        ))))
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await??;
        Ok(())
    }
    async fn run(
        &self,
        spec: &ProcessSpec,
        input: Vec<u8>,
    ) -> TestResult<(LaunchExitOutcome, Vec<u8>, Vec<u8>)> {
        let (resource, exit, out, err) = self.run_retained(spec, input).await?;
        self.launcher.remove_stopped(spec, &resource).await?;
        Ok((exit, out, err))
    }
    async fn run_retained(
        &self,
        spec: &ProcessSpec,
        input: Vec<u8>,
    ) -> TestResult<(ResourceIdentity, LaunchExitOutcome, Vec<u8>, Vec<u8>)> {
        let resource = self
            .launcher
            .prepare(spec, ProcessCreateDisposition::CreateOnce)
            .await?;
        let result = tokio::time::timeout(Duration::from_secs(150), async {
            let io = self.launcher.start(spec, &resource).await?;
            let mut stdout = io.stdout;
            let stderr = io.stderr;
            let control = io.control;
            // An admitted command that emits before reading stdin still cannot
            // execute before the trusted caller opens the fixed private gate.
            let mut early = [0];
            if tokio::time::timeout(Duration::from_millis(100), stdout.read(&mut early))
                .await
                .is_ok()
            {
                return Err("payload emitted or exited before start gate".into());
            }
            let effect = async {
                if let Err(error) = control
                    .write_stdin_once(LaunchStdinPayload::from_bytes(input)?)
                    .await
                {
                    let _stopped = control.stop(LaunchStopReason::Cancelled).await;
                    return Err::<LaunchExitOutcome, Box<dyn std::error::Error + Send + Sync>>(
                        error.into(),
                    );
                }
                let replay = control
                    .write_stdin_once(LaunchStdinPayload::from_bytes(Vec::new())?)
                    .await;
                if replay.is_ok() {
                    return Err("one-shot stdin was replayed".into());
                }
                Ok(control.wait().await?)
            };
            let capture_out = async {
                let mut data = Vec::new();
                stdout.take(1024 * 1024 + 1).read_to_end(&mut data).await?;
                Ok::<_, std::io::Error>(data)
            };
            let capture_err = async {
                let mut data = Vec::new();
                stderr.take(1024 * 1024 + 1).read_to_end(&mut data).await?;
                Ok::<_, std::io::Error>(data)
            };
            let (outcome, output, error) = tokio::join!(effect, capture_out, capture_err);
            let (outcome, output, error) = (outcome?, output?, error?);
            if output.len() > 1024 * 1024 || error.len() > 1024 * 1024 {
                return Err("provider fixture output exceeds bound".into());
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((outcome, output, error))
        })
        .await;
        // Test teardown joins exact provider removal even when an assertion or
        // effect fails. Never remove unrelated Pods/volumes by name prefix/age.
        match result {
            Ok(Ok((exit, out, err))) => Ok((resource, exit, out, err)),
            Ok(Err(error)) => {
                self.cleanup(&resource).await?;
                Err(error)
            }
            Err(error) => {
                self.cleanup(&resource).await?;
                Err(error.into())
            }
        }
    }
}
#[tokio::test]
#[ignore = "requires explicitly owned disposable Kubernetes cluster, namespace-scoped config and exact candidate images; required provider CI includes this test"]
async fn native_stdin_separate_capture_and_no_replay() -> TestResult {
    let f = Fixture::new().await?;
    let spec = f.spec(vec!["/usr/local/libexec/attune/echo".into()])?;
    let (exit, out, err) = f
        .run(&spec, br#"{"message":"owned-native-exact-stdin"}"#.to_vec())
        .await?;
    assert_eq!(exit.exit_code, 0);
    assert!(!exit.oom_killed);
    assert_eq!(out, b"owned-native-exact-stdin");
    assert!(err.is_empty());
    Ok(())
}

async fn candidate_copy_and_artifact_reuse(f: &Fixture) -> TestResult {
    let pack = f.input(
        LaunchInputKind::Pack,
        vec![("source.txt", b"immutable candidate source".to_vec(), false)],
    )?;
    let runtime = f.input(
        LaunchInputKind::Runtime,
        vec![(
            "project/source.txt",
            b"published runtime project".to_vec(),
            false,
        )],
    )?;
    // Reuse the same immutable inputs in distinct private attempts. Run these
    // sequentially inside the existing case; do not add another concurrent Pod
    // to the owned node's resource budget.
    for _ in 0..2 {
        let mut spec = f.spec(vec![
            "/usr/bin/python3".into(),
            "-c".into(),
            include_str!("support/candidate_copy.py").into(),
        ])?;
        spec.purpose = LaunchPurpose::CandidateTest;
        spec.target = LaunchTarget::CandidateTest {
            install: 1,
            test_job: 2,
            attempt: Uuid::new_v4(),
        };
        spec.pack_access = PackAccess::CandidateCopy;
        spec.inputs = vec![pack.clone(), runtime.clone()];
        spec.invocation.working_root = LaunchRoot::Pack;
        let (resource, exit, out, err) = f.run_retained(&spec, vec![]).await?;
        let result: TestResult = async {
            if exit.exit_code != 0
                || exit.oom_killed
                || out != b"owned-candidate-private-copy"
                || !err.is_empty()
            {
                return Err("candidate private copy or published Runtime boundary failed".into());
            }
            let mut file = f
                .launcher
                .read_stopped_artifact(
                    &spec,
                    &resource,
                    &attune_common::artifact_transport::ValidatedRelativePath::new(
                        "candidate.txt",
                    )?,
                    64,
                )
                .await?
                .ok_or("candidate selected artifact missing")?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut bytes)?;
            if bytes != b"owned-candidate-artifact" {
                return Err("candidate selected artifact changed".into());
            }
            Ok(())
        }
        .await;
        // This effects fixture accepts the selected bytes; it does not model
        // the owning worker's domain publication/completion receipt.
        f.launcher.remove_stopped(&spec, &resource).await?;
        result?;
    }
    for (pin, relative, expected) in [
        (
            &pack,
            "source.txt",
            b"immutable candidate source".as_slice(),
        ),
        (
            &runtime,
            "project/source.txt",
            b"published runtime project".as_slice(),
        ),
    ] {
        let mut archive = f.cache.open_input_archive(pin)?;
        let root = f.cache.install_input(pin, &mut archive)?;
        if std::fs::read(root.join(relative))? != expected {
            return Err("candidate poisoned a retained immutable input".into());
        }
    }
    Ok(())
}
#[tokio::test]
#[ignore = "requires explicitly owned disposable Kubernetes cluster, namespace-scoped config and exact candidate images; required provider CI includes this test"]
async fn python_setup_archive_reuse_preserves_console_script_and_absolute_link() -> TestResult {
    let f = Fixture::new().await?;
    // Local wheel, pip and ensurepip only; no network/package-index dependency.
    let program = r#"import os, subprocess, sys, zipfile
root = '/opt/attune/runtime'
assert os.getcwd() == root + '/project'
assert open('/opt/attune/pack/source.txt').read() == 'immutable source'
with open('source.txt', 'w') as f: f.write('private setup update')
with open('generated.lock', 'w') as f: f.write('generated lock')
assert open('/opt/attune/pack/source.txt').read() == 'immutable source'
subprocess.run([sys.executable, '-m', 'venv', '--copies', root], check=True)
files = {
 'ownedtool.py': 'def main():\n print("owned-console-ok")\n',
 'ownedtool-1.0.dist-info/METADATA': 'Metadata-Version: 2.1\nName: ownedtool\nVersion: 1.0\n',
 'ownedtool-1.0.dist-info/WHEEL': 'Wheel-Version: 1.0\nGenerator: owned-fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
 'ownedtool-1.0.dist-info/entry_points.txt': '[console_scripts]\nowned-console = ownedtool:main\n',
}
files['ownedtool-1.0.dist-info/RECORD'] = ''.join(k + ',,\n' for k in files) + 'ownedtool-1.0.dist-info/RECORD,,\n'
wheel = '/tmp/ownedtool-1.0-py3-none-any.whl'
with zipfile.ZipFile(wheel, 'w') as z:
 for path, data in files.items(): z.writestr(path, data)
subprocess.run([root + '/bin/python', '-m', 'pip', 'install', '--no-index', '--no-deps', wheel], check=True)
os.symlink(root + '/bin/python', root + '/bin/internal-python')
assert open(root + '/bin/owned-console').readline().strip() == '#!' + root + '/bin/python'
"#;
    let mut setup = f.spec(Vec::new())?;
    let runner = {
        use std::io::Read;
        let file = std::fs::File::open(required("ATTUNE_ISOLATION_TEST_RUNNER_PATH")?)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() > 64 * 1024 * 1024 {
            return Err("owned public Runner exceeds fixture input bound".into());
        }
        let mut bytes = Vec::new();
        file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
            return Err("owned public Runner exceeds fixture input bound".into());
        }
        bytes
    };
    let runner_name = format!("linux-{}", setup.platform.native_arch());
    setup
        .inputs
        .push(f.input(LaunchInputKind::Runner, vec![(&runner_name, runner, true)])?);
    setup.inputs.push(f.input(
        LaunchInputKind::Pack,
        vec![("source.txt", b"immutable source".to_vec(), false)],
    )?);
    setup.invocation.argv = vec![
        attune_workload_backend::planning::runner_program(setup.platform),
        "setup".into(),
    ];
    setup.invocation.working_root = LaunchRoot::Stage;
    let plan = attune_workload_backend::planning::SetupPlan {
        version: 1,
        environment: BTreeMap::from([("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())]),
        steps: vec![attune_workload_backend::planning::SetupStep {
            argv: vec!["/usr/bin/python3".into(), "-c".into(), program.into()],
            if_pack_file: None,
        }],
    };
    plan.validate()?;
    setup.purpose = LaunchPurpose::Setup;
    setup.target = LaunchTarget::Setup {
        admission: 1,
        attempt: Uuid::new_v4(),
    };
    setup.stage = Some(PrivateStage {
        cache_key: "owned-python-setup".into(),
        quota_bytes: 128 * 1024 * 1024,
        quota_files: 4096,
    });
    eprintln!("stage fixture phase=setup-run");
    let (resource, exit, _, _) = f.run_retained(&setup, serde_json::to_vec(&plan)?).await?;
    let result = async {
        if exit.exit_code != 0 || exit.oom_killed {
            return Err("owned Python setup did not succeed".into());
        }
        eprintln!("stage fixture phase=archive-initial");
        let mut archive = f.launcher.archive_stopped_stage(&setup, &resource).await?;
        // Repeated transfer returns the exact retained body, not a regenerated ZIP.
        use sha2::{Digest, Sha256};
        use std::io::{Read, Seek};
        fn hash(file: &mut std::fs::File) -> std::io::Result<String> {
            file.rewind()?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            file.rewind()?;
            Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
        }
        let digest = hash(&mut archive)?;
        eprintln!("stage fixture phase=archive-replay");
        let mut replay = f.launcher.archive_stopped_stage(&setup, &resource).await?;
        if digest != hash(&mut replay)? {
            return Err("stage transfer replay changed exact bytes".into());
        }
        let pin = PinnedLaunchInput {
            pin_id: 1,
            object_id: 1,
            object_version: "owned-setup-publication".into(),
            kind: LaunchInputKind::Runtime,
            size_bytes: archive.metadata()?.len(),
            sha256: Sha256Digest::parse(digest)?,
            max_expanded_bytes: 128 * 1024 * 1024,
            max_files: 4096,
        };
        eprintln!("stage fixture phase=cache-install");
        f.cache.install_input(&pin, &mut archive)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(pin)
    }
    .await;
    let pin = match result {
        Ok(pin) => pin,
        Err(error) => {
            f.cleanup(&resource).await?;
            return Err(error);
        }
    };
    // This fixture models provider effects, not a domain publication receipt.
    // The exact archive is now accepted by the owned cache; acknowledge builder
    // removal before starting reuse, so reuse cannot depend on its live helper
    // or keep the stopped builder's Pod resources reserved while starting anew.
    eprintln!("stage fixture phase=setup-remove");
    f.launcher.remove_stopped(&setup, &resource).await?;
    let verify = r#"import os, subprocess
assert os.getcwd() == '/opt/attune/runtime/project'
assert open('generated.lock').read() == 'generated lock'
assert open('source.txt').read() == 'private setup update'
assert open('/opt/attune/pack/source.txt').read() == 'immutable source'
subprocess.run(['/opt/attune/runtime/bin/owned-console'], check=True)
subprocess.run(['/opt/attune/runtime/bin/internal-python', '-c', 'print("owned-link-ok")'], check=True)
try: open('/opt/attune/runtime/poison', 'w')
except OSError as e:
 assert e.errno == 30
else: raise AssertionError('published runtime writable')
"#;
    let mut action = f.spec(vec!["/usr/bin/python3".into(), "-c".into(), verify.into()])?;
    action.inputs.push(pin);
    action.inputs.push(
        setup
            .inputs
            .iter()
            .find(|p| p.kind == LaunchInputKind::Pack)
            .unwrap()
            .clone(),
    );
    action.invocation.working_root = LaunchRoot::Runtime;
    eprintln!("stage fixture phase=runtime-start-and-seed");
    let (exit, out, err) = f.run(&action, Vec::new()).await?;
    if exit.exit_code != 0
        || exit.oom_killed
        || out != b"owned-console-ok\nowned-link-ok\n"
        || !err.is_empty()
    {
        return Err("published console script/link/readonly regression failed".into());
    }
    // A failed dependency/hook stops the same fixed helper immediately; no
    // successful stage publication or following hook may be fabricated.
    let mut failed = setup.clone();
    failed.intent = Uuid::new_v4();
    failed.target = LaunchTarget::Setup {
        admission: 1,
        attempt: Uuid::new_v4(),
    };
    let mut failed_plan = plan;
    failed_plan.steps = vec![
        attune_workload_backend::planning::SetupStep {
            argv: vec!["/bin/sh".into(), "-c".into(), "exit 17".into()],
            if_pack_file: None,
        },
        attune_workload_backend::planning::SetupStep {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf owned-must-not-run".into(),
            ],
            if_pack_file: None,
        },
    ];
    eprintln!("stage fixture phase=failed-step-stops-publication");
    let (resource, exit, out, _) = f
        .run_retained(&failed, serde_json::to_vec(&failed_plan)?)
        .await?;
    let denied = f
        .launcher
        .archive_stopped_stage(&failed, &resource)
        .await
        .is_err();
    f.launcher.remove_stopped(&failed, &resource).await?;
    if exit.exit_code == 0 || !out.is_empty() || !denied {
        return Err("failed setup step continued or permitted publication".into());
    }
    eprintln!("stage fixture phase=candidate-copy-and-artifact-reuse");
    candidate_copy_and_artifact_reuse(&f).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires explicitly owned disposable Kubernetes cluster, namespace-scoped config and exact candidate images; required provider CI includes this test"]
async fn actual_bash_procfs_python_and_selected_environment() -> TestResult {
    let f = Fixture::new().await?;
    let script = r#"test -e /proc/self/fd && test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token && test ! -e /var/run/docker.sock && test ! -e /opt/attune/transfer && test ! -e /cache && mkdir /tmp/artifacts/selected && printf owned-file > /tmp/artifacts/selected/content && : > /tmp/artifacts/empty && cat <(printf owned-procfs) && /usr/bin/python3 -c 'import os,sys,threading; assert os.environ["SELECTED"]=="owned-selected-value"; assert not any(k.startswith(("ATTUNE__","KUBERNETES_","DOCKER_")) for k in os.environ); t=threading.Thread(target=lambda:None);t.start();t.join(); sys.stderr.write("owned-separate-stderr")'"#;
    let spec = f.spec(vec!["/bin/bash".into(), "-c".into(), script.into()])?;
    let (resource, exit, out, err) = f.run_retained(&spec, Vec::new()).await?;
    let result = async {
        if exit.exit_code != 0 || out != b"owned-procfs" || err != b"owned-separate-stderr" {
            return Err("procfs/environment/separate-stream regression".into());
        }
        use attune_common::artifact_transport::ValidatedRelativePath;
        use std::io::Read;
        let selected = ValidatedRelativePath::new("selected/content")?;
        let mut file = f
            .launcher
            .read_stopped_artifact(&spec, &resource, &selected, 10)
            .await?
            .ok_or("selected artifact incorrectly missing")?;
        let mut body = Vec::new();
        file.read_to_end(&mut body)?;
        if body != b"owned-file" {
            return Err("selected artifact bytes changed".into());
        }
        let empty = ValidatedRelativePath::new("empty")?;
        let file = f
            .launcher
            .read_stopped_artifact(&spec, &resource, &empty, 0)
            .await?
            .ok_or("empty artifact incorrectly missing")?;
        if file.metadata()?.len() != 0 {
            return Err("empty artifact gained bytes".into());
        }
        if f.launcher
            .read_stopped_artifact(
                &spec,
                &resource,
                &ValidatedRelativePath::new("selected/not-found")?,
                10,
            )
            .await?
            .is_some()
        {
            return Err("missing selected artifact unexpectedly present".into());
        }
        if f.launcher
            .read_stopped_artifact(&spec, &resource, &selected, 9)
            .await
            .is_ok()
        {
            return Err("selected artifact exceeded remaining bound".into());
        }
        let mut changed = resource.clone();
        if let ResourceIdentity::Kubernetes { pod_uid, .. } = &mut changed {
            *pod_uid = Uuid::new_v4().to_string();
        }
        if f.launcher
            .read_stopped_artifact(&spec, &changed, &selected, 10)
            .await
            .is_ok()
        {
            return Err("changed Pod UID read artifact".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    if result.is_ok() {
        f.launcher.remove_stopped(&spec, &resource).await?;
    } else {
        f.cleanup(&resource).await?;
    }
    result?;
    inspect_installed_command_without_manager_discovery(&f).await?;
    let secrets: Api<Secret> = Api::namespaced(f.client.clone(), &f.policy.namespace);
    match secrets.list(&ListParams::default()).await {
        Err(kube::Error::Api(e)) if e.code == 403 => {}
        _ => return Err("launcher credential unexpectedly permits Secret reads".into()),
    };
    Ok(())
}

// These are fixed provider effects, not source admission or a cache proof. The
// trusted caller must bind the result to the same image/platform/inputs before
// using it. No action parameters or action API token enter the inspection Pod.
async fn inspect_installed_command_without_manager_discovery(f: &Fixture) -> TestResult {
    use std::io::Read;
    let file = std::fs::File::open(required("ATTUNE_ISOLATION_TEST_RUNNER_PATH")?)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > 64 * 1024 * 1024 {
        return Err("owned public Runner exceeds fixture input bound".into());
    }
    let mut runner = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut runner)?;
    if runner.is_empty() || runner.len() > 64 * 1024 * 1024 {
        return Err("owned public Runner exceeds fixture input bound".into());
    }
    let mut probe = f.spec(Vec::new())?;
    probe.purpose = LaunchPurpose::RuntimeProbe;
    probe.target = LaunchTarget::RuntimeProbe {
        admission: 1,
        attempt: Uuid::new_v4(),
    };
    probe.inputs.push(f.input(
        LaunchInputKind::Pack,
        vec![(
            "actions/installed.yaml",
            b"ref: owned.installed\nrunner_type: native\nlaunch:\n  type: command\n  program: bash\nparameter_delivery: none\n".to_vec(),
            false,
        )],
    )?);
    let name = format!("linux-{}", probe.platform.native_arch());
    probe
        .inputs
        .push(f.input(LaunchInputKind::Runner, vec![(&name, runner, true)])?);
    probe.invocation.argv = vec![
        attune_workload_backend::planning::runner_program(probe.platform),
        "inspect-command-program".into(),
        "bash".into(),
    ];
    probe.invocation.working_root = LaunchRoot::Pack;
    probe.invocation.env = BTreeMap::from([("PATH".into(), "/bin:/usr/bin".into())]);
    let (exit, body, err) = f.run(&probe, Vec::new()).await?;
    if exit.exit_code != 0 || exit.oom_killed || !err.is_empty() || body.len() > 16 * 1024 {
        return Err("installed program inspection failed or exceeded its bound".into());
    }
    let selected: attune_common::action_cli::SelectedProgram = serde_json::from_slice(&body)?;
    if !selected.path.is_absolute()
        || selected.path.starts_with(PACK_ROOT)
        || selected.invocation_name != "bash"
        || selected.artifact.is_some()
        || selected.variant.is_some()
    {
        return Err("installed program selection changed its static contract".into());
    }
    Sha256Digest::parse(&selected.sha256)?;
    // Reuse the exact admitted image and pins for execution. A source-side
    // authority proof is deliberately not modeled in this provider fixture.
    let mut execution = probe.clone();
    execution.intent = Uuid::new_v4();
    execution.purpose = LaunchPurpose::Action;
    execution.target = f.spec(Vec::new())?.target;
    execution.invocation.argv = attune_workload_backend::planning::cli_exec_argv(
        execution.platform,
        &selected,
        &[
            "-c".into(),
            "test \"$0\" = bash && test -z \"${ATTUNE_API_TOKEN+x}\" && printf owned-static-argv0"
                .into(),
        ],
    )?;
    let (exit, out, err) = f.run(&execution, Vec::new()).await?;
    if exit.exit_code != 0 || exit.oom_killed || out != b"owned-static-argv0" || !err.is_empty() {
        return Err("inspected program did not preserve hash/static argv0 execution".into());
    }
    // Reuse the same immutable image and public Runner. Recipe discovery runs
    // only its tool commands, with a real defining input and no action stdin.
    let recipe = runtime_recipe::recipe();
    probe.inputs.retain(|pin| pin.kind != LaunchInputKind::Pack);
    let runner_inputs = probe.inputs.clone();
    for matches in [true, false] {
        let mut selected = recipe.clone();
        if !matches {
            selected.tools[0].version_pattern = "must-not-match-(?<version>[0-9]+)".into();
        }
        probe.intent = Uuid::new_v4();
        probe.target = LaunchTarget::RuntimeProbe {
            admission: 1,
            attempt: Uuid::new_v4(),
        };
        let mut pack = f.input(
            LaunchInputKind::Pack,
            vec![("runtimes/owned.json", serde_json::to_vec(&selected)?, false)],
        )?;
        pack.pin_id = 2;
        probe.inputs = runner_inputs.clone();
        probe.inputs.push(pack);
        probe.invocation.argv = attune_workload_backend::planning::runtime_tool_probe_argv(
            probe.platform,
            &selected,
            "/usr/bin:/bin",
        )?;
        let (exit, out, err) = f.run(&probe, Vec::new()).await?;
        if exit.oom_killed
            || (matches
                && (exit.exit_code != 0 || !err.is_empty() || !runtime_recipe::valid_result(&out)))
            || (!matches
                && (exit.exit_code == 0
                    || !out.is_empty()
                    || err.len() > 1024
                    || String::from_utf8_lossy(&err).contains("must-not-run-private")))
        {
            return Err(
                "isolated runtime recipe discovery or rejected-version control failed".into(),
            );
        }
    }
    Ok(())
}
#[tokio::test]
#[ignore = "requires explicitly owned disposable Kubernetes cluster, namespace-scoped config and exact candidate images; required provider CI includes this test"]
async fn reconcile_missing_intent_never_creates_and_changed_uid_never_starts() -> TestResult {
    let f = Fixture::new().await?;
    let spec = f.spec(vec!["/bin/true".into()])?;
    assert!(f
        .launcher
        .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
        .await
        .is_err());
    let resource = f
        .launcher
        .prepare(&spec, ProcessCreateDisposition::CreateOnce)
        .await?;
    let mut revoked_policy = f.policy.clone();
    revoked_policy.allowed_images.clear();
    let cache = CacheStore::open(
        f._root.path().join("revoked-cache"),
        CacheLimits {
            compressed_bytes: 1024,
            expanded_bytes: 4096,
            files: 32,
        },
    )?;
    let revoked = KubernetesLauncher::new(f.client.clone(), revoked_policy, Arc::new(cache));
    let recovered = revoked
        .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
        .await?;
    assert_eq!(recovered, resource);
    let denied_run = revoked.start(&spec, &resource).await.is_err();
    let mut changed = resource.clone();
    if let ResourceIdentity::Kubernetes { pod_uid, .. } = &mut changed {
        *pod_uid = Uuid::new_v4().to_string();
    }
    let rejected = f.launcher.start(&spec, &changed).await.is_err();
    let rejected_cleanup = f.launcher.remove_stopped(&spec, &changed).await.is_err();
    let recovery = async {
        // A recovered manager has no ProcessIo/ProcessControl. Wait only for
        // the fixed gate to be running; never attach, seed or open its stdin.
        let pods: Api<Pod> = Api::namespaced(f.client.clone(), &f.policy.namespace);
        let ResourceIdentity::Kubernetes {
            pod_name, pod_uid, ..
        } = &resource
        else {
            return Err("expected exact owned Pod".into());
        };
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let pod = pods.get(pod_name).await?;
                if pod.metadata.uid.as_ref() != Some(pod_uid) {
                    return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                        "recovered Pod UID changed".into(),
                    );
                }
                if pod
                    .status
                    .and_then(|status| status.container_statuses)
                    .unwrap_or_default()
                    .iter()
                    .any(|status| {
                        status.name == "payload"
                            && status
                                .state
                                .as_ref()
                                .is_some_and(|state| state.running.is_some())
                    })
                {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await??;
        let mut public_spec = spec.clone();
        public_spec.invocation.argv.clear();
        public_spec.invocation.env.clear();
        assert!(revoked
            .stop_retained(&public_spec, &changed, LaunchStopReason::Cancelled)
            .await
            .is_err());
        let outcome = revoked
            .stop_retained(&public_spec, &resource, LaunchStopReason::Cancelled)
            .await?;
        if outcome.exit_code == 0 || outcome.oom_killed {
            return Err("unopened retained gate must stop without running the action".into());
        }
        let pod = pods.get(pod_name).await?;
        if pod.metadata.uid.as_ref() != Some(pod_uid)
            || !pod
                .status
                .and_then(|status| status.container_statuses)
                .unwrap_or_default()
                .iter()
                .any(|status| {
                    status.name == "transfer"
                        && status
                            .state
                            .as_ref()
                            .is_some_and(|state| state.running.is_some())
                })
        {
            return Err("retained stop removed the transfer helper/private staging".into());
        }
        revoked.remove_stopped(&public_spec, &resource).await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    if recovery.is_err() {
        f.cleanup(&resource).await?;
    }
    recovery?;
    assert!(rejected);
    assert!(rejected_cleanup);
    assert!(denied_run);
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicitly owned disposable Kubernetes cluster, namespace-scoped config and exact candidate images; required provider CI includes this test"]
async fn actual_owned_host_credential_process_files_and_signaling_are_hidden() -> TestResult {
    let f = Fixture::new().await?;
    let token = format!("owned-dummy-{}", Uuid::new_v4());
    let secret_file = f._root.path().join("owned-host-only-credential");
    std::fs::write(&secret_file, token.as_bytes())?;
    let mut dummy = tokio::process::Command::new("/bin/sleep")
        .arg("3600")
        .env("OWNED_PROCESS_TOKEN", &token)
        .kill_on_drop(true)
        .spawn()?;
    let pid = dummy.id().ok_or("owned dummy did not start")?;
    let expected = Sha256Digest::of_bytes(token.as_bytes());
    // Only a digest of a newly generated dummy credential enters this test
    // invocation, not the credential itself or any real manager secret.
    let script = r#"import os,pathlib,signal,sys,hashlib
pid=int(sys.argv[1]); expected=sys.argv[2]; host_file=sys.argv[3]
assert not pathlib.Path(host_file).exists()
status=pathlib.Path('/proc/self/status').read_text()
assert 'NoNewPrivs:\t1' in status and 'CapEff:\t0000000000000000' in status
for path in pathlib.Path('/proc').glob('[0-9]*/environ'):
    try: entries=path.read_bytes().split(b'\0')
    except (PermissionError,FileNotFoundError,ProcessLookupError): continue
    for entry in entries:
        if entry.startswith(b'OWNED_PROCESS_TOKEN='):
            assert hashlib.sha256(entry.split(b'=',1)[1]).hexdigest()!=expected
# Namespace-local PID reuse is legitimate. If that ID exists it must have a
# different credential; otherwise signaling the host ID must fail locally.
if not pathlib.Path('/proc/'+str(pid)).exists():
    try: os.kill(pid,signal.SIGSTOP)
    except ProcessLookupError: pass
    else: raise AssertionError('host process signal escaped PID namespace')
try: pathlib.Path('/etc/attune-owned-write-probe').write_text('owned')
except OSError: pass
else: raise AssertionError('payload root is writable')
assert not pathlib.Path('/var/run/secrets/kubernetes.io/serviceaccount/token').exists()
assert not pathlib.Path('/var/run/docker.sock').exists()
sys.stdout.write('owned-host-boundary-pass')
"#;
    let result = async {
        let spec = f.spec(vec![
            "/usr/bin/python3".into(),
            "-c".into(),
            script.into(),
            pid.to_string(),
            expected.as_str().into(),
            secret_file.to_string_lossy().into_owned(),
        ])?;
        f.run(&spec, Vec::new()).await
    }
    .await;
    // Always settle this owned child, including after a failed isolation probe.
    let state = std::fs::read_to_string(format!("/proc/{pid}/status"));
    let stopped = state.as_ref().is_ok_and(|s| {
        s.lines()
            .any(|line| line.starts_with("State:") && line.contains('T'))
    });
    let _kill_request = dummy.start_kill();
    tokio::time::timeout(Duration::from_secs(10), dummy.wait()).await??;
    let (exit, out, err) = result?;
    assert!(!stopped, "owned host process was signaled from the payload");
    assert_eq!(exit.exit_code, 0);
    assert_eq!(out, b"owned-host-boundary-pass");
    assert!(err.is_empty());
    Ok(())
}
