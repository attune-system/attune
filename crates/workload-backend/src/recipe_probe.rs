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
