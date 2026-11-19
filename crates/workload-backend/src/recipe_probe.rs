//! Fixed guest-only recipe discovery. Admission/provenance remain manager policy.
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

/// Discovery/preparation run before the helper touches action stdin. Its parent
/// protection prevents pack subprocesses from inspecting the held invocation
/// environment or stdin FD. Each subprocess gets null stdin and public env only.
/// The provider owns the whole deadline, cancellation, capture and stopped-writer
/// cleanup; no generation is archived, published, reused or shared with siblings.
pub(super) fn run(arguments: &[OsString]) -> io::Result<()> {
    use attune_common::{runtime_lifecycle::RuntimeSource, workload_launch::PACK_ROOT};
    use std::{path::Path, process::Stdio};
    super::boundary_guard(true)?;
    attune_common::parent_process_protection::protect_service_parent()
        .map_err(|_| super::invalid())?;
    let (recipe, path, source, inline, expected) = decode_run(arguments)?;
    let environment = probe_environment(&recipe, &path)?;
    let invocation =
        attune_workload_backend::planning::runtime_recipe_environment(&std::env::vars().collect())
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
        let final_environment =
            attune_workload_backend::planning::runtime_recipe_environment(&final_environment)
                .map_err(|_| super::invalid())?;
        let mut command = tokio::process::Command::new(&target.command[0]);
        command
            .env_clear()
            .envs(final_environment)
            .args(&target.command[1..])
            .current_dir(target.working_dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let group = child.id();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(recipe.launch.timeout_seconds),
            child.wait(),
        )
        .await;
        attune_common::runtime_lifecycle::stop_owned_process_group(group)
            .await
            .map_err(|_| super::invalid())?;
        if result.is_err() {
            child.wait().await?;
        }
        let status = result.map_err(|_| super::invalid())??;
        // Output/custody and cleanup remain the owning worker's responsibility.
        std::process::exit(status.code().unwrap_or(1));
    })
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
