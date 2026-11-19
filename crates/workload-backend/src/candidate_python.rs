//! Fixed guest-only legacy Python preparation. The trusted worker binds the
//! base definition and immutable provider before admitting this one effect.
use attune_common::{
    child_process_environment::ChildProcessEnvironment,
    workload_launch::{PACK_ROOT, RUNNER_ROOT, RUNTIME_ROOT},
};
use attune_workload_backend::planning::{
    candidate_python_setup, CandidatePythonPlan, CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES,
    CANDIDATE_PYTHON_PREP_CHALLENGE_ENV, CANDIDATE_PYTHON_PREP_RECEIPT, RUNTIME_RECIPE_CHUNK_BYTES,
    SETUP_STDIN_LIMIT,
};
use std::{ffi::OsString, io, path::Path, process::Stdio};

pub(super) fn run(arguments: &[OsString]) -> io::Result<i32> {
    use std::os::unix::process::CommandExt;
    boundary()?;
    attune_common::parent_process_protection::protect_service_parent()
        .map_err(|_| super::invalid())?;
    let challenge =
        std::env::var(CANDIDATE_PYTHON_PREP_CHALLENGE_ENV).map_err(|_| super::invalid())?;
    std::env::remove_var(CANDIDATE_PYTHON_PREP_CHALLENGE_ENV);
    if !valid_challenge(&challenge) {
        return Err(super::invalid());
    }
    let plan = decode(arguments)?;
    if std::env::var_os("ATTUNE_API_TOKEN").is_some() {
        return Err(super::invalid());
    }
    // Retain invocation context in the protected helper. Preparation gets only
    // the separate public plan and null stdin, never this environment/stdin.
    let mut invocation =
        attune_workload_backend::planning::runtime_recipe_environment(&std::env::vars().collect())
            .map_err(|_| super::invalid())?;
    let generation = tempfile::Builder::new()
        .prefix("attune-candidate-python-")
        .tempdir_in("/tmp")?
        .keep();
    let env_dir = generation.join("env");
    std::fs::create_dir(&env_dir)?;
    let (setup, interpreter) =
        candidate_python_setup(&plan.config, env_dir.to_str().ok_or_else(super::invalid)?)
            .map_err(|_| super::invalid())?;
    let public = ChildProcessEnvironment::from_parent(
        setup
            .environment
            .iter()
            .map(|(key, value)| (key.as_str().into(), value.as_str().into())),
        &[],
    )
    .map_err(|_| super::invalid())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(plan.timeout_ms);
    runtime.block_on(async {
        for step in setup.steps {
            if let Some(relative) = &step.if_pack_file {
                let path = Path::new(PACK_ROOT).join(relative);
                match std::fs::metadata(&path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Ok(metadata) if metadata.is_file() => {
                        if !path
                            .canonicalize()?
                            .starts_with(Path::new(PACK_ROOT).canonicalize()?)
                        {
                            return Err(super::invalid());
                        }
                    }
                    _ => return Err(super::invalid()),
                }
            }
            let mut command = tokio::process::Command::new(&step.argv[0]);
            public.apply(command.as_std_mut());
            command.args(&step.argv[1..]).current_dir(PACK_ROOT);
            prepare(
                command,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await?;
        }
        Ok::<_, io::Error>(())
    })?;
    drop(runtime);
    let path = Path::new(&interpreter);
    let metadata = path.metadata()?;
    use std::os::unix::fs::PermissionsExt;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(super::invalid());
    }
    let entry = Path::new(PACK_ROOT).join(&plan.entry);
    if !entry.metadata()?.is_file()
        || !entry
            .canonicalize()?
            .starts_with(Path::new(PACK_ROOT).canonicalize()?)
    {
        return Err(super::invalid());
    }
    let bin = path
        .parent()
        .and_then(Path::to_str)
        .ok_or_else(super::invalid)?;
    invocation.insert("PATH".into(), format!("{bin}:/usr/local/bin:/usr/bin:/bin"));
    let (program, arguments) = final_command(&plan, &interpreter);
    let mut command = std::process::Command::new(program);
    command.env_clear().envs(invocation).args(arguments);
    command
        .current_dir(PACK_ROOT)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    write_prepared(Path::new("/tmp/artifacts"), &challenge)?;
    // Exec keeps the physical generation and exact interpreter path unchanged.
    // The owning provider cleans this private scratch after terminal/removal
    // proof; no shared ready marker or reusable runtime is published.
    Err(command.exec())
}
fn final_command(plan: &CandidatePythonPlan, interpreter: &str) -> (String, Vec<String>) {
    let entry = format!("{PACK_ROOT}/{}", plan.entry);
    if plan.runner == "script" {
        let shell = if Path::new(&plan.entry).extension().and_then(|s| s.to_str()) == Some("sh") {
            "/bin/sh"
        } else {
            "/bin/bash"
        };
        return (shell.into(), vec![entry]);
    }
    let mut args = vec!["-m".into(), plan.runner.clone(), entry];
    if plan.runner == "pytest" {
        args.push("-v".into());
    }
    (interpreter.into(), args)
}

fn write_prepared(root: &Path, challenge: &str) -> io::Result<()> {
    use std::{
        io::Write,
        os::unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            io::{AsRawFd, FromRawFd},
        },
    };
    if !valid_challenge(challenge) {
        return Err(super::invalid());
    }
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::fs::create_dir(root)?,
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root)?;
    let metadata = directory.metadata()?;
    // Kubernetes emptyDir may be root-owned with the fixed guest fsGroup.
    // The closed provider proves this private mount; do not invent another
    // owner/readiness attestation or reject that supported volume shape.
    if !metadata.is_dir() {
        return Err(super::invalid());
    }
    let name =
        std::ffi::CString::new(CANDIDATE_PYTHON_PREP_RECEIPT).map_err(|_| super::invalid())?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(challenge.as_bytes())?;
    file.sync_all()?;
    directory.sync_all()?;
    Ok(())
}
fn valid_challenge(value: &str) -> bool {
    value.len() == CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn boundary() -> io::Result<()> {
    super::boundary_guard_basic()?;
    for (root, readonly) in [(PACK_ROOT, false), (RUNNER_ROOT, true)] {
        if !Path::new(root).is_dir() {
            return Err(super::invalid());
        }
        super::readonly_path(Path::new(root), readonly)?;
    }
    if Path::new(RUNTIME_ROOT).exists()
        || std::env::current_dir()?.canonicalize()? != Path::new(PACK_ROOT).canonicalize()?
    {
        return Err(super::invalid());
    }
    Ok(())
}

fn decode(arguments: &[OsString]) -> io::Result<CandidatePythonPlan> {
    if arguments.is_empty() || arguments.len() > SETUP_STDIN_LIMIT / RUNTIME_RECIPE_CHUNK_BYTES + 1
    {
        return Err(super::invalid());
    }
    let mut json = String::new();
    for part in arguments {
        let part = part.to_str().ok_or_else(super::invalid)?;
        if part.is_empty() || part.len() > RUNTIME_RECIPE_CHUNK_BYTES {
            return Err(super::invalid());
        }
        json.push_str(part);
        if json.len() > SETUP_STDIN_LIMIT {
            return Err(super::invalid());
        }
    }
    let plan: CandidatePythonPlan = serde_json::from_str(&json).map_err(|_| super::invalid())?;
    plan.validate().map_err(|_| super::invalid())?;
    Ok(plan)
}

/// Zero-retention bounded-memory capture: drain both streams to real EOF and
/// join them, without feeding pip/setup text to the test-result parser.
async fn prepare(
    mut command: tokio::process::Command,
    remaining: std::time::Duration,
) -> io::Result<()> {
    if remaining.is_zero() {
        return Err(super::invalid());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn()?;
    let group = child.id();
    let mut stdout = child.stdout.take().ok_or_else(super::invalid)?;
    let mut stderr = child.stderr.take().ok_or_else(super::invalid)?;
    let stdout =
        tokio::spawn(async move { tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await });
    let stderr =
        tokio::spawn(async move { tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await });
    let outcome = tokio::time::timeout(remaining, child.wait()).await;
    let stopped = attune_common::runtime_lifecycle::stop_owned_process_group(group).await;
    if !matches!(&outcome, Ok(Ok(_))) {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    let (stdout, stderr) = tokio::join!(stdout, stderr);
    stopped.map_err(|_| super::invalid())?;
    stdout.map_err(|_| super::invalid())??;
    stderr.map_err(|_| super::invalid())??;
    let status = outcome.map_err(|_| super::invalid())??;
    if !status.success() {
        return Err(super::invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> CandidatePythonPlan {
        let metadata: serde_json::Value =
            serde_yaml_ng::from_str(include_str!("../../../packs/core/runtimes/python.yaml"))
                .unwrap();
        CandidatePythonPlan {
            version: 1,
            config: serde_json::from_value(metadata["execution_config"].clone()).unwrap(),
            runner: "pytest".into(),
            entry: "tests/check.py".into(),
            timeout_ms: 300_000,
        }
    }
    #[test]
    fn private_plan_round_trip_needs_no_stdin_or_host_python() {
        let argv = attune_workload_backend::planning::candidate_python_argv(
            attune_common::workload_launch::LaunchPlatform::LinuxAmd64,
            &plan(),
        )
        .unwrap();
        let decoded = decode(&argv[2..].iter().map(OsString::from).collect::<Vec<_>>()).unwrap();
        assert_eq!(decoded.runner, "pytest");
        assert_eq!(decoded.entry, "tests/check.py");
        assert!(decoded.config.lifecycle.is_none());
    }
    #[test]
    fn final_commands_preserve_shell_selection_and_python_module_arguments() {
        let mut selected = plan();
        let interpreter = "/tmp/owned-private/env/bin/python3";
        assert_eq!(
            final_command(&selected, interpreter),
            (
                interpreter.into(),
                vec![
                    "-m".into(),
                    "pytest".into(),
                    format!("{PACK_ROOT}/tests/check.py"),
                    "-v".into()
                ]
            )
        );
        selected.runner = "unittest".into();
        assert_eq!(
            final_command(&selected, interpreter).1,
            ["-m", "unittest", &format!("{PACK_ROOT}/tests/check.py")]
        );
        selected.runner = "script".into();
        for (entry, shell) in [
            ("tests/check.sh", "/bin/sh"),
            ("tests/check.txt", "/bin/bash"),
            ("-leading.sh", "/bin/sh"),
        ] {
            selected.entry = entry.into();
            selected.validate().unwrap();
            assert_eq!(
                final_command(&selected, interpreter),
                (shell.into(), vec![format!("{PACK_ROOT}/{entry}")])
            );
        }
    }
    #[test]
    fn malformed_or_unbounded_private_plans_fail_without_execution() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[OsString::from("not-json")]).is_err());
        assert!(decode(&[OsString::from("a".repeat(RUNTIME_RECIPE_CHUNK_BYTES + 1))]).is_err());
        let mut invalid = plan();
        invalid.runner = "arbitrary".into();
        assert!(decode(&[serde_json::to_string(&invalid).unwrap().into()]).is_err());
    }
    #[test]
    fn prep_receipt_is_exact_private_and_never_replaces_an_existing_leaf() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let proof = "a".repeat(64);
        write_prepared(root.path(), &proof).unwrap();
        let path = root.path().join(CANDIDATE_PYTHON_PREP_RECEIPT);
        assert_eq!(std::fs::read(&path).unwrap(), proof.as_bytes());
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert!(write_prepared(root.path(), &"b".repeat(64)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), proof.as_bytes());
        root.close().unwrap();
    }
    #[test]
    fn prep_receipt_refuses_symlink_roots_leaves_and_invalid_nonce() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = root.path().join("artifacts");
        assert!(write_prepared(&artifacts, &"A".repeat(64)).is_err());
        assert!(!artifacts.exists());
        std::fs::create_dir(&artifacts).unwrap();
        std::os::unix::fs::symlink(&artifacts, root.path().join("alias")).unwrap();
        assert!(write_prepared(&root.path().join("alias"), &"a".repeat(64)).is_err());
        let other = root.path().join("other");
        std::fs::write(&other, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&other, artifacts.join(CANDIDATE_PYTHON_PREP_RECEIPT)).unwrap();
        assert!(write_prepared(&artifacts, &"a".repeat(64)).is_err());
        assert_eq!(std::fs::read(other).unwrap(), b"unchanged");
        root.close().unwrap();
    }
}
