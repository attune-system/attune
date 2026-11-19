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
/// Keep every private JSON fragment below Linux's per-argument byte limit.
pub const RUNTIME_RECIPE_CHUNK_BYTES: usize = 32 * 1024;
pub const RUNTIME_TOOL_RESULT_BYTES: usize = 4 * 1024 * 1024;
/// Private per-attempt control, injected by the trusted producer AFTER ordinary
/// invocation environment validation and frozen in its encrypted context.
pub const RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV: &str = "ATTUNE_LAUNCH_RECIPE_TIMEOUT_CHALLENGE";
pub const RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES: usize = 64;
/// Fixed relative leaf under the native attempt's existing artifact root.
pub const RUNTIME_RECIPE_TIMEOUT_RECEIPT: &str = ".attune-recipe-timeout";
pub use attune_common::workload_launch::{
    CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES, CANDIDATE_PYTHON_PREP_CHALLENGE_ENV,
    CANDIDATE_PYTHON_PREP_RECEIPT,
};
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
    #[error("runtime lifecycle recipes require an isolated private-generation execution path")]
    Lifecycle,
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
/// Private invocation, never a public Runner input or a release manifest.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePythonPlan {
    pub version: u8,
    pub config: RuntimeExecutionConfig,
    pub runner: String,
    pub entry: String,
    pub timeout_ms: u64,
}
impl CandidatePythonPlan {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !matches!(self.runner.as_str(), "unittest" | "pytest" | "script")
            || !(1..=3_600_000).contains(&self.timeout_ms)
        {
            return Err(PlanningError::Configuration);
        }
        pack_path(&self.entry)?;
        candidate_python_setup(&self.config, "/tmp/attune-candidate-python-validation/env")?;
        if serde_json::to_vec(self)
            .map_err(|_| PlanningError::Bounds)?
            .len()
            > SETUP_STDIN_LIMIT
        {
            return Err(PlanningError::Bounds);
        }
        Ok(())
    }
}
pub fn candidate_python_argv(
    platform: LaunchPlatform,
    plan: &CandidatePythonPlan,
) -> Result<Vec<String>> {
    plan.validate()?;
    let json = serde_json::to_string(plan).map_err(|_| PlanningError::Bounds)?;
    let mut argv = vec![runner_program(platform), "run-candidate-python".into()];
    let mut start = 0;
    while start < json.len() {
        let mut end = (start + RUNTIME_RECIPE_CHUNK_BYTES).min(json.len());
        while !json.is_char_boundary(end) {
            end -= 1;
        }
        argv.push(json[start..end].into());
        start = end;
    }
    Ok(argv)
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
/// Probe an already-admitted definition in the selected guest image. This is
/// private invocation context, never Pod metadata or a public Runner archive.
/// The caller selects the guest PATH explicitly; manager PATH is not consulted.
/// This runs only tool probes, not preparation or the recipe launch command.
pub fn runtime_tool_probe_argv(
    platform: LaunchPlatform,
    recipe: &attune_common::runtime_lifecycle::RuntimeRecipe,
    selected_guest_path: &str,
) -> Result<Vec<String>> {
    recipe
        .validate()
        .map_err(|_| PlanningError::Configuration)?;
    if selected_guest_path.contains('\0') {
        return Err(PlanningError::Environment);
    }
    let definition = serde_json::to_string(recipe).map_err(|_| PlanningError::Configuration)?;
    if definition.len() > crate::runner_protocol::MAX_START_CONTEXT {
        return Err(PlanningError::Bounds);
    }
    let mut argv = vec![
        runner_program(platform),
        "inspect-runtime-tools".into(),
        selected_guest_path.into(),
    ];
    let mut offset = 0;
    while offset < definition.len() {
        let mut end = (offset + RUNTIME_RECIPE_CHUNK_BYTES).min(definition.len());
        while !definition.is_char_boundary(end) {
            end -= 1;
        }
        argv.push(definition[offset..end].into());
        offset = end;
    }
    // Account for the existing private framing/JSON encoding budget as well
    // as the definition itself; do not introduce another transport or format.
    crate::runner_protocol::encode_start(&attune_common::workload_launch::LaunchInvocation {
        argv: argv.clone(),
        env: BTreeMap::new(),
        working_root: attune_common::workload_launch::LaunchRoot::Pack,
    })
    .map_err(|_| PlanningError::Bounds)?;
    Ok(argv)
}
/// Private CP-selected envelope, never a public Runner body or caller plan.
/// Selection/provenance must already be bound to the admitted immutable image
/// or native input. This function never probes the CP host filesystem/PATH.
/// Legacy version/availability probes use their exact public release metadata,
/// separately from operator-admitted lifecycle recipes. No invocation context.
pub fn runtime_verification_argv(
    platform: LaunchPlatform,
    definition: &attune_common::runtime_detection::NativeRuntimeVerification,
    guest_path: &str,
) -> Result<Vec<String>> {
    if definition.execution_config.get("lifecycle").is_some() || guest_path.contains('\0') {
        return Err(PlanningError::Configuration);
    }
    let json = serde_json::to_string(definition).map_err(|_| PlanningError::Configuration)?;
    let mut argv = vec![
        runner_program(platform),
        "inspect-runtime-verification".into(),
        guest_path.into(),
    ];
    let mut rest = json.as_str();
    while !rest.is_empty() {
        let mut end = rest.len().min(RUNTIME_RECIPE_CHUNK_BYTES);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        argv.push(rest[..end].into());
        rest = &rest[end..];
    }
    crate::runner_protocol::encode_start(&attune_common::workload_launch::LaunchInvocation {
        argv: argv.clone(),
        env: BTreeMap::new(),
        working_root: attune_common::workload_launch::LaunchRoot::Pack,
    })
    .map_err(|_| PlanningError::Bounds)?;
    Ok(argv)
}

/// Fixed guest lifecycle invocation. The definition and source are private
/// context, never provider metadata. Source is relative to the pinned release;
/// inline source admission remains an owning-loop decision.
pub fn runtime_recipe_argv(
    platform: LaunchPlatform,
    recipe: &attune_common::runtime_lifecycle::RuntimeRecipe,
    guest_path: &str,
    source: &str,
    inline: bool,
    selected_version: Option<&str>,
) -> Result<Vec<String>> {
    if !inline {
        pack_path(source)?;
    }
    if source.contains('\0') || selected_version.is_some_and(|v| v.is_empty() || v.contains('\0')) {
        return Err(PlanningError::Configuration);
    }
    let mut argv = runtime_tool_probe_argv(platform, recipe, guest_path)?;
    argv[1] = "run-runtime-recipe".into();
    argv.insert(2, if inline { "inline" } else { "file" }.into());
    // Source may exceed Linux's per-argument limit as well.
    let source_json = serde_json::to_string(source).map_err(|_| PlanningError::Configuration)?;
    // Split on UTF-8 boundaries using the same bounded encoder as recipes.
    let mut parts = Vec::new();
    let mut rest = source_json.as_str();
    while !rest.is_empty() {
        let mut end = rest.len().min(RUNTIME_RECIPE_CHUNK_BYTES);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(rest[..end].to_owned());
        rest = &rest[end..];
    }
    let count = parts.len();
    argv.insert(3, parts.len().to_string());
    argv.splice(4..4, parts);
    argv.insert(4 + count, selected_version.unwrap_or("").into());
    if argv.iter().map(String::len).sum::<usize>() > crate::runner_protocol::MAX_START_CONTEXT {
        return Err(PlanningError::Bounds);
    }
    Ok(argv)
}

/// Candidate tests retain their existing private writable Pack source. The
/// closed adapter must admit CandidateCopy; the fixed guest checks that mount.
pub fn candidate_runtime_recipe_argv(
    platform: LaunchPlatform,
    recipe: &attune_common::runtime_lifecycle::RuntimeRecipe,
    guest_path: &str,
    source: &str,
    inline: bool,
    selected_version: Option<&str>,
) -> Result<Vec<String>> {
    let mut argv = runtime_recipe_argv(
        platform,
        recipe,
        guest_path,
        source,
        inline,
        selected_version,
    )?;
    argv[1] = "run-candidate-runtime-recipe".into();
    Ok(argv)
}

/// The owning loop has already selected allowed caller overrides and API scope.
/// Keep the invocation context out of preparation, and reject CP variables.
pub fn runtime_recipe_environment(
    selected: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    validate_environment(selected, true)?;
    Ok(selected.clone())
}

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
    config.lifecycle.is_some()
        || config
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
    require_legacy_configuration(config)?;
    let vars = variables(config, None, true, PackAccess::ReadOnly)?;
    setup_plan_with_variables(config, selected, hooks, &vars)
}
fn setup_plan_with_variables(
    config: &RuntimeExecutionConfig,
    selected: &BTreeMap<String, String>,
    hooks: &[Vec<String>],
    vars: &BTreeMap<&str, String>,
) -> Result<SetupPlan> {
    let mut environment = runtime_environment(config, selected, vars)?;
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
            argv: render_command(&config.create_command, vars),
            if_pack_file: None,
        });
    }
    if let Some(dependency) = &config.dependencies {
        pack_path(&dependency.manifest_file)?;
        if dependency.install_command.is_empty() {
            return Err(PlanningError::Configuration);
        }
        steps.push(SetupStep {
            argv: render_command(&dependency.install_command, vars),
            if_pack_file: Some(dependency.manifest_file.clone()),
        });
    }
    for hook in hooks {
        steps.push(SetupStep {
            argv: render_command(hook, vars),
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
/// Legacy candidate Python setup uses the candidate cwd and one actual private
/// guest generation. No filesystem/PATH lookup or shared Runtime publication.
pub fn candidate_python_setup(
    config: &RuntimeExecutionConfig,
    env_dir: &str,
) -> Result<(SetupPlan, String)> {
    require_legacy_configuration(config)?;
    let relative = env_dir.strip_prefix("/tmp/").ok_or(PlanningError::Path)?;
    attune_common::artifact_transport::ValidatedRelativePath::new(relative)
        .map_err(|_| PlanningError::Path)?;
    let environment = config
        .environment
        .as_ref()
        .filter(|e| e.env_type != "none" && !e.create_command.is_empty())
        .ok_or(PlanningError::Configuration)?;
    let template = environment
        .interpreter_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .ok_or(PlanningError::Configuration)?;
    if config.interpreter.binary.is_empty() {
        return Err(PlanningError::Configuration);
    }
    let mut vars = BTreeMap::from([("pack_dir", PACK_ROOT.into()), ("env_dir", env_dir.into())]);
    let interpreter = render(template, &vars);
    let suffix = interpreter
        .strip_prefix(&format!("{env_dir}/"))
        .ok_or(PlanningError::Path)?;
    attune_common::artifact_transport::ValidatedRelativePath::new(suffix)
        .map_err(|_| PlanningError::Path)?;
    vars.insert("interpreter", interpreter.clone());
    if let Some(dependency) = &config.dependencies {
        vars.insert("manifest_path", pack_path(&dependency.manifest_file)?);
    }
    let plan = setup_plan_with_variables(config, &BTreeMap::new(), &[], &vars)?;
    if plan
        .environment
        .keys()
        .any(|key| key.to_ascii_uppercase().starts_with("ATTUNE_CANDIDATE_"))
    {
        return Err(PlanningError::Environment);
    }
    Ok((plan, interpreter))
}
/// The CP must supply an actual runtime-object build proof before selecting
/// `has_runtime`. Missing setup never falls back to a CP host interpreter.
pub fn interpreted_argv(
    config: &RuntimeExecutionConfig,
    entry: &str,
    has_runtime: bool,
    pack_access: PackAccess,
) -> Result<Vec<String>> {
    require_legacy_configuration(config)?;
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
    require_legacy_configuration(config)?;
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
/// Recipe discovery, per-step cwd/deadline and retained launch targets cannot be
/// represented by the legacy environment/dependency plan. Never ignore recipe
/// metadata even when an interpreter or an existing runtime is also present.
fn require_legacy_configuration(config: &RuntimeExecutionConfig) -> Result<()> {
    if config.lifecycle.is_some() {
        return Err(PlanningError::Lifecycle);
    }
    Ok(())
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

/// Verify mutable DB runtime projections against the exact retained provider
/// release archive before native startup discovery. Never read a mutable tree.
pub fn validate_runtime_release_archive(
    archive: std::fs::File,
    runtime_ref: &str,
    runtime_config: &serde_json::Value,
    version: Option<(&str, &serde_json::Value)>,
) -> Result<()> {
    validate_runtime_release_metadata(archive, runtime_ref, runtime_config, version, None)
}
pub fn validate_runtime_release_metadata(
    archive: std::fs::File,
    runtime_ref: &str,
    runtime_config: &serde_json::Value,
    version: Option<(&str, &serde_json::Value)>,
    distributions: Option<(&serde_json::Value, Option<&serde_json::Value>)>,
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(archive).map_err(|_| PlanningError::Configuration)?;
    let mut matched = false;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| PlanningError::Configuration)?;
        let path = entry.name().map_err(|_| PlanningError::Configuration)?;
        if !path.starts_with("runtimes/") || !(path.ends_with(".yaml") || path.ends_with(".yml")) {
            continue;
        }
        if entry.size() > 1024 * 1024 {
            return Err(PlanningError::Configuration);
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        entry
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| PlanningError::Configuration)?;
        let metadata: serde_json::Value =
            attune_common::runtime_lifecycle::parse_release_metadata(&bytes)
                .map_err(|_| PlanningError::Configuration)?;
        if metadata.get("ref").and_then(|r| r.as_str()) != Some(runtime_ref) {
            continue;
        }
        let empty = serde_json::json!({});
        if distributions
            .is_some_and(|(base, _)| metadata.get("distributions").unwrap_or(&empty) != base)
        {
            return Err(PlanningError::Configuration);
        }
        if matched || metadata.get("execution_config") != Some(runtime_config) {
            return Err(PlanningError::Configuration);
        }
        if let Some((version, version_config)) = version {
            let declared = metadata
                .get("versions")
                .and_then(|v| v.as_array())
                .and_then(|versions| {
                    versions
                        .iter()
                        .find(|v| v.get("version").and_then(|v| v.as_str()) == Some(version))
                })
                .ok_or(PlanningError::Configuration)?;
            if distributions
                .and_then(|(_, version)| version)
                .is_some_and(|expected| declared.get("distributions").unwrap_or(&empty) != expected)
            {
                return Err(PlanningError::Configuration);
            }
            if declared.get("execution_config") != Some(version_config) {
                return Err(PlanningError::Configuration);
            }
        }
        matched = true;
    }
    if !matched {
        return Err(PlanningError::Configuration);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_recipes_never_fall_through_legacy_planning() {
        let recipe: attune_common::runtime_lifecycle::RuntimeRecipe =
            serde_json::from_value(serde_json::json!({
            "tools":[{
                "name":"fixture", "binaries":["fixture"],
                "version_pattern":"(?<version>1)", "primary":true
            }],
            "prepare":[{"command":["{tool_fixture}","build"],"cwd":"{workspace_dir}"}],
            "launch":{"command":["{tool_fixture}","{source_file}"]}
            }))
            .unwrap();
        // Both recipe-only configurations and recipes with legacy fields must
        // retain their distinct preparation/launch semantics.
        for mut config in [RuntimeExecutionConfig::default(), python()] {
            config.lifecycle = Some(recipe.clone());
            assert!(needs_runtime(&config));
            assert!(matches!(
                setup_plan(&config, &BTreeMap::new(), &[]),
                Err(PlanningError::Lifecycle)
            ));
            for access in [PackAccess::ReadOnly, PackAccess::CandidateCopy] {
                for has_runtime in [false, true] {
                    assert!(matches!(
                        interpreted_argv(&config, "actions/tool", has_runtime, access),
                        Err(PlanningError::Lifecycle)
                    ));
                    assert!(matches!(
                        action_environment(
                            &config,
                            &BTreeMap::new(),
                            "actions/tool",
                            has_runtime,
                            access
                        ),
                        Err(PlanningError::Lifecycle)
                    ));
                }
            }
        }
    }
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
    fn candidate_python_keeps_candidate_cwd_and_stable_private_interpreter() {
        let (plan, interpreter) =
            candidate_python_setup(&python(), "/tmp/owned-candidate/env").unwrap();
        assert_eq!(
            plan.steps[0].argv,
            [
                "python3",
                "-m",
                "venv",
                "--copies",
                "/tmp/owned-candidate/env"
            ]
        );
        assert_eq!(interpreter, "/tmp/owned-candidate/env/bin/python3");
        assert_eq!(plan.steps[1].argv[0], interpreter);
        assert_eq!(
            plan.steps[1].argv.last().unwrap(),
            &format!("{PACK_ROOT}/requirements.txt")
        );
        assert_eq!(
            plan.steps[1].if_pack_file.as_deref(),
            Some("requirements.txt")
        );
        assert_eq!(plan.environment["PYTHONPATH"], format!("{PACK_ROOT}/lib"));
        assert_eq!(plan.environment["PATH"], "/usr/local/bin:/usr/bin:/bin");
        assert!(!serde_json::to_string(&plan).unwrap().contains(RUNTIME_ROOT));
    }
    #[test]
    fn candidate_python_has_no_missing_definition_or_interpreter_fallback() {
        assert!(candidate_python_setup(&Default::default(), "/tmp/owned/env").is_err());
        let mut config = python();
        config.environment.as_mut().unwrap().interpreter_path = None;
        assert!(candidate_python_setup(&config, "/tmp/owned/env").is_err());
        config.environment.as_mut().unwrap().interpreter_path = Some("/usr/bin/python3".into());
        assert!(candidate_python_setup(&config, "/tmp/owned/env").is_err());
        assert!(candidate_python_setup(&python(), "/opt/attune/runtime").is_err());
        assert!(candidate_python_setup(&python(), "/tmp/owned/../shared").is_err());
        let mut config = python();
        config
            .dependencies
            .as_mut()
            .unwrap()
            .install_command
            .clear();
        assert!(candidate_python_setup(&config, "/tmp/owned/env").is_err());
    }
    #[test]
    fn candidate_python_preparation_cannot_receive_invocation_authority() {
        let mut config = python();
        config.env_vars.insert(
            "ATTUNE_CANDIDATE_EFFECTS_TOKEN".into(),
            RuntimeEnvVarConfig::Value("forbidden".into()),
        );
        let (plan, _) = candidate_python_setup(&config, "/tmp/owned/env").unwrap();
        assert!(!plan
            .environment
            .contains_key("ATTUNE_CANDIDATE_EFFECTS_TOKEN"));
        let mut config = python();
        config.env_vars.insert(
            "ATTUNE_API_TOKEN".into(),
            RuntimeEnvVarConfig::Value("forbidden".into()),
        );
        // Reserved execution defaults are ignored by the shared renderer.
        let (plan, _) = candidate_python_setup(&config, "/tmp/owned/env").unwrap();
        assert!(!plan.environment.contains_key("ATTUNE_API_TOKEN"));
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
    #[test]
    fn runtime_release_metadata_must_match_exact_retained_recipe_and_version() {
        use std::io::Write;
        let config = serde_json::json!({"lifecycle":{"owned":"public"}});
        let make = |duplicates: bool| {
            let mut file = tempfile::tempfile().unwrap();
            {
                let mut zip = zip::ZipWriter::new(&mut file);
                for name in if duplicates {
                    vec!["runtimes/owned.yaml", "runtimes/duplicate.yaml"]
                } else {
                    vec!["runtimes/owned.yaml"]
                } {
                    zip.start_file(name, zip::write::SimpleFileOptions::default())
                        .unwrap();
                    zip.write_all(serde_json::to_string(&serde_json::json!({"ref":"owned.tool","execution_config":config,"versions":[{"version":"1","execution_config":config}]})).unwrap().as_bytes()).unwrap();
                }
                zip.finish().unwrap();
            }
            file
        };
        assert!(validate_runtime_release_archive(
            make(false),
            "owned.tool",
            &config,
            Some(("1", &config))
        )
        .is_ok());
        assert!(
            validate_runtime_release_archive(make(false), "other.tool", &config, None).is_err()
        );
        assert!(validate_runtime_release_archive(
            make(false),
            "owned.tool",
            &serde_json::json!({}),
            None
        )
        .is_err());
        assert!(validate_runtime_release_archive(
            make(false),
            "owned.tool",
            &config,
            Some(("2", &config))
        )
        .is_err());
        assert!(validate_runtime_release_archive(make(true), "owned.tool", &config, None).is_err());
    }
}
