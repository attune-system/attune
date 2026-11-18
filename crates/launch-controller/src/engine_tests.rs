use super::*;
use crate::test_support::*;
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
        let env = engine
            .image_environment(&spec.manifest.image.reference)
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
