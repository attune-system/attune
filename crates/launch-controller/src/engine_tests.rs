use super::*;
use crate::test_support::*;
use attune_common::workload_launch::*;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn selected_invocation_cannot_change_public_policy_or_expose_manager_credentials() {
    let image = format!("sha256:{}", "a".repeat(64));
    let mut spec = spec(image.clone());
    let policy = policy(image, "attune-owned-network".into());
    let original = policy
        .prepare(
            &spec,
            &["LEAK=owned-image-secret".into(), "LD_PRELOAD=bad".into()],
        )
        .unwrap();
    let json = serde_json::to_value(&original.body).unwrap();
    assert!(json["Env"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("LEAK=")));
    assert!(json["Env"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("LD_PRELOAD=")));
    assert_eq!(json["HostConfig"]["Privileged"], false);
    assert_eq!(json["HostConfig"]["CapDrop"], serde_json::json!(["ALL"]));
    assert_eq!(json["HostConfig"]["ReadonlyRootfs"], true);
    spec.invocation
        .env
        .insert("APP_SECRET".into(), "selected-secret".into());
    spec.invocation.argv.push("private-argument".into());
    let selected = policy.prepare(&spec, &[]).unwrap();
    assert_eq!(original.public_digest(), selected.public_digest());
    assert!(
        !serde_json::to_string(&serde_json::to_value(&selected.public.labels).unwrap())
            .unwrap()
            .contains("selected-secret")
    );
    for key in [
        "DOCKER_HOST",
        "KUBECONFIG",
        "JWT_SECRET",
        "ATTUNE_LAUNCH_LEASE",
        "ATTUNE_RUNTIME_LOG_CUSTODY_KEY",
    ] {
        spec.invocation.env.insert(key.into(), "denied".into());
        assert!(policy.prepare(&spec, &[]).is_err());
        spec.invocation.env.remove(key);
    }
    let mut revoked = policy.clone();
    revoked.allowed_images.clear();
    // Operator configuration must remain valid but the original image may be
    // revoked; reconstructing cleanup ownership still succeeds.
    revoked
        .allowed_images
        .insert(format!("sha256:{}", "b".repeat(64)));
    assert!(revoked.prepare(&spec, &[]).is_err());
    assert_eq!(
        revoked.public_plan(&spec.manifest).unwrap().digest(),
        original.public_digest()
    );
}

#[tokio::test]
async fn bounded_demultiplexer_separates_channels_and_rejects_bad_frames() {
    let (mut tx, rx) = tokio::io::duplex(128);
    tx.write_all(&[
        1, 0, 0, 0, 0, 0, 0, 3, b'o', b'u', b't', 2, 0, 0, 0, 0, 0, 0, 3, b'e', b'r', b'r',
    ])
    .await
    .unwrap();
    tx.shutdown().await.unwrap();
    let (out_tx, mut out_rx) = tokio::io::duplex(128);
    let (err_tx, mut err_rx) = tokio::io::duplex(128);
    demultiplex(rx, out_tx, err_tx).await.unwrap();
    let mut output = Vec::new();
    out_rx.read_to_end(&mut output).await.unwrap();
    assert_eq!(output, b"out");
    let mut error = Vec::new();
    err_rx.read_to_end(&mut error).await.unwrap();
    assert_eq!(error, b"err");
    assert!(demultiplex(
        &[1, 1, 0, 0, 0, 0, 0, 0][..],
        tokio::io::sink(),
        tokio::io::sink()
    )
    .await
    .is_err());
    assert!(demultiplex(
        &[1, 0, 0, 0, 255, 255, 255, 255][..],
        tokio::io::sink(),
        tokio::io::sink()
    )
    .await
    .is_err());
}

#[tokio::test]
async fn daemon_replacement_and_wrong_platform_image_are_rejected_before_create() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("owned-engine.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let image = format!("sha256:{}", "a".repeat(64));
    let image_id = image.clone();
    let server = tokio::spawn(async move {
        for index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() < 16384);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("GET "));
            assert!(!request.contains("/containers/"));
            let response = if index < 2 {
                assert!(request.starts_with("GET /v1.47/info "));
                serde_json::json!({"ID":if index==0 {"replacement-daemon"} else {"admitted-daemon"},"OSType":"linux","Architecture":"x86_64","SecurityOptions":["name=seccomp,profile=builtin"]})
            } else {
                assert!(request.contains("/images/"));
                serde_json::json!({"Id":image_id,"Os":"linux","Architecture":"arm64","Config":{"Env":["LEAK=owned-image-secret"],"Volumes":null}})
            };
            let body = serde_json::to_vec(&response).unwrap();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let engine = DockerEngine::connect(&socket).unwrap();
    let policy = policy(image.clone(), "owned-network".into());
    let binding = LaunchEngineBinding {
        engine_id: 5,
        daemon_id: "admitted-daemon".into(),
        platform: LaunchPlatform::LinuxAmd64,
    };
    assert!(engine
        .verify_engine_binding(&binding, &policy)
        .await
        .is_err());
    let rejected = engine
        .image_environment_for_binding(
            &binding,
            &policy,
            &PinnedImage {
                reference: image,
                sha256: Sha256Digest::parse(&"a".repeat(64)).unwrap(),
            },
        )
        .await;
    server.await.unwrap();
    assert!(rejected.is_err());
    assert!(!format!("{:?}", rejected.err().unwrap()).contains("owned-image-secret"));
}

/// External Engine test, never silently skipped when explicitly selected. It
/// creates/removes only its UUID network and CP-nonce-derived own container.
#[tokio::test]
#[ignore = "requires explicitly owned Docker Engine socket and pinned Bash image"]
async fn real_engine_full_proc_and_readonly_policy() {
    let socket = std::env::var("ATTUNE_TEST_DOCKER_SOCKET").expect("explicit owned engine socket");
    let image = std::env::var("ATTUNE_TEST_LAUNCH_IMAGE").expect("explicit immutable Bash image");
    let network = format!("attune-launch-test-{}", uuid::Uuid::new_v4().simple());
    let docker = |args: &[&str]| {
        std::process::Command::new("docker")
            .arg("--host")
            .arg(format!("unix://{socket}"))
            .args(args)
            .output()
            .unwrap()
    };
    let engine = DockerEngine::connect(Path::new(&socket)).unwrap();
    engine.verify_daemon_policy().await.unwrap();
    let owner = format!("io.attune.test.owner={network}");
    assert!(docker(&["network", "create", "--label", &owner, &network])
        .status
        .success());
    let mut dummy = std::process::Command::new("/bin/sleep")
        .arg("60")
        .env_clear()
        .env("ATTUNE_OWNED_DUMMY_CREDENTIAL", "owned-secret-not-for-pack")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let dummy_pid = dummy.id();
    let mut spec = spec(image.clone());
    spec.invocation.argv = vec!["/bin/bash".into(), "-c".into(), "set -e; test -e /proc/self/fd; test ! -e /var/run/docker.sock; test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token; test $(cat <(printf process-substitution)) = process-substitution; ! touch /etc/pack-poison 2>/dev/null; printf 'owned-proc-ok'; printf 'owned-stderr' >&2".into()];
    spec.invocation.argv[2] = format!(
        "{}; test ! -e /proc/{dummy_pid}/environ; ! kill -TERM {dummy_pid} 2>/dev/null",
        spec.invocation.argv[2]
    );
    let policy = policy(image, network.clone());
    let result = async {
        let binding = engine.observed_binding(policy.engine_id).await?;
        engine.verify_engine_binding(&binding, &policy).await?;
        let mut wrong_daemon = binding.clone();
        wrong_daemon.daemon_id = uuid::Uuid::new_v4().to_string();
        assert!(engine
            .verify_engine_binding(&wrong_daemon, &policy)
            .await
            .is_err());
        let mut wrong_platform = binding.clone();
        wrong_platform.platform = match binding.platform {
            LaunchPlatform::LinuxAmd64 => LaunchPlatform::LinuxArm64,
            LaunchPlatform::LinuxArm64 => LaunchPlatform::LinuxAmd64,
        };
        assert!(engine
            .verify_engine_binding(&wrong_platform, &policy)
            .await
            .is_err());
        let env = engine
            .image_environment_for_binding(&binding, &policy, &spec.manifest.image)
            .await?;
        let prepared = policy.prepare(&spec, &env)?;
        let container = match engine.create_once(&prepared).await? {
            CreateOutcome::Created(container) => container,
            CreateOutcome::DefinitivelyRejected { status } => {
                return Err(EngineError::Rejected(status))
            }
            CreateOutcome::ReconcileRequired => engine
                .reconcile(&prepared.public)
                .await?
                .ok_or(EngineError::InvalidResponse)?,
        };
        // Always tear down the owned container even on assertions/probe error.
        let run = async {
            let attached = engine.attach(&container).await?;
            let (read, mut stdin) = tokio::io::split(attached);
            let (out_tx, mut out_rx) = tokio::io::duplex(4096);
            let (err_tx, mut err_rx) = tokio::io::duplex(4096);
            let task = tokio::spawn(demultiplex(read, out_tx, err_tx));
            engine.start_once(&container).await?;
            stdin.shutdown().await?;
            let exit = engine.wait(&container).await?;
            let mut output = Vec::new();
            out_rx.read_to_end(&mut output).await?;
            let mut error = Vec::new();
            err_rx.read_to_end(&mut error).await?;
            task.await.map_err(|_| EngineError::InvalidResponse)??;
            if exit.exit_code != 0
                || exit.oom_killed
                || output != b"owned-proc-ok"
                || error != b"owned-stderr"
            {
                return Err(EngineError::InvalidResponse);
            }
            // Actual engine inspection must match fixed RO/private policy.
            engine
                .reconcile(&prepared.public)
                .await?
                .ok_or(EngineError::OwnershipMismatch)?;
            Ok::<_, EngineError>(())
        }
        .await;
        let stopped = engine.stop_and_wait(&container, 1).await;
        let removed = engine.remove_stopped(&container).await;
        stopped?;
        removed?;
        run
    }
    .await;
    let dummy_survived = dummy.try_wait().unwrap().is_none();
    if dummy_survived {
        dummy.kill().unwrap();
    }
    dummy.wait().unwrap();
    let cleanup = docker(&["network", "rm", &network]);
    assert!(cleanup.status.success(), "owned network cleanup failed");
    assert!(
        dummy_survived,
        "pack signaled owned credential process outside its PID namespace"
    );
    result.unwrap();
}

/// Runs inside an owned controller container with the exact named volume also
/// mounted locally. This test fails if the driver omits any prerequisite.
#[tokio::test]
#[ignore = "requires owned colocated controller container/Engine/cache volume and pinned Python image"]
async fn real_engine_cache_binding_and_python_publication() {
    use crate::cache::{CacheLimits, CacheStore};
    let socket = std::env::var("ATTUNE_TEST_DOCKER_SOCKET").expect("owned Engine socket");
    let image = std::env::var("ATTUNE_TEST_PYTHON_IMAGE").expect("immutable Python image");
    let volume = std::env::var("ATTUNE_TEST_CACHE_VOLUME").expect("owned exact named volume");
    let root =
        std::env::var("ATTUNE_TEST_CACHE_ROOT").expect("owned local mount of exact named volume");
    let engine = DockerEngine::connect(Path::new(&socket)).unwrap();
    engine.verify_daemon_policy().await.unwrap();
    let network = format!("attune-archive-test-{}", uuid::Uuid::new_v4().simple());
    let network_response = engine.control.post(DockerEngine::url(&["networks","create"])).json(&serde_json::json!({"Name":network,"Driver":"bridge","Labels":{"io.attune.test.owner":network}})).send().await.unwrap();
    assert_eq!(network_response.status(), StatusCode::CREATED);
    let mut policy = policy(image.clone(), network.clone());
    policy.cache_volume = volume;
    let cache = CacheStore::open(
        root.into(),
        CacheLimits {
            compressed_bytes: 128 * 1024 * 1024,
            expanded_bytes: 256 * 1024 * 1024,
            files: 20000,
        },
    )
    .unwrap();
    let wrong = format!("attune-wrong-binding-{}", uuid::Uuid::new_v4().simple());
    use futures::FutureExt;
    let result =
        std::panic::AssertUnwindSafe(publication_fixture(&engine, &policy, &cache, image, &wrong))
            .catch_unwind()
            .await;
    let wrong_cleanup = engine
        .control
        .delete(DockerEngine::url(&["volumes", &wrong]))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(
            wrong_cleanup.status(),
            StatusCode::NO_CONTENT | StatusCode::NOT_FOUND
        ),
        "owned wrong-volume cleanup failed"
    );
    let removed = engine
        .control
        .delete(DockerEngine::url(&["networks", &network]))
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    match result {
        Ok(value) => value.unwrap(),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

async fn publication_fixture(
    engine: &DockerEngine,
    policy: &EnginePolicy,
    cache: &crate::cache::CacheStore,
    image: String,
    wrong: &str,
) -> Result<()> {
    use std::{
        collections::BTreeMap,
        io::{Read, Seek, SeekFrom, Write},
    };
    engine.verify_cache_binding(policy, cache, &image).await?;
    // An existing different named volume must not pass as the local cache.
    let response = engine
        .control
        .post(DockerEngine::url(&["volumes", "create"]))
        .json(&serde_json::json!({"Name":wrong,"Labels":{"io.attune.test.owner":wrong}}))
        .send()
        .await
        .map_err(|_| EngineError::Transport)?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let mut wrong_policy = policy.clone();
    wrong_policy.cache_volume = wrong.to_string();
    let rejected = engine
        .verify_cache_binding(&wrong_policy, cache, &image)
        .await
        .is_err();
    let removed = engine
        .control
        .delete(DockerEngine::url(&["volumes", wrong]))
        .send()
        .await
        .map_err(|_| EngineError::Transport)?;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    assert!(rejected, "incorrect cache binding passed");
    let mut pack = tempfile::tempfile().unwrap();
    let mut wheel = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut wheel);
        for (name,body) in [
                ("owned_tool.py","def main():\n    print('published-console-ok')\n"),
                ("owned_tool-1.0.dist-info/METADATA","Metadata-Version: 2.1\nName: owned-tool\nVersion: 1.0\n"),
                ("owned_tool-1.0.dist-info/WHEEL","Wheel-Version: 1.0\nGenerator: owned-offline-fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n"),
                ("owned_tool-1.0.dist-info/entry_points.txt","[console_scripts]\nowned-console=owned_tool:main\n"),
                ("owned_tool-1.0.dist-info/RECORD","owned_tool.py,,\nowned_tool-1.0.dist-info/METADATA,,\nowned_tool-1.0.dist-info/WHEEL,,\nowned_tool-1.0.dist-info/entry_points.txt,,\nowned_tool-1.0.dist-info/RECORD,,\n"),
            ] { zip.start_file(name,zip::write::SimpleFileOptions::default()).unwrap();zip.write_all(body.as_bytes()).unwrap(); }
        zip.finish().unwrap();
    }
    {
        let mut zip = zip::ZipWriter::new(&mut pack);
        zip.start_file(
            "owned_tool-1.0-py3-none-any.whl",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(wheel.get_ref()).unwrap();
        zip.finish().unwrap();
    }
    pack.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    pack.read_to_end(&mut bytes).unwrap();
    let pack_pin = PinnedLaunchInput {
        pin_id: 1,
        kind: LaunchInputKind::Pack,
        object_id: 2,
        object_version: "owned-pack".into(),
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::of_bytes(&bytes),
        max_expanded_bytes: 1024 * 1024,
        max_files: 32,
    };
    cache.install_input(&pack_pin, &mut pack).unwrap();
    let mut setup = spec(image.clone());
    setup.manifest.purpose = LaunchPurpose::Setup;
    setup.manifest.target = LaunchTarget::Setup {
        admission: 10,
        attempt: uuid::Uuid::new_v4(),
    };
    setup.manifest.stage = Some(PrivateStage {
        cache_key: "owned-image-bound-cache".into(),
        quota_bytes: 256 * 1024 * 1024,
        quota_files: 20000,
    });
    setup.manifest.inputs = vec![pack_pin];
    setup.invocation=LaunchInvocation { argv:vec!["/bin/sh".into(),"-ec".into(),"python3 -m venv --copies /opt/attune/runtime; /opt/attune/runtime/bin/python3 -m pip install --no-index --no-deps --no-compile /opt/attune/pack/owned_tool-1.0-py3-none-any.whl >/tmp/pip-install.log; ln -s /opt/attune/runtime/bin/owned-console /opt/attune/runtime/absolute-tool; ln -s bin/owned-console /opt/attune/runtime/relative-tool; test ! -e /var/run/docker.sock; test ! -d /opt/attune/private; ! touch /opt/attune/pack/poison 2>/dev/null".into()],env:BTreeMap::new(),working_root:LaunchRoot::Stage };
    cache
        .ensure_capacity(&setup.manifest, 128 * 1024 * 1024)
        .unwrap();
    cache.prepare_private(&setup.manifest).unwrap();
    let (exit, resource, _, _) = run_captured(engine, policy, &setup).await?;
    assert_eq!(exit.exit_code, 0);
    assert!(!exit.oom_killed);
    cache.record_removal(&setup.manifest, &resource).unwrap();
    let status = LaunchStatus {
        request: LaunchRequest {
            launch_id: setup.manifest.launch_id,
            admitted_digest: setup.manifest.admitted_digest().unwrap(),
        },
        state: LaunchState::Stopped,
        runtime_state: LaunchRuntimeState::Exited,
        exit_outcome: Some(LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        }),
        resource: Some(resource),
        stop_reason: None,
        publication: LaunchPublicationState::Pending,
        capture: LaunchCaptureState::Pending,
        capture_deadline: Some(chrono::Utc::now() + chrono::Duration::seconds(60)),
    };
    // This fixture status is not a CP authority test: it exercises the
    // post-Remove filesystem/ZIP contract using actual Engine evidence.
    let mut archive = cache
        .archive_settled_stage(&setup.manifest, &status)
        .unwrap();
    let mut bytes = Vec::new();
    archive.read_to_end(&mut bytes).unwrap();
    archive.seek(SeekFrom::Start(0)).unwrap();
    let runtime_pin = PinnedLaunchInput {
        pin_id: 3,
        kind: LaunchInputKind::Runtime,
        object_id: 4,
        object_version: "owned-runtime".into(),
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::of_bytes(&bytes),
        max_expanded_bytes: 256 * 1024 * 1024,
        max_files: 20000,
    };
    cache.install_input(&runtime_pin, &mut archive).unwrap();
    let mut execute = spec(image);
    execute.manifest.inputs = vec![runtime_pin];
    execute.invocation=LaunchInvocation { argv:vec!["/bin/sh".into(),"-ec".into(),"/opt/attune/runtime/bin/owned-console; /opt/attune/runtime/absolute-tool; /opt/attune/runtime/relative-tool; ! touch /opt/attune/runtime/poison 2>/dev/null".into()],env:BTreeMap::new(),working_root:LaunchRoot::Runtime };
    let (exit, _, out, err) = run_captured(engine, policy, &execute).await?;
    assert_eq!(exit.exit_code, 0);
    assert!(!exit.oom_killed);
    assert!(err.is_empty());
    assert_eq!(
        out,
        b"published-console-ok\npublished-console-ok\npublished-console-ok\n"
    );
    let mut settled = status;
    settled.publication = LaunchPublicationState::Published;
    cache
        .cleanup_settled_private(&setup.manifest, &settled)
        .unwrap();
    Ok::<_, EngineError>(())
}

async fn run_captured(
    engine: &DockerEngine,
    policy: &EnginePolicy,
    spec: &LaunchSpec,
) -> Result<(ExitStatus, EngineResourceEvidence, Vec<u8>, Vec<u8>)> {
    let image_env = engine
        .image_environment(&spec.manifest.image.reference)
        .await?;
    let prepared = policy.prepare(spec, &image_env)?;
    let container = match engine.create_once(&prepared).await? {
        CreateOutcome::Created(container) => container,
        CreateOutcome::DefinitivelyRejected { status } => {
            return Err(EngineError::Rejected(status))
        }
        CreateOutcome::ReconcileRequired => engine
            .reconcile(&prepared.public)
            .await?
            .ok_or(EngineError::Transport)?,
    };
    let stdout = crate::capture::Capture::new(65536)?;
    let stderr = crate::capture::Capture::new(65536)?;
    let mut reader = None;
    let run = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let attached = engine.attach(&container).await?;
        let (read, mut stdin) = tokio::io::split(attached);
        reader = Some(tokio::spawn(demultiplex(
            read,
            stdout.clone(),
            stderr.clone(),
        )));
        engine.start_once(&container).await?;
        stdin.shutdown().await?;
        engine.wait(&container).await
    })
    .await
    .map_err(|_| EngineError::Transport)
    .and_then(|result| result);
    let cleanup = async {
        engine.stop_and_wait(&container, 1).await?;
        engine.remove_stopped(&container).await
    }
    .await;
    if let Some(mut task) = reader {
        if cleanup.is_err() {
            task.abort();
            let _ = task.await;
        } else {
            match tokio::time::timeout(std::time::Duration::from_secs(10), &mut task).await {
                Ok(result) => result.map_err(|_| EngineError::InvalidResponse)??,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    return Err(EngineError::Transport);
                }
            }
        }
    }
    cleanup?;
    let exit = run?;
    let out = stdout.read(0, 65536)?;
    let err = stderr.read(0, 65536)?;
    if !out.eof || !err.eof || out.truncated_bytes != 0 || err.truncated_bytes != 0 {
        return Err(EngineError::InvalidResponse);
    }
    Ok((exit, container.evidence().clone(), out.bytes, err.bytes))
}
