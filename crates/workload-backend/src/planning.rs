//! Pure trusted-CP planning for admitted OCI launches. This module never reads
//! host runtime availability, creates directories or executes a command.
//! Plans are private selected stdin, bound under randomized encrypted context;
//! they must never be embedded in a public Runner archive or plaintext digest.
use attune_common::{
    models::runtime::{RuntimeEnvVarConfig, RuntimeEnvVarOperation, RuntimeExecutionConfig},
    workload_launch::{LaunchPlatform, PackAccess, PACK_ROOT, RUNNER_ROOT, RUNTIME_ROOT},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};
use thiserror::Error;

pub const SETUP_STDIN_LIMIT: usize = 1024 * 1024;
/// Setup-owned source/project output retained with the published runtime.
pub const PROJECT_ROOT: &str = "/opt/attune/runtime/project";
/// Runtime invocations use their frozen setup project. Candidate Pack working
/// roots remain their private writable copy; mount roots do not change.
pub fn working_directory(root: attune_common::workload_launch::LaunchRoot) -> &'static str {
    match root {
        attune_common::workload_launch::LaunchRoot::Runtime => PROJECT_ROOT,
        _ => root.container_path(),
    }
}
#[derive(Debug, Error)]
pub enum PlanningError {
    #[error("invalid pack-relative launch path")]
    Path,
    #[error("runtime configuration requires an admitted setup/publication or is unsupported")]
    Configuration,
    #[error("forbidden authority or control variable in workload environment")]
    Environment,
    #[error("private setup plan exceeds bounds")]
    Bounds,
}
pub type Result<T> = std::result::Result<T, PlanningError>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupPlan {
    pub version: u8,
    pub environment: BTreeMap<String, String>,
    pub steps: Vec<SetupStep>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupStep {
    pub argv: Vec<String>,
    /// Canonical relative path under the private setup project, not a host path.
    /// Missing manifests skip only their dependency step, never environment creation.
    pub if_pack_file: Option<String>,
}
impl fmt::Debug for SetupPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupPlan")
            .field("version", &self.version)
            .field("environment_count", &self.environment.len())
            .field("step_count", &self.steps.len())
            .finish()
    }
}
impl fmt::Debug for SetupStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupStep")
            .field("argument_count", &self.argv.len())
            .finish_non_exhaustive()
    }
}
impl SetupPlan {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.steps.len() > 64 {
            return Err(PlanningError::Bounds);
        }
        validate_environment(&self.environment, false)?;
        let mut bytes = 0usize;
        for step in &self.steps {
            if step.argv.is_empty() || step.argv[0].is_empty() || step.argv.len() > 1024 {
                return Err(PlanningError::Configuration);
            }
            if let Some(path) = &step.if_pack_file {
                pack_path(path)?;
            }
            for argument in &step.argv {
                if argument.contains('\0') {
                    return Err(PlanningError::Configuration);
                }
                bytes = bytes
                    .checked_add(argument.len())
                    .ok_or(PlanningError::Bounds)?;
            }
        }
        if bytes > 256 * 1024
            || serde_json::to_vec(self)
                .map_err(|_| PlanningError::Bounds)?
                .len()
                > SETUP_STDIN_LIMIT
        {
            return Err(PlanningError::Bounds);
        }
        Ok(())
    }
}
pub fn runner_program(platform: LaunchPlatform) -> String {
    format!("{RUNNER_ROOT}/linux-{}", platform.native_arch())
}
/// Private CP-selected envelope, never a public Runner body or caller plan.
/// Selection/provenance must already be bound to the admitted immutable image
/// or native input. This function never probes the CP host filesystem/PATH.
pub fn cli_exec_argv(
    platform: LaunchPlatform,
    selected: &attune_common::action_cli::SelectedProgram,
    arguments: &[String],
) -> Result<Vec<String>> {
    validate_cli_exec(selected, arguments)?;
    let mut argv = vec![
        runner_program(platform),
        "exec".into(),
        selected.sha256.clone(),
        selected.path.to_str().ok_or(PlanningError::Path)?.into(),
        selected.invocation_name.clone(),
    ];
    argv.extend_from_slice(arguments);
    Ok(argv)
}

pub fn validate_cli_exec(
    selected: &attune_common::action_cli::SelectedProgram,
    arguments: &[String],
) -> Result<()> {
    let path = selected.path.to_str().ok_or(PlanningError::Path)?;
    if !selected.path.is_absolute()
        || selected
            .path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        || path.contains('\0')
        || selected.invocation_name.is_empty()
        || selected.invocation_name.contains('\0')
    {
        return Err(PlanningError::Path);
    }
    attune_common::workload_launch::Sha256Digest::parse(&selected.sha256)
        .map_err(|_| PlanningError::Configuration)?;
    let limits = attune_common::action_cli::ArgumentLimits::default();
    let program_bytes = path.len().max(selected.invocation_name.len());
    if arguments.len() > limits.max_arguments || program_bytes > limits.max_argument_bytes {
        return Err(PlanningError::Bounds);
    }
    let mut total = program_bytes.checked_add(1).ok_or(PlanningError::Bounds)?;
    for argument in arguments {
        if argument.contains('\0') || argument.len() > limits.max_argument_bytes {
            return Err(PlanningError::Bounds);
        }
        total = total
            .checked_add(argument.len())
            .and_then(|sum| sum.checked_add(1))
            .ok_or(PlanningError::Bounds)?;
    }
    if total > limits.max_total_bytes {
        return Err(PlanningError::Bounds);
    }
    Ok(())
}
pub fn needs_runtime(config: &RuntimeExecutionConfig) -> bool {
    config
        .environment
        .as_ref()
        .is_some_and(|environment| environment.env_type != "none")
        || config.dependencies.is_some()
}
/// `selected` has already passed CP origin/authority admission; no ambient
/// process environment is consulted. Package-manager hooks remain descendants
/// of these steps inside the same single Setup container/private Stage.
pub fn setup_plan(
    config: &RuntimeExecutionConfig,
    selected: &BTreeMap<String, String>,
    hooks: &[Vec<String>],
) -> Result<SetupPlan> {
    let vars = variables(config, None, true, PackAccess::ReadOnly)?;
    let mut environment = runtime_environment(config, selected, &vars)?;
    for (key, value) in [
        ("HOME", "/tmp"),
        ("TMPDIR", "/tmp"),
        ("XDG_CONFIG_HOME", "/tmp/config"),
        ("XDG_CACHE_HOME", "/tmp/cache"),
        ("PIP_CACHE_DIR", "/tmp/cache/pip"),
        ("PIP_CONFIG_FILE", "/dev/null"),
    ] {
        environment.insert(key.into(), value.into());
    }
    let mut steps = Vec::new();
    if let Some(config) = config
        .environment
        .as_ref()
        .filter(|environment| environment.env_type != "none")
    {
        if config.create_command.is_empty() {
            return Err(PlanningError::Configuration);
        }
        steps.push(SetupStep {
            argv: render_command(&config.create_command, &vars),
            if_pack_file: None,
        });
    }
    if let Some(dependency) = &config.dependencies {
        pack_path(&dependency.manifest_file)?;
        if dependency.install_command.is_empty() {
            return Err(PlanningError::Configuration);
        }
        steps.push(SetupStep {
            argv: render_command(&dependency.install_command, &vars),
            if_pack_file: Some(dependency.manifest_file.clone()),
        });
    }
    for hook in hooks {
        steps.push(SetupStep {
            argv: render_command(hook, &vars),
            if_pack_file: None,
        });
    }
    let plan = SetupPlan {
        version: 1,
        environment,
        steps,
    };
    plan.validate()?;
    Ok(plan)
}
/// The CP must supply an actual runtime-object build proof before selecting
/// `has_runtime`. Missing setup never falls back to a CP host interpreter.
pub fn interpreted_argv(
    config: &RuntimeExecutionConfig,
    entry: &str,
    has_runtime: bool,
    pack_access: PackAccess,
) -> Result<Vec<String>> {
    if config.interpreter.binary.is_empty() || (needs_runtime(config) && !has_runtime) {
        return Err(PlanningError::Configuration);
    }
    let vars = variables(config, Some(entry), has_runtime, pack_access)?;
    let mut argv = vec![vars["interpreter"].clone()];
    argv.extend(render_command(&config.interpreter.args, &vars));
    argv.push(action_path(
        entry,
        has_runtime && pack_access == PackAccess::ReadOnly,
    )?);
    Ok(argv)
}
pub fn native_program(entry: &str, is_directory: bool, platform: LaunchPlatform) -> Result<String> {
    let path = pack_path(entry)?;
    Ok(if is_directory {
        format!("{path}/linux-{}", platform.native_arch())
    } else {
        path
    })
}
pub fn action_environment(
    config: &RuntimeExecutionConfig,
    selected: &BTreeMap<String, String>,
    entry: &str,
    has_runtime: bool,
    pack_access: PackAccess,
) -> Result<BTreeMap<String, String>> {
    if needs_runtime(config) && !has_runtime {
        return Err(PlanningError::Configuration);
    }
    // The owning loop enforces API opt-in, origins and the current attempt.
    // Retain its explicit scoped token while rejecting manager/control context.
    validate_environment(selected, true)?;
    let vars = if entry.starts_with('/') {
        // Installed native commands were selected in the admitted image. Keep
        // their exact guest path; setup/project paths remain pack-relative.
        if has_runtime
            || pack_access != PackAccess::ReadOnly
            || !matches!(config.interpreter.binary.as_str(), "" | "native" | "none")
            || entry.contains(['\\', '\0'])
            || entry.chars().any(char::is_control)
            || entry
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
        {
            return Err(PlanningError::Path);
        }
        let mut vars = variables(config, None, false, pack_access)?;
        vars.insert("action_file", entry.to_owned());
        vars
    } else {
        variables(config, Some(entry), has_runtime, pack_access)?
    };
    let mut output = BTreeMap::from([("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())]);
    apply_runtime_environment(config, &mut output, &vars);
    output.extend(selected.clone());
    validate_environment(&output, true)?;
    Ok(output)
}
fn variables(
    config: &RuntimeExecutionConfig,
    entry: Option<&str>,
    has_runtime: bool,
    pack_access: PackAccess,
) -> Result<BTreeMap<&'static str, String>> {
    let use_project = has_runtime && pack_access == PackAccess::ReadOnly;
    let mut vars = BTreeMap::from([
        (
            "pack_dir",
            if use_project { PROJECT_ROOT } else { PACK_ROOT }.into(),
        ),
        ("env_dir", RUNTIME_ROOT.into()),
    ]);
    if let Some(dependency) = &config.dependencies {
        vars.insert(
            "manifest_path",
            action_path(&dependency.manifest_file, use_project)?,
        );
    }
    if let Some(entry) = entry {
        vars.insert("action_file", action_path(entry, use_project)?);
    }
    let interpreter = if has_runtime {
        config
            .environment
            .as_ref()
            .filter(|environment| environment.env_type != "none")
            .and_then(|environment| environment.interpreter_path.as_deref())
            .map(|template| render(template, &vars))
            .unwrap_or_else(|| config.interpreter.binary.clone())
    } else {
        config.interpreter.binary.clone()
    };
    vars.insert("interpreter", interpreter);
    Ok(vars)
}
fn runtime_environment(
    config: &RuntimeExecutionConfig,
    selected: &BTreeMap<String, String>,
    vars: &BTreeMap<&str, String>,
) -> Result<BTreeMap<String, String>> {
    validate_environment(selected, false)?;
    let mut output = selected.clone();
    output
        .entry("PATH".into())
        .or_insert_with(|| "/usr/local/bin:/usr/bin:/bin".into());
    apply_runtime_environment(config, &mut output, vars);
    validate_environment(&output, false)?;
    Ok(output)
}
fn apply_runtime_environment(
    config: &RuntimeExecutionConfig,
    output: &mut BTreeMap<String, String>,
    vars: &BTreeMap<&str, String>,
) {
    for (name, specification) in &config.env_vars {
        if attune_common::execution_env::is_reserved_execution_env_var(name) {
            continue;
        }
        let value = match specification {
            RuntimeEnvVarConfig::Value(value) => render(value, vars),
            RuntimeEnvVarConfig::Spec(specification) => {
                let resolved = render(&specification.value, vars);
                let existing = output.get(name).map(String::as_str).unwrap_or_default();
                let (left, right) = match specification.operation {
                    RuntimeEnvVarOperation::Set => (resolved.as_str(), ""),
                    RuntimeEnvVarOperation::Prepend => (resolved.as_str(), existing),
                    RuntimeEnvVarOperation::Append => (existing, resolved.as_str()),
                };
                match (left.is_empty(), right.is_empty()) {
                    (true, _) => right.into(),
                    (_, true) => left.into(),
                    _ => format!("{left}{}{right}", specification.separator),
                }
            }
        };
        output.insert(name.clone(), value);
    }
}
fn validate_environment(
    environment: &BTreeMap<String, String>,
    selected_action_token: bool,
) -> Result<()> {
    let mut bytes = 0usize;
    for (name, value) in environment {
        let upper = name.to_ascii_uppercase();
        if name.is_empty()
            || name.bytes().enumerate().any(|(index, byte)| {
                !(byte.is_ascii_alphabetic() || byte == b'_' || index > 0 && byte.is_ascii_digit())
            })
            || value.contains('\0')
            || upper.starts_with("ATTUNE__")
            || upper.starts_with("ATTUNE_LAUNCH_")
            || upper.starts_with("ATTUNE_RUNTIME_LOG_")
            || upper.starts_with("DOCKER_")
            || upper.starts_with("KUBERNETES_")
            || (upper == "ATTUNE_API_TOKEN"
                && (!selected_action_token || name != "ATTUNE_API_TOKEN"))
            || matches!(
                upper.as_str(),
                "DATABASE_URL"
                    | "MQ_URL"
                    | "ATTUNE_MQ_URL"
                    | "ATTUNE_MQ_EXCHANGE"
                    | "JWT_SECRET"
                    | "ENCRYPTION_KEY"
                    | "KUBECONFIG"
                    | "ATTUNE_CONFIG"
                    | "ATTUNE_AGENT_BOOTSTRAP_TOKEN"
            )
        {
            return Err(PlanningError::Environment);
        }
        bytes = bytes
            .checked_add(name.len() + value.len())
            .ok_or(PlanningError::Bounds)?;
    }
    if bytes > 256 * 1024 {
        return Err(PlanningError::Bounds);
    }
    Ok(())
}
fn action_path(relative: &str, has_runtime: bool) -> Result<String> {
    let original = pack_path(relative)?;
    Ok(if has_runtime {
        format!("{PROJECT_ROOT}/{relative}")
    } else {
        original
    })
}
fn pack_path(relative: &str) -> Result<String> {
    if relative.is_empty()
        || relative.contains(['\\', '\0'])
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PlanningError::Path);
    }
    Ok(format!("{PACK_ROOT}/{relative}"))
}
fn render_command(command: &[String], vars: &BTreeMap<&str, String>) -> Vec<String> {
    command.iter().map(|value| render(value, vars)).collect()
}
/// Single pass: a replacement value is never interpreted as another template.
fn render(template: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut output = String::new();
    let mut remaining = template;
    while !remaining.is_empty() {
        if let Some((token, value)) = vars
            .iter()
            .map(|(key, value)| (format!("{{{key}}}"), value))
            .find(|(token, _)| remaining.starts_with(token))
        {
            output.push_str(value);
            remaining = &remaining[token.len()..];
        } else {
            let character = remaining.chars().next().expect("nonempty");
            output.push(character);
            remaining = &remaining[character.len_utf8()..];
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_image_native_command_path_never_changes_project_path_rules() {
        let mut native: RuntimeExecutionConfig =
            serde_json::from_value(serde_json::json!({})).unwrap();
        native.env_vars.insert(
            "SELECTED_FILE".into(),
            RuntimeEnvVarConfig::Value("{action_file}".into()),
        );
        let environment = action_environment(
            &native,
            &BTreeMap::new(),
            "/echo",
            false,
            PackAccess::ReadOnly,
        )
        .unwrap();
        assert_eq!(environment["SELECTED_FILE"], "/echo");
        for path in [
            "/",
            "/a/../echo",
            "/a/./echo",
            "/a//echo",
            "/echo/",
            "/echo\n",
            "/echo\0",
            "/a\\echo",
        ] {
            assert!(action_environment(
                &native,
                &BTreeMap::new(),
                path,
                false,
                PackAccess::ReadOnly
            )
            .is_err());
        }
        assert!(action_environment(
            &native,
            &BTreeMap::new(),
            "/echo",
            true,
            PackAccess::ReadOnly
        )
        .is_err());
        assert!(action_environment(
            &native,
            &BTreeMap::new(),
            "/echo",
            false,
            PackAccess::CandidateCopy
        )
        .is_err());
        assert!(action_environment(
            &python(),
            &BTreeMap::new(),
            "/echo",
            false,
            PackAccess::ReadOnly
        )
        .is_err());
        assert!(action_path("/echo", false).is_err());
        assert!(action_path("/echo", true).is_err());
    }

    fn python() -> RuntimeExecutionConfig {
        let pack: serde_json::Value =
            serde_yaml_ng::from_str(include_str!("../../../packs/core/runtimes/python.yaml"))
                .unwrap();
        serde_json::from_value(pack["execution_config"].clone()).unwrap()
    }
    #[test]
    fn python_setup_and_action_keep_stable_paths_without_host_fallback() {
        let config = python();
        let plan = setup_plan(&config, &BTreeMap::new(), &[]).unwrap();
        assert_eq!(
            plan.steps[0].argv,
            ["python3", "-m", "venv", "--copies", RUNTIME_ROOT]
        );
        assert_eq!(plan.steps[1].argv[0], "/opt/attune/runtime/bin/python3");
        assert_eq!(
            plan.steps[1].if_pack_file.as_deref(),
            Some("requirements.txt")
        );
        assert_eq!(plan.environment["HOME"], "/tmp");
        assert_eq!(
            plan.steps[1].argv.last().unwrap(),
            "/opt/attune/runtime/project/requirements.txt"
        );
        assert_eq!(
            working_directory(attune_common::workload_launch::LaunchRoot::Runtime),
            PROJECT_ROOT
        );
        assert_eq!(
            working_directory(attune_common::workload_launch::LaunchRoot::Stage),
            RUNTIME_ROOT
        );
        assert_eq!(
            working_directory(attune_common::workload_launch::LaunchRoot::Pack),
            PACK_ROOT
        );
        assert!(interpreted_argv(&config, "actions/tool.py", false, PackAccess::ReadOnly).is_err());
        assert_eq!(
            interpreted_argv(&config, "actions/tool.py", true, PackAccess::ReadOnly).unwrap(),
            [
                "/opt/attune/runtime/bin/python3",
                "-u",
                "/opt/attune/runtime/project/actions/tool.py"
            ]
        );
    }
    #[test]
    fn candidate_source_stays_in_writable_pack_while_interpreter_and_dependencies_use_runtime() {
        let config = python();
        assert_eq!(
            interpreted_argv(&config, "actions/test.py", true, PackAccess::CandidateCopy).unwrap(),
            [
                "/opt/attune/runtime/bin/python3",
                "-u",
                "/opt/attune/pack/actions/test.py"
            ]
        );
        let vars = variables(
            &config,
            Some("actions/test.py"),
            true,
            PackAccess::CandidateCopy,
        )
        .unwrap();
        assert_eq!(vars["pack_dir"], PACK_ROOT);
        assert_eq!(vars["manifest_path"], "/opt/attune/pack/requirements.txt");
        assert_eq!(vars["interpreter"], "/opt/attune/runtime/bin/python3");
        assert_eq!(
            working_directory(attune_common::workload_launch::LaunchRoot::Pack),
            PACK_ROOT
        );
    }
    #[test]
    fn canonical_dependency_plans_do_not_swallow_install_or_manifest_copy_errors() {
        for source in [
            include_str!("../../../packs/core/runtimes/ruby.yaml"),
            include_str!("../../../packs/core/runtimes/go.yaml"),
            include_str!("../../../packs/core/runtimes/r.yaml"),
            include_str!("../../../packs/core/runtimes/perl.yaml"),
            include_str!("../../../packs/core/runtimes/nodejs.yaml"),
        ] {
            let definition: serde_json::Value = serde_yaml_ng::from_str(source).unwrap();
            let config: RuntimeExecutionConfig =
                serde_json::from_value(definition["execution_config"].clone()).unwrap();
            let plan = setup_plan(&config, &BTreeMap::new(), &[]).unwrap();
            for step in &plan.steps {
                assert!(!step
                    .argv
                    .iter()
                    .any(|s| s.contains("|| true") || s.contains("2>/dev/null")));
            }
            assert!(plan.steps[1].if_pack_file.is_some());
            assert!(plan
                .steps
                .iter()
                .flat_map(|step| &step.argv)
                .any(|s| s.contains(PROJECT_ROOT)));
        }
    }
    #[test]
    fn native_selection_uses_admitted_platform_and_paths_cannot_escape() {
        assert_eq!(
            native_program("bin/tool", true, LaunchPlatform::LinuxArm64).unwrap(),
            "/opt/attune/pack/bin/tool/linux-arm64"
        );
        assert_eq!(
            runner_program(LaunchPlatform::LinuxAmd64),
            "/opt/attune/runner/linux-amd64"
        );
        for path in [
            "../secret",
            "/etc/shadow",
            "actions/../tool",
            "actions//tool",
            "actions\\tool",
        ] {
            assert!(native_program(path, false, LaunchPlatform::LinuxAmd64).is_err());
        }
    }
    #[test]
    fn private_plan_never_debugs_values_or_accepts_manager_authority() {
        let config = python();
        let environment =
            BTreeMap::from([("PIP_INDEX_URL".into(), "owned-index-credential".into())]);
        let plan = setup_plan(&config, &environment, &[]).unwrap();
        assert!(!format!("{plan:?}").contains("owned-index-credential"));
        for name in [
            "ATTUNE_API_TOKEN",
            "ATTUNE__DATABASE__URL",
            "ATTUNE_LAUNCH_SESSION",
            "ATTUNE_RUNTIME_LOG_CUSTODY_KEY",
            "DOCKER_HOST",
        ] {
            assert!(setup_plan(
                &config,
                &BTreeMap::from([(name.into(), "denied".into())]),
                &[]
            )
            .is_err());
        }
        let mut invalid = plan;
        invalid.version = 2;
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn action_environment_retains_selected_api_token_without_runtime_default_authority() {
        let mut config = python();
        config.env_vars.insert(
            "ATTUNE_API_TOKEN".into(),
            RuntimeEnvVarConfig::Value("runtime-must-not-mint-or-replace-token".into()),
        );
        let selected = BTreeMap::from([
            (
                "ATTUNE_API_TOKEN".into(),
                "owned-selected-scoped-token".into(),
            ),
            ("ATTUNE_ACTION".into(), "owned.action".into()),
        ]);
        let result = action_environment(
            &config,
            &selected,
            "actions/tool.py",
            true,
            PackAccess::ReadOnly,
        )
        .unwrap();
        assert_eq!(result["ATTUNE_API_TOKEN"], "owned-selected-scoped-token");
        assert_eq!(result["ATTUNE_ACTION"], "owned.action");
        assert!(!action_environment(
            &config,
            &BTreeMap::new(),
            "actions/tool.py",
            true,
            PackAccess::ReadOnly,
        )
        .unwrap()
        .contains_key("ATTUNE_API_TOKEN"));
        assert!(setup_plan(&config, &selected, &[]).is_err());
        for key in [
            "ATTUNE__DATABASE__URL",
            "DATABASE_URL",
            "MQ_URL",
            "ATTUNE_MQ_URL",
            "ATTUNE_MQ_EXCHANGE",
            "JWT_SECRET",
            "ENCRYPTION_KEY",
            "ATTUNE_CONFIG",
            "ATTUNE_LAUNCH_SESSION",
            "ATTUNE_RUNTIME_LOG_CUSTODY_KEY",
            "DOCKER_HOST",
            "KUBECONFIG",
            "attune_api_token",
        ] {
            let error = action_environment(
                &config,
                &BTreeMap::from([(key.into(), "owned-private-manager-value".into())]),
                "actions/tool.py",
                true,
                PackAccess::ReadOnly,
            )
            .unwrap_err();
            assert!(!error.to_string().contains("owned-private-manager-value"));
        }
        config.env_vars.insert(
            "JWT_SECRET".into(),
            RuntimeEnvVarConfig::Value("owned-private-runtime-value".into()),
        );
        assert!(action_environment(
            &config,
            &BTreeMap::new(),
            "actions/tool.py",
            true,
            PackAccess::ReadOnly,
        )
        .is_err());
    }
    #[test]
    fn rendering_never_executes_hooks_or_reinterprets_replacement_values() {
        let root = tempfile::tempdir().unwrap();
        let sentinel = root.path().join("must-not-run");
        let hook = vec![
            "/usr/bin/touch".into(),
            sentinel.to_string_lossy().into_owned(),
        ];
        let plan = setup_plan(&python(), &BTreeMap::new(), &[hook]).unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert!(!sentinel.exists());
        let entry = "actions/{env_dir}.py";
        assert_eq!(
            interpreted_argv(&python(), entry, true, PackAccess::ReadOnly)
                .unwrap()
                .last()
                .unwrap(),
            "/opt/attune/runtime/project/actions/{env_dir}.py"
        );
        let selected = BTreeMap::from([("PYTHONPATH".into(), "selected-override".into())]);
        assert_eq!(
            action_environment(&python(), &selected, entry, true, PackAccess::ReadOnly).unwrap()
                ["PYTHONPATH"],
            "selected-override"
        );
    }
    #[test]
    fn private_cli_envelope_preserves_named_image_program_and_literal_arguments_without_host_lookup(
    ) {
        let selected = attune_common::action_cli::SelectedProgram {
            path: "/opt/owned-image-only/program-not-on-cp-host".into(),
            sha256: "a".repeat(64),
            invocation_name: "static-multicall-name".into(),
            artifact: None,
            variant: None,
        };
        let arguments = vec![
            "".into(),
            "spaces and quotes '\"; $(false)".into(),
            "--".into(),
        ];
        let argv = cli_exec_argv(LaunchPlatform::LinuxArm64, &selected, &arguments).unwrap();
        assert_eq!(
            &argv[..5],
            &[
                "/opt/attune/runner/linux-arm64",
                "exec",
                &"a".repeat(64),
                "/opt/owned-image-only/program-not-on-cp-host",
                "static-multicall-name"
            ]
        );
        assert_eq!(&argv[5..], &arguments);
    }

    #[test]
    fn private_cli_envelope_rejects_unbound_program_shape_and_shared_argument_budget_overflow() {
        let mut selected = attune_common::action_cli::SelectedProgram {
            path: "/usr/bin/owned-tool".into(),
            sha256: "a".repeat(64),
            invocation_name: "tool".into(),
            artifact: None,
            variant: None,
        };
        selected.sha256 = "A".repeat(64);
        assert!(cli_exec_argv(LaunchPlatform::LinuxAmd64, &selected, &[]).is_err());
        selected.sha256 = "a".repeat(64);
        selected.path = "relative/tool".into();
        assert!(cli_exec_argv(LaunchPlatform::LinuxAmd64, &selected, &[]).is_err());
        selected.path = "/usr/bin/owned-tool".into();
        assert!(cli_exec_argv(LaunchPlatform::LinuxAmd64, &selected, &["\0".into()]).is_err());
        assert!(cli_exec_argv(
            LaunchPlatform::LinuxAmd64,
            &selected,
            &vec!["x".repeat(16384); 4]
        )
        .is_err());
        selected.invocation_name.clear();
        assert!(cli_exec_argv(LaunchPlatform::LinuxAmd64, &selected, &[]).is_err());
    }
}
