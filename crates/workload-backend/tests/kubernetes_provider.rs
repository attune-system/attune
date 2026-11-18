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
#[tokio::test]
#[ignore = "requires explicitly owned disposable Kubernetes cluster, namespace-scoped config and exact candidate images; required provider CI includes this test"]
async fn python_setup_archive_reuse_preserves_console_script_and_absolute_link() -> TestResult {
    let f = Fixture::new().await?;
    // Local wheel, pip and ensurepip only; no network/package-index dependency.
    let program = r#"import os, subprocess, sys, zipfile
root = '/opt/attune/runtime'
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
    let mut setup = f.spec(vec!["/usr/bin/python3".into(), "-c".into(), program.into()])?;
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
    let (resource, exit, _, _) = f.run_retained(&setup, Vec::new()).await?;
    let result = async {
        if exit.exit_code != 0 || exit.oom_killed {
            return Err("owned Python setup did not succeed".into());
        }
        let mut archive = f.launcher.archive_stopped_stage(&setup, &resource).await?;
        // Repeated transfer returns the exact retained body, not a regenerated ZIP.
        use std::io::{Read, Seek};
        use sha2::{Digest, Sha256};
        fn hash(file: &mut std::fs::File) -> std::io::Result<String> {
            file.rewind()?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 { break; }
                hash.update(&buffer[..count]);
            }
            file.rewind()?;
            Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
        }
        let digest = hash(&mut archive)?;
        let mut replay = f.launcher.archive_stopped_stage(&setup, &resource).await?;
        if digest != hash(&mut replay)? {
            return Err("stage transfer replay changed exact bytes".into());
        }
        let pin = PinnedLaunchInput { pin_id: 1, object_id: 1,
            object_version: "owned-setup-publication".into(), kind: LaunchInputKind::Runtime,
            size_bytes: archive.metadata()?.len(), sha256: Sha256Digest::parse(digest)?,
            max_expanded_bytes: 128 * 1024 * 1024, max_files: 4096 };
        f.cache.install_input(&pin, &mut archive)?;
        let verify = r#"import os, subprocess
subprocess.run(['/opt/attune/runtime/bin/owned-console'], check=True)
subprocess.run(['/opt/attune/runtime/bin/internal-python', '-c', 'print("owned-link-ok")'], check=True)
try: open('/opt/attune/runtime/poison', 'w')
except OSError as e:
 assert e.errno == 30
else: raise AssertionError('published runtime writable')
"#;
        let mut action = f.spec(vec!["/usr/bin/python3".into(), "-c".into(), verify.into()])?;
        action.inputs.push(pin);
        let (exit, out, err) = f.run(&action, Vec::new()).await?;
        if exit.exit_code != 0 || exit.oom_killed
            || out != b"owned-console-ok\nowned-link-ok\n" || !err.is_empty()
        {
            return Err("published console script/link/readonly regression failed".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }.await;
    if result.is_ok() {
        f.launcher.remove_stopped(&setup, &resource).await?;
    } else {
        f.cleanup(&resource).await?;
    }
    result
}
#[tokio::test]
#[ignore = "requires explicitly owned disposable Kubernetes cluster, namespace-scoped config and exact candidate images; required provider CI includes this test"]
async fn actual_bash_procfs_python_and_selected_environment() -> TestResult {
    let f = Fixture::new().await?;
    let script = r#"test -e /proc/self/fd && test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token && test ! -e /var/run/docker.sock && test ! -e /opt/attune/transfer && test ! -e /cache && cat <(printf owned-procfs) && /usr/bin/python3 -c 'import os,sys,threading; assert os.environ["SELECTED"]=="owned-selected-value"; assert not any(k.startswith(("ATTUNE__","KUBERNETES_","DOCKER_")) for k in os.environ); t=threading.Thread(target=lambda:None);t.start();t.join(); sys.stderr.write("owned-separate-stderr")'"#;
    let spec = f.spec(vec!["/bin/bash".into(), "-c".into(), script.into()])?;
    let (exit, out, err) = f.run(&spec, Vec::new()).await?;
    assert_eq!(exit.exit_code, 0);
    assert_eq!(out, b"owned-procfs");
    assert_eq!(err, b"owned-separate-stderr");
    let secrets: Api<Secret> = Api::namespaced(f.client.clone(), &f.policy.namespace);
    match secrets.list(&ListParams::default()).await {
        Err(kube::Error::Api(e)) if e.code == 403 => {}
        _ => return Err("launcher credential unexpectedly permits Secret reads".into()),
    };
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
    f.cleanup(&resource).await?;
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
