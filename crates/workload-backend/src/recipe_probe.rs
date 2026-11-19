//! Fixed guest runtime probes and private recipe execution. Admission,
//! provenance and outcome classification remain trusted manager policy.
use attune_common::{
    child_process_environment::ChildProcessEnvironment, runtime_lifecycle::RuntimeRecipe,
};
use attune_workload_backend::{
    planning::{RUNTIME_RECIPE_CHUNK_BYTES, RUNTIME_TOOL_RESULT_BYTES},
    runner_protocol::MAX_START_CONTEXT,
};
use std::{
    ffi::OsString,
    io::{self, Write},
};

pub(super) fn inspect(arguments: &[OsString]) -> io::Result<()> {
    super::boundary_guard(true)?;
    if std::env::var_os("ATTUNE_API_TOKEN").is_some() {
        return Err(super::invalid());
    }
    let (recipe, path) = decode(arguments)?;
    let environment = probe_environment(&recipe, &path)?;
    attune_common::parent_process_protection::protect_service_parent()
        .map_err(|_| super::invalid())?;
    let tools = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(recipe.discover(&environment))
        .map_err(|_| super::invalid())?;
    for tool in tools.values() {
        // Keep the executable alias (e.g. a venv interpreter) and separately
        // prove the canonical identity. Writable scratch never supplies a tool
        // whose version is accepted as immutable image/input provenance.
        super::readonly_path(&tool.executable, true)?;
        super::readonly_path(&tool.identity, true)?;
    }
    let result = serde_json::to_vec(&tools).map_err(|_| super::invalid())?;
    if result.len() > RUNTIME_TOOL_RESULT_BYTES {
        return Err(super::invalid());
    }
    std::io::stdout().lock().write_all(&result)
}

/// Legacy metadata commands retain their priority, expected exit and patterns.
/// Success is an empty captured map: no language version or tool identity is
/// invented for metadata-only availability declarations.
pub(super) fn inspect_legacy(arguments: &[OsString]) -> io::Result<()> {
    use attune_common::runtime_detection::NativeRuntimeVerification;
    super::boundary_guard(true)?;
    attune_common::parent_process_protection::protect_service_parent()
        .map_err(|_| super::invalid())?;
    if std::env::var_os("ATTUNE_API_TOKEN").is_some()
        || arguments.len() < 2
        || arguments.len() > 4096
    {
        return Err(super::invalid());
    }
    let path = arguments[0].to_str().ok_or_else(super::invalid)?;
    let mut json = String::new();
    for part in &arguments[1..] {
        let part = part.to_str().ok_or_else(super::invalid)?;
        if part.len() > RUNTIME_RECIPE_CHUNK_BYTES || part.is_empty() {
            return Err(super::invalid());
        }
        json.push_str(part);
        if json.len() > MAX_START_CONTEXT {
            return Err(super::invalid());
        }
    }
    let definition: NativeRuntimeVerification =
        serde_json::from_str(&json).map_err(|_| super::invalid())?;
    if definition.execution_config.get("lifecycle").is_some() {
        return Err(super::invalid());
    }
    let environment = ChildProcessEnvironment::from_parent(
        [
            ("PATH".into(), path.into()),
            ("HOME".into(), "/tmp".into()),
            ("TMPDIR".into(), "/tmp".into()),
            ("LANG".into(), "C.UTF-8".into()),
        ],
        &[],
    )
    .map_err(|_| super::invalid())?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async { verify_legacy(&definition, &environment).await })?;
    std::io::stdout().lock().write_all(b"{}")
}
async fn verify_legacy(
    definition: &attune_common::runtime_detection::NativeRuntimeVerification,
    environment: &ChildProcessEnvironment,
) -> io::Result<()> {
    let verification = definition.distributions.get("verification");
    if !definition.registered_version
        && verification.is_some_and(|v| {
            v.get("always_available").and_then(|v| v.as_bool()) == Some(true)
                || v.get("check_required").and_then(|v| v.as_bool()) == Some(false)
        })
    {
        return Ok(());
    }
    let mut commands = verification
        .and_then(|v| v.get("commands"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    commands.sort_by_key(|c| {
        c.get("priority")
            .and_then(|v| v.as_i64())
            .unwrap_or(if definition.registered_version {
                100
            } else {
                999
            })
    });
    if commands.is_empty() {
        if !definition.registered_version {
            return Err(super::invalid());
        }
        let config: attune_common::models::runtime::RuntimeExecutionConfig =
            serde_json::from_value(definition.execution_config.clone())
                .map_err(|_| super::invalid())?;
        if config.interpreter.binary.is_empty() {
            return Ok(());
        }
        commands.push(serde_json::json!({"binary":config.interpreter.binary,"args":["--version"],"exit_code":0}));
    }
    for command in commands {
        let Some(binary) = command.get("binary").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(executable) =
            attune_common::runtime_lifecycle::resolve_executable(binary, environment)
        else {
            continue;
        };
        if super::readonly_path(&executable, true).is_err()
            || super::readonly_path(&executable.canonicalize()?, true).is_err()
        {
            return Err(super::invalid());
        }
        let args = command
            .get("args")
            .and_then(|v| v.as_array())
            .map(|args| args.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        let expected = command
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let Ok(expected) = i32::try_from(expected) else {
            continue;
        };
        let mut process = tokio::process::Command::new(executable);
        environment.apply(process.as_std_mut());
        process.args(args).current_dir("/tmp");
        let Ok(output) = attune_common::runtime_lifecycle::collect_runtime_verification(
            process,
            std::time::Duration::from_secs(10),
            expected,
            environment,
        )
        .await
        else {
            continue;
        };
        if let Some(pattern) = command.get("pattern").and_then(|v| v.as_str()) {
            if !legacy_pattern_matches(pattern, &output) {
                continue;
            }
        }
        return Ok(());
    }
    Err(super::invalid())
}
fn legacy_pattern_matches(pattern: &str, output: &str) -> bool {
    // Keep regex compilation in common's existing verifier module/dependency.
    attune_common::runtime_detection::verification_pattern_matches(pattern, output)
}

/// Discovery/preparation run before the helper touches action stdin. Its parent
/// protection prevents pack subprocesses from inspecting the held invocation
/// environment or stdin FD. Each subprocess gets null stdin and public env only.
/// The provider owns the whole deadline, cancellation, capture and stopped-writer
/// cleanup; no generation is archived, published, reused or shared with siblings.
pub(super) fn run(arguments: &[OsString]) -> io::Result<()> {
    run_with_pack_access(arguments, false)
}
pub(super) fn run_candidate(arguments: &[OsString]) -> io::Result<()> {
    run_with_pack_access(arguments, true)
}
fn run_with_pack_access(arguments: &[OsString], candidate_copy: bool) -> io::Result<()> {
    use attune_common::{runtime_lifecycle::RuntimeSource, workload_launch::PACK_ROOT};
    use std::{path::Path, process::Stdio};
    if candidate_copy {
        super::boundary_guard_basic()?;
        for (path, readonly) in [
            (PACK_ROOT, false),
            (attune_common::workload_launch::RUNNER_ROOT, true),
        ] {
            if !Path::new(path).is_dir() {
                return Err(super::invalid());
            }
            super::readonly_path(Path::new(path), readonly)?;
        }
        // Recipes never consume a shared/prepared Runtime mount. All writable
        // generations belong to this same candidate attempt's private scratch.
        if Path::new(attune_common::workload_launch::RUNTIME_ROOT).exists()
            || std::env::current_dir()?.canonicalize()? != Path::new(PACK_ROOT).canonicalize()?
        {
            return Err(super::invalid());
        }
    } else {
        super::boundary_guard(true)?;
    }
    attune_common::parent_process_protection::protect_service_parent()
        .map_err(|_| super::invalid())?;
    let (recipe, path, source, inline, expected) = decode_run(arguments)?;
    let environment = probe_environment(&recipe, &path)?;
    let mut invocation = std::env::vars().collect();
    let timeout_challenge = take_timeout_challenge(&mut invocation)?;
    let invocation = attune_workload_backend::planning::runtime_recipe_environment(&invocation)
        .map_err(|_| super::invalid())?;
    let file = Path::new(PACK_ROOT).join(&source);
    let source = if inline {
        RuntimeSource::Inline(&source)
    } else {
        RuntimeSource::File(&file)
    };
    // /tmp belongs solely to this native attempt. Keep absolute generation paths
    // stable through final execution, including venv shebangs and build outputs.
    let generation_root = Path::new("/tmp/attune-lifecycle-generations");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let target = recipe
            .prepare_with_tool_validation(
                Path::new(PACK_ROOT),
                source,
                generation_root,
                &environment,
                |tools| {
                    for tool in tools.values() {
                        super::readonly_path(&tool.executable, true)
                            .and_then(|_| super::readonly_path(&tool.identity, true))
                            .map_err(|_| {
                                attune_common::runtime_lifecycle::Error::Invalid(
                                    "Mutable runtime tool".into(),
                                )
                            })?;
                    }
                    if let Some(expected) = &expected {
                        let primary = recipe
                            .tools
                            .iter()
                            .find(|tool| tool.primary)
                            .and_then(|tool| tools.get(&tool.name))
                            .ok_or_else(|| {
                                attune_common::runtime_lifecycle::Error::Invalid(
                                    "Missing primary tool".into(),
                                )
                            })?;
                        if primary.version != *expected
                            && !attune_common::version_matching::matches_constraint(
                                &primary.version,
                                &format!("={expected}"),
                            )
                            .unwrap_or(false)
                        {
                            return Err(attune_common::runtime_lifecycle::Error::Invalid(
                                "Runtime version drift".into(),
                            ));
                        }
                    }
                    Ok(())
                },
            )
            .await
            .map_err(|_| super::invalid())?;
        if let Some(expected) = expected {
            target
                .validate_selected_version(&expected)
                .map_err(|_| super::invalid())?;
        }
        let mut final_environment = invocation.clone();
        final_environment.entry("PATH".into()).or_insert(path);
        final_environment.extend(target.env);
        for name in &recipe.permitted_caller_env {
            if let Some(value) = invocation.get(name) {
                final_environment.insert(name.clone(), value.clone());
            }
        }
        // Revalidate expanded recipe values and retain the already-admitted
        // invocation context. The recipe cannot set reserved Attune variables.
        let mut final_environment =
            attune_workload_backend::planning::runtime_recipe_environment(&final_environment)
                .map_err(|_| super::invalid())?;
        // Keep action input out of all tool probes/preparation, including
        // file delivery. Only the protected helper materializes it now, after
        // preparation has joined and immediately before the final child.
        let prepared_input = if super::parameter_input::required(&final_environment)? {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            Some(super::parameter_input::prepare(
                &mut final_environment,
                &bytes,
            )?)
        } else {
            None
        };
        let mut command = tokio::process::Command::new(&target.command[0]);
        command
            .env_clear()
            .envs(final_environment)
            .args(&target.command[1..])
            .current_dir(target.working_dir)
            .stdin(match &prepared_input {
                Some(prepared) => Stdio::from(prepared.stdin.try_clone()?),
                None => Stdio::inherit(),
            })
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn()?;
        let status = wait_recipe_child(
            child,
            std::time::Duration::from_secs(recipe.launch.timeout_seconds),
            timeout_challenge.as_deref(),
            Path::new("/tmp/artifacts"),
        )
        .await?;
        drop(prepared_input);
        // Output/custody and cleanup remain the owning worker's responsibility.
        std::process::exit(status.code().unwrap_or(1));
    })
}

/// Extract only the fixed private control. Other forbidden controls still fail
/// ordinary environment validation. The helper is protected before this runs;
/// neither prep's explicit environment nor final env_clear overlays retain it.
fn take_timeout_challenge(
    environment: &mut std::collections::BTreeMap<String, String>,
) -> io::Result<Option<String>> {
    use attune_workload_backend::planning::{
        RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES, RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV,
    };
    let challenge = environment.remove(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV);
    if challenge.as_ref().is_some_and(|value| {
        value.len() != RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(super::invalid());
    }
    Ok(challenge)
}
async fn wait_recipe_child(
    mut child: tokio::process::Child,
    timeout: std::time::Duration,
    challenge: Option<&str>,
    artifact_root: &std::path::Path,
) -> io::Result<std::process::ExitStatus> {
    let group = child.id();
    let result = tokio::time::timeout(timeout, child.wait()).await;
    if result.is_err() {
        // Reap the exact leader while group termination is confirmed; otherwise
        // its zombie alone can prevent positive group-disappearance evidence.
        let (stopped, joined) = tokio::join!(
            attune_common::runtime_lifecycle::stop_owned_process_group(group),
            child.wait()
        );
        stopped.map_err(|_| super::invalid())?;
        joined?;
        // Parent classification still waits for whole-resource writer removal.
        if let Some(challenge) = challenge {
            write_timeout_receipt(artifact_root, challenge)?;
        }
        return Err(super::invalid());
    }
    attune_common::runtime_lifecycle::stop_owned_process_group(group)
        .await
        .map_err(|_| super::invalid())?;
    // Every ordinary exit, including124, preserves its code and writes nothing.
    result.map_err(|_| super::invalid())?
}
fn write_timeout_receipt(artifact_root: &std::path::Path, challenge: &str) -> io::Result<()> {
    use attune_workload_backend::planning::{
        RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES, RUNTIME_RECIPE_TIMEOUT_RECEIPT,
    };
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};
    if challenge.len() != RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES
        || !challenge
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(super::invalid());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(artifact_root.join(RUNTIME_RECIPE_TIMEOUT_RECEIPT))?;
    file.write_all(challenge.as_bytes())?;
    file.sync_all()
}

fn decode_run(
    arguments: &[OsString],
) -> io::Result<(RuntimeRecipe, String, String, bool, Option<String>)> {
    let invalid = super::invalid;
    if arguments
        .iter()
        .try_fold(0usize, |size, part| size.checked_add(part.len()))
        .is_none_or(|size| size > MAX_START_CONTEXT)
    {
        return Err(invalid());
    }
    let inline = match arguments.first().and_then(|s| s.to_str()) {
        Some("inline") => true,
        Some("file") => false,
        _ => return Err(invalid()),
    };
    let count = arguments
        .get(1)
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|count| (1..=4096).contains(count))
        .ok_or_else(invalid)?;
    if arguments.len() < count + 5 {
        return Err(invalid());
    }
    let mut source = String::new();
    for part in &arguments[2..2 + count] {
        let part = part.to_str().ok_or_else(invalid)?;
        if part.is_empty() || part.len() > RUNTIME_RECIPE_CHUNK_BYTES {
            return Err(invalid());
        }
        source.push_str(part);
        if source.len() > MAX_START_CONTEXT {
            return Err(invalid());
        }
    }
    let source: String = serde_json::from_str(&source).map_err(|_| invalid())?;
    if source.contains('\0') {
        return Err(invalid());
    }
    if !inline {
        attune_common::artifact_transport::ValidatedRelativePath::new(&source)
            .map_err(|_| invalid())?;
    }
    let expected = arguments[2 + count].to_str().ok_or_else(invalid)?;
    if expected.contains('\0') {
        return Err(invalid());
    }
    let (recipe, path) = decode(&arguments[3 + count..])?;
    Ok((
        recipe,
        path,
        source,
        inline,
        if expected.is_empty() {
            None
        } else {
            Some(expected.into())
        },
    ))
}

fn decode(arguments: &[OsString]) -> io::Result<(RuntimeRecipe, String)> {
    if arguments.len() < 2 || arguments.len() > 4096 {
        return Err(super::invalid());
    }
    let path = arguments[0].to_str().ok_or_else(super::invalid)?;
    if path.contains('\0') || path.len() > MAX_START_CONTEXT {
        return Err(super::invalid());
    }
    let mut definition = String::new();
    for part in &arguments[1..] {
        let part = part.to_str().ok_or_else(super::invalid)?;
        if part.is_empty()
            || part.len() > RUNTIME_RECIPE_CHUNK_BYTES
            || definition
                .len()
                .checked_add(part.len())
                .is_none_or(|len| len > MAX_START_CONTEXT)
        {
            return Err(super::invalid());
        }
        definition.push_str(part);
    }
    let recipe: RuntimeRecipe = serde_json::from_str(&definition).map_err(|_| super::invalid())?;
    recipe.validate().map_err(|_| super::invalid())?;
    Ok((recipe, path.into()))
}

fn probe_environment(recipe: &RuntimeRecipe, path: &str) -> io::Result<ChildProcessEnvironment> {
    // Start from explicit values only. Neither ambient recipe-digest grants,
    // image configuration nor a manager's environment supply child authority.
    ChildProcessEnvironment::from_parent(
        [
            ("PATH".into(), path.into()),
            ("HOME".into(), "/tmp".into()),
            ("TMPDIR".into(), "/tmp".into()),
            ("LANG".into(), "C.UTF-8".into()),
            (
                "ATTUNE_RUNTIME_RECIPE_DIGESTS".into(),
                recipe.digest().map_err(|_| super::invalid())?.into(),
            ),
        ],
        &[],
    )
    .map_err(|_| super::invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::LaunchPlatform;
    use attune_workload_backend::planning::runtime_tool_probe_argv;

    fn recipe() -> RuntimeRecipe {
        serde_json::from_value(serde_json::json!({
            "tools":[{"name":"owned","binaries":["owned-tool"],
                "version_pattern":"(?<version>1)","primary":true}],
            "prepare":[{"command":["must-not-run-private-preparation"]}],
            "launch":{"command":["must-not-run-private-launch"]}
        }))
        .unwrap()
    }
    #[test]
    fn timeout_challenge_is_private_and_reserved_controls_are_not_broadly_removed() {
        use attune_workload_backend::planning::*;
        let mut environment = std::collections::BTreeMap::from([
            (RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV.into(), "a".repeat(64)),
            ("PUBLIC".into(), "owned".into()),
        ]);
        assert!(runtime_recipe_environment(&environment).is_err());
        let challenge = take_timeout_challenge(&mut environment).unwrap().unwrap();
        assert_eq!(challenge.len(), 64);
        assert!(!environment.contains_key(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV));
        assert!(runtime_recipe_environment(&environment).is_ok());
        let prep = probe_environment(&recipe(), "/usr/bin:/bin").unwrap();
        assert!(prep.get(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV).is_none());
        environment.insert("ATTUNE_LAUNCH_UNKNOWN".into(), "owned-control".into());
        assert!(runtime_recipe_environment(&environment).is_err());
        for value in ["short".into(), "A".repeat(64), "g".repeat(64)] {
            let mut environment = std::collections::BTreeMap::from([(
                RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV.into(),
                value,
            )]);
            assert!(take_timeout_challenge(&mut environment).is_err());
        }
    }
    #[test]
    fn timeout_receipt_rejects_precreation_and_aliases_without_overwriting() {
        use attune_workload_backend::planning::RUNTIME_RECIPE_TIMEOUT_RECEIPT;
        let root = tempfile::tempdir().unwrap();
        let challenge = "b".repeat(64);
        let leaf = root.path().join(RUNTIME_RECIPE_TIMEOUT_RECEIPT);
        std::fs::write(&leaf, "guest-precreated").unwrap();
        assert!(write_timeout_receipt(root.path(), &challenge).is_err());
        assert_eq!(std::fs::read(&leaf).unwrap(), b"guest-precreated");
        std::fs::remove_file(&leaf).unwrap();
        let other = root.path().join("owned-target");
        std::fs::write(&other, "untouched").unwrap();
        std::os::unix::fs::symlink(&other, &leaf).unwrap();
        assert!(write_timeout_receipt(root.path(), &challenge).is_err());
        assert_eq!(std::fs::read(&other).unwrap(), b"untouched");
    }
    #[tokio::test]
    async fn ordinary_exit_124_never_writes_a_timeout_receipt_or_inherits_challenge() {
        use attune_workload_backend::planning::*;
        let root = tempfile::tempdir().unwrap();
        let mut environment = std::collections::BTreeMap::from([(
            RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV.into(),
            "c".repeat(64),
        )]);
        let challenge = take_timeout_challenge(&mut environment).unwrap();
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .env_clear()
            .envs(environment)
            .args([
                "-c",
                r#"test -z "${ATTUNE_LAUNCH_RECIPE_TIMEOUT_CHALLENGE+x}" || exit 71; exit 124"#,
            ])
            .kill_on_drop(true)
            .process_group(0);
        let status = wait_recipe_child(
            command.spawn().unwrap(),
            std::time::Duration::from_secs(5),
            challenge.as_deref(),
            root.path(),
        )
        .await
        .unwrap();
        assert_eq!(status.code(), Some(124));
        assert!(!root.path().join(RUNTIME_RECIPE_TIMEOUT_RECEIPT).exists());
    }
    #[tokio::test]
    async fn own_child_timeout_writes_only_bounded_private_challenge_after_join() {
        use attune_workload_backend::planning::*;
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let challenge = "d".repeat(64);
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .env_clear()
            .args(["-c", "exec /bin/sleep 20"])
            .kill_on_drop(true)
            .process_group(0);
        assert!(wait_recipe_child(
            command.spawn().unwrap(),
            std::time::Duration::from_millis(50),
            Some(&challenge),
            root.path()
        )
        .await
        .is_err());
        let path = root.path().join(RUNTIME_RECIPE_TIMEOUT_RECEIPT);
        assert_eq!(std::fs::read(&path).unwrap(), challenge.as_bytes());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn public_verification_collector_still_denies_native_manager_local_launches() {
        let environment = ChildProcessEnvironment::from_parent([], &[])
            .unwrap()
            .with_workload_sandbox(
                attune_common::workload_sandbox::WorkloadSandbox::NativeEffectsRequired,
            );
        assert!(
            attune_common::runtime_lifecycle::collect_runtime_verification(
                tokio::process::Command::new("/owned-native-denied-must-never-run"),
                std::time::Duration::from_secs(1),
                0,
                &environment,
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn legacy_metadata_only_availability_preserves_base_and_version_modes() {
        let environment = ChildProcessEnvironment::from_parent([], &[]).unwrap();
        let mut definition = attune_common::runtime_detection::NativeRuntimeVerification {
            registered_version: false,
            execution_config: serde_json::json!({}),
            distributions: serde_json::json!({"verification":{"always_available":true}}),
        };
        assert!(verify_legacy(&definition, &environment).await.is_ok());
        definition.distributions = serde_json::json!({"verification":{"check_required":false}});
        assert!(verify_legacy(&definition, &environment).await.is_ok());
        definition.distributions = serde_json::json!({});
        assert!(verify_legacy(&definition, &environment).await.is_err());
        definition.registered_version = true;
        assert!(verify_legacy(&definition, &environment).await.is_ok());
        assert!(legacy_pattern_matches("^owned$", "owned"));
        assert!(!legacy_pattern_matches("[", "owned"));
    }

    #[test]
    fn legacy_definition_is_exact_bounded_public_context_and_never_a_recipe() {
        let definition = attune_common::runtime_detection::NativeRuntimeVerification {
            registered_version: true,
            execution_config: serde_json::json!({"interpreter":{"binary":"owned"}}),
            distributions: serde_json::json!({"verification":{"commands":[{"binary":"owned","exit_code":7,"pattern":"^owned$"}]}}),
        };
        let argv = attune_workload_backend::planning::runtime_verification_argv(
            LaunchPlatform::LinuxArm64,
            &definition,
            "/guest/bin",
        )
        .unwrap();
        assert_eq!(argv[1], "inspect-runtime-verification");
        let decoded: attune_common::runtime_detection::NativeRuntimeVerification =
            serde_json::from_str(&argv[3..].concat()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(&definition).unwrap()
        );
        let mut invalid = definition;
        invalid.execution_config = serde_json::json!({"lifecycle":{}});
        assert!(
            attune_workload_backend::planning::runtime_verification_argv(
                LaunchPlatform::LinuxAmd64,
                &invalid,
                "/guest/bin"
            )
            .is_err()
        );
    }

    #[test]
    fn definition_fragments_round_trip_with_only_explicit_guest_admission() {
        let mut recipe = recipe();
        recipe
            .env
            .insert("NOTE".into(), "owned-private-€".repeat(9000));
        let argv = runtime_tool_probe_argv(LaunchPlatform::LinuxArm64, &recipe, "/owned/guest/bin")
            .unwrap();
        assert_eq!(argv[0], "/opt/attune/runner/linux-arm64");
        assert!(argv.len() > 4);
        let arguments = argv[2..].iter().map(OsString::from).collect::<Vec<_>>();
        let (decoded, path) = decode(&arguments).unwrap();
        assert_eq!(decoded.digest().unwrap(), recipe.digest().unwrap());
        let environment = probe_environment(&decoded, &path).unwrap();
        assert_eq!(
            environment.get("PATH"),
            Some(std::ffi::OsStr::new("/owned/guest/bin"))
        );
        assert_eq!(environment.get("HOME"), Some(std::ffi::OsStr::new("/tmp")));
        assert!(environment.admits_runtime_recipe(&recipe.digest().unwrap()));
        assert!(environment.get("ATTUNE_RUNTIME_RECIPE_DIGESTS").is_none());
        assert!(environment.get("ATTUNE_API_TOKEN").is_none());
        assert!(environment.get("DATABASE_URL").is_none());
    }
    #[test]
    fn recipe_execution_plan_preserves_unicode_source_and_exact_version() {
        for inline in [true, false] {
            let source = if inline {
                "print('€')".repeat(9000)
            } else {
                "actions/owned.py".into()
            };
            let argv = attune_workload_backend::planning::runtime_recipe_argv(
                LaunchPlatform::LinuxAmd64,
                &recipe(),
                "/usr/bin:/bin",
                &source,
                inline,
                Some("3.12"),
            )
            .unwrap();
            let arguments = argv[2..].iter().map(OsString::from).collect::<Vec<_>>();
            let (decoded, path, actual_source, actual_inline, version) =
                decode_run(&arguments).unwrap();
            assert_eq!(decoded.digest().unwrap(), recipe().digest().unwrap());
            assert_eq!(path, "/usr/bin:/bin");
            assert_eq!(actual_source, source);
            assert_eq!(actual_inline, inline);
            assert_eq!(version.as_deref(), Some("3.12"));
            for end in 0..arguments.len() {
                assert!(decode_run(&arguments[..end]).is_err());
            }
        }
        for source in [
            "../manager",
            "/manager",
            "actions/../manager",
            "actions/owned\0.py",
        ] {
            assert!(attune_workload_backend::planning::runtime_recipe_argv(
                LaunchPlatform::LinuxAmd64,
                &recipe(),
                "/usr/bin:/bin",
                source,
                false,
                None,
            )
            .is_err());
        }
    }

    #[test]
    fn invalid_or_oversized_definitions_fail_without_echoing_private_values() {
        let definition = serde_json::to_string(&recipe()).unwrap();
        for arguments in [
            vec![],
            vec![OsString::from("/guest/bin")],
            vec![
                OsString::from("/guest/bin"),
                OsString::from("owned-private-invalid-json"),
            ],
            vec![
                OsString::from("/guest/bin\0private"),
                OsString::from(&definition),
            ],
            vec![
                OsString::from("/guest/bin"),
                OsString::from("x".repeat(RUNTIME_RECIPE_CHUNK_BYTES + 1)),
            ],
            vec![
                OsString::from("/guest/bin"),
                OsString::from(definition.replace("\"tools\"", "\"unknown_private\"")),
            ],
        ] {
            let error = decode(&arguments).unwrap_err();
            for value in ["owned-private", "/guest/bin", "unknown_private"] {
                assert!(!error.to_string().contains(value));
            }
        }
        let mut oversized = recipe();
        oversized
            .env
            .insert("NOTE".into(), "x".repeat(MAX_START_CONTEXT));
        assert!(
            runtime_tool_probe_argv(LaunchPlatform::LinuxAmd64, &oversized, "/guest/bin").is_err()
        );
        assert!(runtime_tool_probe_argv(
            LaunchPlatform::LinuxAmd64,
            &recipe(),
            "/guest/bin\0private"
        )
        .is_err());
    }
}
