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
    runner: PinnedLaunchInput,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        Self::with_image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE").await
    }
    async fn with_image(image_variable: &str) -> TestResult<Self> {
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
        let image = image(image_variable)?;
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
            runner: configured.runner,
        })
    }
    fn input(
        &self,
        kind: LaunchInputKind,
        body: &mut std::fs::File,
    ) -> TestResult<PinnedLaunchInput> {
        use sha2::{Digest, Sha256};
        body.rewind()?;
        let mut hash = Sha256::new();
        let mut bytes = [0; 65536];
        loop {
            let count = body.read(&mut bytes)?;
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
        body.rewind()?;
        let pin = PinnedLaunchInput {
            pin_id: 2,
            object_id: 2,
            object_version: Uuid::new_v4().to_string(),
            kind,
            size_bytes: body.metadata()?.len(),
            sha256: Sha256Digest::parse(
                hash.finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            )?,
            max_expanded_bytes: 64 * 1024 * 1024,
            max_files: 4096,
        };
        self.cache.install_input(&pin, body)?;
        Ok(pin)
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
    python_setup_removed_builder_and_readonly_reuse(&f).await?;
    candidate_copy_and_artifact_reuse(&f).await?;
    node_threads_and_procfs_through_concrete_adapter().await?;
    // Keep the cache Arc in the fixture, never as a guest mount of its parent.
    drop(f.cache);
    Ok(())
}

async fn candidate_copy_and_artifact_reuse(f: &Fixture) -> TestResult {
    use std::io::Write;
    let mut pins = Vec::new();
    for (kind, path, bytes) in [
        (
            LaunchInputKind::Pack,
            "source.txt",
            b"immutable candidate source".as_slice(),
        ),
        (
            LaunchInputKind::Runtime,
            "project/source.txt",
            b"published runtime project".as_slice(),
        ),
    ] {
        let mut body = tempfile::tempfile()?;
        {
            let mut zip = zip::ZipWriter::new(&mut body);
            zip.start_file(
                path,
                zip::write::SimpleFileOptions::default().unix_permissions(0o644),
            )?;
            zip.write_all(bytes)?;
            zip.finish()?;
        }
        pins.push(f.input(kind, &mut body)?);
    }
    for _ in 0..2 {
        let mut spec = f.spec(vec![
            "/usr/bin/python3".into(),
            "-c".into(),
            include_str!("support/candidate_copy.py").into(),
        ]);
        spec.purpose = LaunchPurpose::CandidateTest;
        spec.target = LaunchTarget::CandidateTest {
            install: 1,
            test_job: 2,
            attempt: Uuid::new_v4(),
        };
        spec.pack_access = PackAccess::CandidateCopy;
        spec.inputs = pins.clone();
        spec.invocation.working_root = LaunchRoot::Pack;
        let resource = f
            .launcher
            .prepare(&spec, ProcessCreateDisposition::CreateOnce)
            .await?;
        let result: TestResult = async {
            let (exit, out, err) = run(f.launcher.start(&spec, &resource).await?, vec![]).await?;
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
                    &ValidatedRelativePath::new("candidate.txt")?,
                    64,
                )
                .await?
                .ok_or("candidate selected artifact missing")?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            if bytes != b"owned-candidate-artifact" {
                return Err("candidate selected artifact changed".into());
            }
            Ok(())
        }
        .await;
        // Byte acceptance in this effects fixture is not a domain receipt.
        // Teardown is still awaited after any failed assertion/effect.
        f.launcher.remove_stopped(&spec, &resource).await?;
        result?;
    }
    for (pin, relative, expected) in [
        (
            &pins[0],
            "source.txt",
            b"immutable candidate source".as_slice(),
        ),
        (
            &pins[1],
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

async fn node_threads_and_procfs_through_concrete_adapter() -> TestResult {
    // The owned harness already pins/pulls this image for the retained Engine
    // ABI coverage. Exercise it through the actual in-process adapter too,
    // before retiring that older Engine-only callsink.
    let f = Fixture::with_image("ATTUNE_TEST_NODE_IMAGE").await?;
    let spec = f.spec(vec![
        "node".into(),
        "-e".into(),
        r#"const assert=require('node:assert'); const fs=require('node:fs'); const {Worker}=require('node:worker_threads');
assert(fs.statSync('/proc/self/fd').isDirectory());
assert(!fs.existsSync('/var/run/docker.sock'));
assert(!fs.existsSync('/var/run/secrets/kubernetes.io/serviceaccount/token'));
assert.equal(process.env.SELECTED,'owned-selected-value');
assert(!Object.keys(process.env).some(k=>/^(ATTUNE__|DOCKER_|KUBERNETES_)/.test(k)));
const w=new Worker("require('node:worker_threads').parentPort.postMessage(42)",{eval:true});
w.once('message',n=>{assert.equal(n,42);process.stdout.write('owned-node-thread');process.stderr.write('owned-node-stderr')});
w.once('error',e=>{throw e});"#
            .into(),
    ]);
    let resource = f
        .launcher
        .prepare(&spec, ProcessCreateDisposition::CreateOnce)
        .await?;
    let result = async { run(f.launcher.start(&spec, &resource).await?, vec![]).await }.await;
    f.launcher.remove_stopped(&spec, &resource).await?;
    let (exit, out, err) = result?;
    if exit.exit_code != 0
        || exit.oom_killed
        || out != b"owned-node-thread"
        || err != b"owned-node-stderr"
    {
        return Err("Node thread/procfs/separate capture failed through concrete adapter".into());
    }
    Ok(())
}

async fn python_setup_removed_builder_and_readonly_reuse(f: &Fixture) -> TestResult {
    use attune_workload_backend::planning::{runner_program, SetupPlan, SetupStep};
    use std::io::Write;
    let mut pack = tempfile::tempfile()?;
    {
        let mut zip = zip::ZipWriter::new(&mut pack);
        zip.start_file(
            "source.txt",
            zip::write::SimpleFileOptions::default().unix_permissions(0o644),
        )?;
        zip.write_all(b"immutable source")?;
        zip.finish()?;
    }
    let pack = f.input(LaunchInputKind::Pack, &mut pack)?;
    let mut setup = f.spec(vec![runner_program(f.platform), "setup".into()]);
    setup.inputs = vec![pack.clone(), f.runner.clone()];
    setup.purpose = LaunchPurpose::Setup;
    setup.target = LaunchTarget::Setup {
        admission: 1,
        attempt: Uuid::new_v4(),
    };
    setup.stage = Some(PrivateStage {
        cache_key: "owned-docker-setup".into(),
        quota_bytes: 64 * 1024 * 1024,
        quota_files: 4096,
    });
    setup.invocation.working_root = LaunchRoot::Stage;
    let program = r#"import os,subprocess,sys,zipfile
root='/opt/attune/runtime'
assert os.getcwd()==root+'/project'
assert open('/opt/attune/pack/source.txt').read()=='immutable source'
open('source.txt','w').write('private project')
open('generated.lock','w').write('retained lock')
assert open('/opt/attune/pack/source.txt').read()=='immutable source'
subprocess.run([sys.executable,'-m','venv','--copies',root],check=True)
files={
 'ownedtool.py':'def main():\n print("owned-docker-console")\n',
 'ownedtool-1.0.dist-info/METADATA':'Metadata-Version: 2.1\nName: ownedtool\nVersion: 1.0\n',
 'ownedtool-1.0.dist-info/WHEEL':'Wheel-Version: 1.0\nGenerator: owned-fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
 'ownedtool-1.0.dist-info/entry_points.txt':'[console_scripts]\nowned-console = ownedtool:main\n',
}
files['ownedtool-1.0.dist-info/RECORD']=''.join(k+',,\n' for k in files)+'ownedtool-1.0.dist-info/RECORD,,\n'
wheel='/tmp/ownedtool-1.0-py3-none-any.whl'
with zipfile.ZipFile(wheel,'w') as z:
 for name,data in files.items(): z.writestr(name,data)
subprocess.run([root+'/bin/python','-m','pip','install','--no-index','--no-deps','--disable-pip-version-check',wheel],check=True)
os.symlink(root+'/bin/python',root+'/bin/internal-python')
assert open(root+'/bin/owned-console').readline().strip()=='#!'+root+'/bin/python'
"#;
    let plan = SetupPlan {
        version: 1,
        environment: BTreeMap::from([("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())]),
        steps: vec![SetupStep {
            argv: vec!["/usr/bin/python3".into(), "-c".into(), program.into()],
            if_pack_file: None,
        }],
    };
    let builder = f
        .launcher
        .prepare(&setup, ProcessCreateDisposition::CreateOnce)
        .await?;
    let result = async {
        let (exit, _, _) = run(
            f.launcher.start(&setup, &builder).await?,
            serde_json::to_vec(&plan)?,
        )
        .await?;
        if exit.exit_code != 0 || exit.oom_killed {
            return Err("owned Docker venv/pip setup failed".into());
        }
        // This effect positively Waits+Removes before validation. A second
        // exact body is replayed from retained staging, never a new build.
        let mut archive = f.launcher.archive_stopped_stage(&setup, &builder).await?;
        let runtime = f.input(LaunchInputKind::Runtime, &mut archive)?;
        let mut replay = f.launcher.archive_stopped_stage(&setup, &builder).await?;
        let repeated = f.input(LaunchInputKind::Runtime, &mut replay)?;
        if runtime.sha256 != repeated.sha256 || runtime.size_bytes != repeated.size_bytes {
            return Err("Docker setup archive changed on exact replay".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(runtime)
    }
    .await;
    // Fixture cache acceptance is not a domain runtime publication receipt.
    // Remove is already acknowledged; final cleanup reclaims private bytes.
    f.launcher.remove_stopped(&setup, &builder).await?;
    let runtime = result?;
    let mut reuse = f.spec(vec![
        "/bin/bash".into(),
        "-ec".into(),
        r#"test "$PWD" = /opt/attune/runtime/project
test "$(cat source.txt)" = 'private project'
test "$(cat generated.lock)" = 'retained lock'
test "$(cat /opt/attune/pack/source.txt)" = 'immutable source'
/opt/attune/runtime/bin/owned-console
/opt/attune/runtime/bin/internal-python -c 'import os; assert os.statvfs("/opt/attune/runtime").f_flag & os.ST_RDONLY; print("owned-docker-internal-link")'
"#
        .into(),
    ]);
    reuse.inputs = vec![pack.clone(), runtime.clone(), f.runner.clone()];
    reuse.invocation.working_root = LaunchRoot::Runtime;
    let resource = f
        .launcher
        .prepare(&reuse, ProcessCreateDisposition::CreateOnce)
        .await?;
    let result = async { run(f.launcher.start(&reuse, &resource).await?, vec![]).await }.await;
    f.launcher.remove_stopped(&reuse, &resource).await?;
    let (exit, out, err) = result?;
    if exit.exit_code != 0
        || exit.oom_killed
        || out != b"owned-docker-console\nowned-docker-internal-link\n"
        || !err.is_empty()
    {
        return Err(
            "Docker runtime reuse lost console script/link/project or read-only mount".into(),
        );
    }
    sibling_setup_stage_isolation(f, &setup, &pack, &runtime).await?;
    Ok(())
}

async fn sibling_setup_stage_isolation(
    f: &Fixture,
    original: &ProcessSpec,
    pack: &PinnedLaunchInput,
    runtime: &PinnedLaunchInput,
) -> TestResult {
    use attune_workload_backend::planning::{SetupPlan, SetupStep};
    let mut archive = f.cache.open_input_archive(runtime)?;
    let published = f.cache.install_input(runtime, &mut archive)?;
    let mut archive = f.cache.open_input_archive(pack)?;
    let source = f.cache.install_input(pack, &mut archive)?;
    let fresh = || {
        let mut spec = original.clone();
        spec.intent = Uuid::new_v4();
        spec.target = LaunchTarget::Setup {
            admission: 1,
            attempt: Uuid::new_v4(),
        };
        spec
    };
    let a = fresh();
    let marker = PathBuf::from("/var/lib/attune/launch/private")
        .join(a.intent.simple().to_string())
        .join("runtime/owned-setup-marker");
    let published_path = published.join("project/source.txt");
    let plan = |program: &str, arguments: Vec<String>, credential: bool| {
        let mut environment =
            BTreeMap::from([("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())]);
        if credential {
            environment.insert(
                "OWNED_DUMMY_SETUP_CREDENTIAL".into(),
                "owned-setup-a-secret".into(),
            );
        }
        let mut argv = vec!["/usr/bin/python3".into(), "-c".into(), program.into()];
        argv.extend(arguments);
        SetupPlan {
            version: 1,
            environment,
            steps: vec![SetupStep {
                argv,
                if_pack_file: None,
            }],
        }
    };
    let a_plan = plan(
        r#"import os,sys,time
from pathlib import Path
assert not Path(sys.argv[1]).exists()
assert not Path('/var/lib/attune/launch').exists()
Path('/opt/attune/runtime/owned-setup-marker').write_text(os.environ['OWNED_DUMMY_SETUP_CREDENTIAL'])
while True: time.sleep(1)
"#,
        vec![published_path
            .to_str()
            .ok_or("invalid owned cache path")?
            .into()],
        true,
    );
    let payload = serde_json::to_vec(&a_plan)?;
    let resource = f
        .launcher
        .prepare(&a, ProcessCreateDisposition::CreateOnce)
        .await?;
    let io = match f.launcher.start(&a, &resource).await {
        Ok(io) => io,
        Err(error) => {
            // A failed start is not publication evidence; the enclosing owned
            // harness also reconciles any uncertain live container on failure.
            f.launcher.remove_stopped(&a, &resource).await?;
            return Err(error.into());
        }
    };
    let control = io.control.clone();
    let running = tokio::spawn(run(io, payload));
    let result: TestResult = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            match std::fs::read(&marker) {
                Ok(bytes) if bytes == b"owned-setup-a-secret" => break,
                Ok(_) => return Err("owned setup marker changed".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if running.is_finished() || tokio::time::Instant::now() >= deadline {
                return Err("sibling setup did not become live".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let b = fresh();
        // Same logical runtime/cache key, distinct attempt-owned staging.
        let b_plan = plan(
            r#"import errno,os,sys
from pathlib import Path
assert Path.cwd() == Path('/opt/attune/runtime/project')
assert not Path(sys.argv[1]).exists()
assert not Path(sys.argv[2]).exists()
assert not Path('/var/lib/attune/launch').exists()
assert 'OWNED_DUMMY_SETUP_CREDENTIAL' not in os.environ
assert os.statvfs('/opt/attune/pack').f_flag & os.ST_RDONLY
assert Path('/opt/attune/pack/source.txt').read_text() == 'immutable source'
try: Path('/opt/attune/pack/source.txt').write_text('poisoned source')
except OSError as error: assert error.errno in (errno.EROFS, errno.EACCES)
else: raise AssertionError('admitted Pack was writable')
Path('/opt/attune/runtime/owned-setup-marker').write_text('owned-b-private')
print('owned-sibling-stage-boundary', end='')
"#,
            vec![
                marker.to_str().ok_or("invalid owned stage path")?.into(),
                published_path
                    .to_str()
                    .ok_or("invalid owned cache path")?
                    .into(),
            ],
            false,
        );
        let b_resource = f
            .launcher
            .prepare(&b, ProcessCreateDisposition::CreateOnce)
            .await?;
        let b_result: TestResult = async {
            let (exit, out, err) = run(
                f.launcher.start(&b, &b_resource).await?,
                serde_json::to_vec(&b_plan)?,
            )
            .await?;
            if exit.exit_code != 0
                || exit.oom_killed
                || out != b"owned-sibling-stage-boundary"
                || !err.is_empty()
            {
                return Err("sibling setup boundary failed".into());
            }
            let archive = f.launcher.archive_stopped_stage(&b, &b_resource).await?;
            let mut zip = zip::ZipArchive::new(archive)?;
            let mut own = Vec::new();
            zip.by_name("owned-setup-marker")?.read_to_end(&mut own)?;
            if own != b"owned-b-private" {
                return Err("setup archive included sibling bytes".into());
            }
            Ok(())
        }
        .await;
        // Only local fixture byte acceptance; no domain Ready receipt modeled.
        f.launcher.remove_stopped(&b, &b_resource).await?;
        b_result?;
        if running.is_finished()
            || std::fs::read(&marker)? != b"owned-setup-a-secret"
            || std::fs::read(&published_path)? != b"private project"
            || std::fs::read(source.join("source.txt"))? != b"immutable source"
        {
            return Err("setup/cleanup changed a live sibling or published input".into());
        }
        Ok(())
    }
    .await;
    // Stop and join the live capture/process before reclaiming its own stage,
    // including after any failed sibling assertion or provider effect.
    let stopped = control.stop(LaunchStopReason::Cancelled).await;
    let joined = running.await;
    let removed = f.launcher.remove_stopped(&a, &resource).await;
    stopped?;
    let (exit, _, _) = joined??;
    removed?;
    result?;
    if exit.exit_code == 0 || exit.oom_killed || marker.exists() {
        return Err("live setup cancellation/owned stage cleanup was not acknowledged".into());
    }
    Ok(())
}
