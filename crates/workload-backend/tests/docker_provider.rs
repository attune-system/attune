//! Actual concrete Docker adapter effects in an explicitly owned, colocated
//! test manager. Missing prerequisites fail. No domain completion is inferred.
use attune_common::{
    artifact_transport::ValidatedRelativePath,
    isolated_process::{ProcessCreateDisposition, ProcessIo, ProcessLauncher, ProcessSpec},
    workload_config::{WorkloadBackendConfig, WorkloadConfig, WorkloadStorageLimits},
    workload_launch::*,
    workload_stdin::LaunchStdinPayload,
};
use attune_workload_backend::{
    cache::CacheStore,
    configuration::{configure, NativeBackend},
    engine::{DockerEngine, DockerLauncher},
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;
use uuid::Uuid;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn required(name: &str) -> TestResult<String> {
    std::env::var(name).map_err(|_| format!("missing owned Docker prerequisite: {name}").into())
}
fn image(name: &str) -> TestResult<PinnedImage> {
    let reference = required(name)?;
    let digest = reference
        .strip_prefix("sha256:")
        .or_else(|| reference.rsplit_once("@sha256:").map(|(_, digest)| digest))
        .ok_or("owned candidate image must have an immutable reference")?;
    Ok(PinnedImage {
        sha256: Sha256Digest::parse(digest)?,
        reference,
    })
}
struct Fixture {
    launcher: Arc<DockerLauncher>,
    cache: Arc<CacheStore>,
    image: PinnedImage,
    platform: LaunchPlatform,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        let socket = PathBuf::from(required("ATTUNE_ISOLATION_TEST_DOCKER_SOCKET")?);
        let engine = DockerEngine::connect(&socket)?;
        let binding = engine.observed_binding(1).await?;
        let platform = match required("ATTUNE_ISOLATION_TEST_PLATFORM")?.as_str() {
            "linux/amd64" => LaunchPlatform::LinuxAmd64,
            "linux/arm64" => LaunchPlatform::LinuxArm64,
            _ => return Err("invalid owned Docker platform".into()),
        };
        if binding.platform != platform {
            return Err("owned Docker daemon platform mismatch".into());
        }
        let image = image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE")?;
        let helper_image = image.clone();
        let mut source = std::fs::File::open(required("ATTUNE_ISOLATION_TEST_RUNNER_PATH")?)?;
        let metadata = source.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 64 * 1024 * 1024 {
            return Err("owned public Runner exceeds fixture input bound".into());
        }
        let mut archive = tempfile::NamedTempFile::new()?;
        {
            let mut zip = zip::ZipWriter::new(archive.as_file_mut());
            zip.start_file(
                format!("linux-{}", platform.native_arch()),
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .unix_permissions(0o755),
            )?;
            let count = std::io::copy(
                &mut Read::by_ref(&mut source).take(metadata.len() + 1),
                &mut zip,
            )?;
            if count != metadata.len() {
                return Err("owned public Runner changed during packaging".into());
            }
            zip.finish()?;
        }
        archive.as_file_mut().rewind()?;
        let mut body = Vec::new();
        archive.as_file_mut().read_to_end(&mut body)?;
        let helper = PinnedLaunchInput {
            pin_id: 1,
            object_id: 1,
            object_version: "owned-runner".into(),
            kind: LaunchInputKind::Runner,
            size_bytes: body.len() as u64,
            sha256: Sha256Digest::of_bytes(&body),
            max_expanded_bytes: metadata.len(),
            max_files: 1,
        };
        let config = WorkloadConfig {
            image: image.clone(),
            platform,
            cache_root: PathBuf::from(required("ATTUNE_ISOLATION_TEST_CACHE_ROOT")?),
            memory_bytes: 256 * 1024 * 1024,
            scratch_bytes: 64 * 1024 * 1024,
            storage: WorkloadStorageLimits {
                compressed_bytes: 128 * 1024 * 1024,
                expanded_bytes: 256 * 1024 * 1024,
                entries: 20_000,
                artifacts: Default::default(),
            },
            helper,
            helper_archive: archive.path().into(),
            backend: WorkloadBackendConfig::Docker {
                socket,
                cache_volume: required("ATTUNE_ISOLATION_TEST_CACHE_VOLUME")?,
                network: required("ATTUNE_ISOLATION_TEST_NETWORK")?,
                binding,
                pids_limit: 128,
                helper_image,
            },
        };
        // Exact same manager path with another owned volume must fail binding.
        let mut wrong = config.clone();
        if let WorkloadBackendConfig::Docker { cache_volume, .. } = &mut wrong.backend {
            *cache_volume = required("ATTUNE_ISOLATION_TEST_WRONG_VOLUME")?;
        }
        if configure(&wrong).await.is_ok() {
            return Err("wrong manager/Engine cache binding passed".into());
        }
        let configured = configure(&config).await?;
        let NativeBackend::Docker(launcher) = configured.backend else {
            return Err("unexpected backend".into());
        };
        if config.cache_root != std::path::Path::new("/var/lib/attune/launch") {
            return Err("owned Docker fixture cache mount does not match fixed test path".into());
        }
        std::fs::write(
            config.cache_root.join("manager-only-credential"),
            b"owned-manager-dummy-credential",
        )?;
        Ok(Self {
            launcher,
            cache: configured.cache,
            image,
            platform,
        })
    }
    fn spec(&self, argv: Vec<String>) -> ProcessSpec {
        ProcessSpec {
            intent: Uuid::new_v4(),
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution: 1,
                worker: 2,
                worker_instance: Uuid::new_v4(),
                attempt: Uuid::new_v4(),
            },
            image: self.image.clone(),
            platform: self.platform,
            inputs: vec![],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(30_000),
            invocation: LaunchInvocation {
                argv,
                env: BTreeMap::from([("SELECTED".into(), "owned-selected-value".into())]),
                working_root: LaunchRoot::Scratch,
            },
        }
    }
}
async fn run(io: ProcessIo, input: Vec<u8>) -> TestResult<(LaunchExitOutcome, Vec<u8>, Vec<u8>)> {
    let control = io.control;
    let mut stdout = io.stdout;
    let stderr = io.stderr;
    let effect = async {
        let mut byte = [0];
        if tokio::time::timeout(Duration::from_millis(100), stdout.read(&mut byte))
            .await
            .is_ok()
        {
            return Err("workload executed or capture ended before private gate".into());
        }
        let out = async {
            let mut bytes = Vec::new();
            stdout.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        };
        let err = async {
            let mut bytes = Vec::new();
            stderr.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        };
        let execute = async {
            control
                .write_stdin_once(LaunchStdinPayload::from_bytes(input)?)
                .await?;
            if control
                .write_stdin_once(LaunchStdinPayload::from_bytes(vec![])?)
                .await
                .is_ok()
            {
                return Err("stdin gate replayed".into());
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(control.wait().await?)
        };
        let (outcome, out, err) = tokio::join!(execute, out, err);
        let (outcome, out, err) = (outcome?, out?, err?);
        if out.len() > 1024 * 1024 || err.len() > 1024 * 1024 {
            return Err("fixture capture exceeded bound".into());
        }
        Ok((outcome, out, err))
    };
    let result = tokio::time::timeout(Duration::from_secs(60), effect).await;
    match result {
        Ok(Ok(value)) => Ok(value),
        error => {
            control.stop(LaunchStopReason::Cancelled).await?;
            match error {
                Ok(Err(error)) => Err(error),
                Err(error) => Err(error.into()),
                _ => unreachable!(),
            }
        }
    }
}
#[tokio::test]
#[ignore = "requires owned colocated UID1000 manager/Unix Engine/cache volume and exact candidate image"]
async fn native_docker_gate_procfs_capture_files_and_recovery() -> TestResult {
    let f = Fixture::new().await?;
    let host_pid: u32 = required("ATTUNE_ISOLATION_TEST_DUMMY_PID")?.parse()?;
    if host_pid <= 256 {
        return Err("owned dummy PID needs distinct namespace test range".into());
    }
    let script = format!(
        r#"set -e
    test -e /proc/self/fd
    test ! -e /var/run/docker.sock
    test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token
    test ! -e /var/lib/attune/launch/manager-only-credential
    test ! -e /proc/{host_pid}/environ
    ! kill -TERM {host_pid} 2>/dev/null
    test "$(cat <(printf owned-procfs))" = owned-procfs
    test ! -e /tmp/attune-healthcheck
    /usr/bin/python3 -c 'import os,threading; assert os.environ["SELECTED"]=="owned-selected-value"; assert not any(k.startswith(("ATTUNE__","DOCKER_","KUBERNETES_")) for k in os.environ); t=threading.Thread(target=lambda:None);t.start();t.join()'
    mkdir /tmp/artifacts/selected
    printf owned-file >/tmp/artifacts/selected/content
    : >/tmp/artifacts/empty
    printf owned-before-stdin
    /usr/local/libexec/attune/echo
    printf owned-stderr >&2
    "#
    );
    let spec = f.spec(vec!["/bin/bash".into(), "-c".into(), script]);
    if f.launcher
        .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
        .await
        .is_ok()
    {
        return Err("missing create intent was recreated".into());
    }
    let resource = f
        .launcher
        .prepare(&spec, ProcessCreateDisposition::CreateOnce)
        .await?;
    let result = async {
        let mut wrong = resource.clone();
        if let attune_common::isolated_process::ResourceIdentity::Docker { container_id, .. } =
            &mut wrong
        {
            *container_id = "a".repeat(64);
        }
        if f.launcher.start(&spec, &wrong).await.is_ok() {
            return Err("wrong container identity started".into());
        }
        let io = f.launcher.start(&spec, &resource).await?;
        let (exit, out, err) = run(io, br#"{"message":"owned-native"}"#.to_vec()).await?;
        if exit.exit_code != 0
            || exit.oom_killed
            || out != b"owned-before-stdinowned-native"
            || err != b"owned-stderr"
        {
            return Err("Docker procfs/environment/native/separate-capture regression".into());
        }
        let selected = ValidatedRelativePath::new("selected/content")?;
        let mut file = f
            .launcher
            .read_stopped_artifact(&spec, &resource, &selected, 10)
            .await?
            .ok_or("selected file missing")?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // A stale Engine Start after the acknowledged removal cannot reopen
        // this container's writable mounts. Inspect 404 alone was never used
        // to create the local removal receipt.
        let attune_common::isolated_process::ResourceIdentity::Docker { container_id, .. } =
            &resource
        else {
            return Err("unexpected Docker identity".into());
        };
        let client = reqwest::Client::builder()
            .unix_socket(PathBuf::from(required(
                "ATTUNE_ISOLATION_TEST_DOCKER_SOCKET",
            )?))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(10))
            .build()?;
        let response = client
            .post(format!(
                "http://engine/v1.47/containers/{container_id}/start"
            ))
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err("removed container accepted stale Start".into());
        }
        if bytes != b"owned-file" {
            return Err("selected artifact bytes changed".into());
        }
        if f.launcher
            .read_stopped_artifact(&spec, &resource, &selected, 9)
            .await
            .is_ok()
        {
            return Err("selected artifact overflow passed".into());
        }
        let empty = f
            .launcher
            .read_stopped_artifact(&spec, &resource, &ValidatedRelativePath::new("empty")?, 0)
            .await?
            .ok_or("empty file missing")?;
        if empty.metadata()?.len() != 0 {
            return Err("empty artifact changed".into());
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
            return Err("missing artifact unexpectedly returned file".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    // The fixture models no domain file publication receipt. No-files/accepted
    // fixture reads are followed by exact acknowledged provider removal only.
    f.launcher.remove_stopped(&spec, &resource).await?;
    result?;
    // Empty original EOF also traverses the private frame without a newline.
    let spec = f.spec(vec![
        "/bin/bash".into(),
        "-c".into(),
        "test -z \"$(cat)\" && ln -s /etc/passwd /tmp/artifacts/escape && printf owned-empty-eof"
            .into(),
    ]);
    let resource = f
        .launcher
        .prepare(&spec, ProcessCreateDisposition::CreateOnce)
        .await?;
    let result = async { run(f.launcher.start(&spec, &resource).await?, vec![]).await }.await;
    let tree_rejected = f
        .launcher
        .read_stopped_artifact(
            &spec,
            &resource,
            &ValidatedRelativePath::new("not-found")?,
            10,
        )
        .await
        .is_err();
    f.launcher.remove_stopped(&spec, &resource).await?;
    let (exit, out, err) = result?;
    if !tree_rejected {
        return Err("unsafe artifact tree became selected-file absence".into());
    }
    if exit.exit_code != 0 || out != b"owned-empty-eof" || !err.is_empty() {
        return Err("empty stdin framing changed".into());
    }
    // Keep the cache Arc in the fixture, never as a guest mount of its parent.
    drop(f.cache);
    Ok(())
}
