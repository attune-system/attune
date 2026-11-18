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
        let launcher = KubernetesLauncher::new(client.clone(), policy.clone(), Arc::new(cache));
        Ok(Self {
            client,
            launcher,
            policy,
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
        let cleanup = self.cleanup(&resource).await;
        cleanup?;
        result?
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
    let mut changed = resource.clone();
    if let ResourceIdentity::Kubernetes { pod_uid, .. } = &mut changed {
        *pod_uid = Uuid::new_v4().to_string();
    }
    let rejected = f.launcher.start(&spec, &changed).await.is_err();
    f.cleanup(&resource).await?;
    assert!(rejected);
    Ok(())
}
