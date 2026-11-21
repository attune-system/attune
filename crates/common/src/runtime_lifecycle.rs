//! Pack-defined toolchains for the explicitly admitted trusted execution profile.
//!
//! Preparation gets no execution context or credentials. Every preparation owns a
//! fresh, nonrelocatable generation; reuse and cross-host distribution are disabled.
//! Unconfirmed cleanup leaves that generation quarantined, never published/reused.

use crate::child_process_environment::ChildProcessEnvironment;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const CAPTURE_BYTES: usize = 64 * 1024;
const MAX_WORKSPACE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_WORKSPACE_FILES: usize = 20_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("Runtime filesystem/process operation failed: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Exact runtime identity is authoritative; aliases only express generic tool
/// requirements and must never supply another runtime's selected versions.
pub fn capability_for<'a>(
    capabilities: Option<&'a serde_json::Value>,
    runtime: &crate::models::Runtime,
) -> Option<&'a serde_json::Value> {
    let recipe: RuntimeRecipe =
        serde_json::from_value(runtime.execution_config.get("lifecycle")?.clone()).ok()?;
    let digest = recipe.digest().ok()?;
    let instance = capabilities?
        .get("runtime_toolchains")?
        .get(&runtime.r#ref)?;
    (instance.get("digest")?.as_str()? == digest).then_some(instance)
}

pub fn available_versions(
    capabilities: Option<&serde_json::Value>,
    runtime: &crate::models::Runtime,
) -> Vec<String> {
    capability_for(capabilities, runtime)
        .and_then(|instance| instance.get("versions")?.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

/// Generic required_runtimes constraints may match any advertised instance.
pub fn versions_matching_names(capabilities: &serde_json::Value, names: &[String]) -> Vec<String> {
    capabilities
        .get("runtime_toolchains")
        .and_then(|value| value.as_object())
        .into_iter()
        .flat_map(|instances| instances.values())
        .filter(|instance| {
            instance
                .get("names")
                .and_then(|value| value.as_array())
                .is_some_and(|aliases| {
                    aliases
                        .iter()
                        .filter_map(|value| value.as_str())
                        .any(|alias| names.iter().any(|name| name == alias))
                })
        })
        .filter_map(|instance| instance.get("versions")?.as_array())
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

/// A version cannot replace its parent's execution/admission model.
pub fn validate_version_mode(
    parent: &serde_json::Value,
    version: &serde_json::Value,
) -> Result<()> {
    if parent.get("lifecycle").is_some() != version.get("lifecycle").is_some() {
        return Err(invalid(
            "Runtime parent and version must use the same lifecycle mode",
        ));
    }
    Ok(())
}

pub async fn verify_registered_version(
    version: &crate::models::RuntimeVersion,
    environment: &ChildProcessEnvironment,
) -> bool {
    let Some(raw) = version.execution_config.get("lifecycle") else {
        return false;
    };
    let Ok(recipe) = serde_json::from_value::<RuntimeRecipe>(raw.clone()) else {
        return false;
    };
    let Ok(tools) = recipe.discover(environment).await else {
        return false;
    };
    let Some(primary) = recipe.tools.iter().find(|tool| tool.primary) else {
        return false;
    };
    tools.get(&primary.name).is_some_and(|tool| {
        crate::version_matching::runtime_version_matches_worker(version, &tool.version)
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRecipe {
    pub tools: Vec<ToolProbe>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub workspace: bool,
    #[serde(default)]
    pub prepare: Vec<RecipeCommand>,
    pub launch: RecipeCommand,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Caller override policy participates in the operator-admitted recipe digest.
    #[serde(default)]
    pub permitted_caller_env: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolProbe {
    pub name: String,
    pub binaries: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// A bounded Rust regex with a named `version` capture.
    pub version_pattern: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeCommand {
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Pack-relative regular file; absence skips this preparation step.
    #[serde(default)]
    pub when_file_exists: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    300
}

/// Stop a still-owned Unix process group and confirm that it has disappeared.
/// Callers must retain ownership from spawn; an unknown leader is not proof of
/// teardown. Failure leaves the generation quarantined, including zombie groups.
pub async fn stop_owned_process_group(group: Option<u32>) -> Result<()> {
    #[cfg(unix)]
    {
        let group = group
            .filter(|pid| *pid > 1 && *pid <= i32::MAX as u32)
            .ok_or_else(|| invalid("Runtime process-group ownership is unconfirmed"))?;
        if unsafe { libc::kill(-(group as i32), libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error.into());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            if unsafe { libc::kill(-(group as i32), 0) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    return Ok(());
                }
                return Err(error.into());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(invalid("Runtime process group has not finished teardown"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = group;
        Err(invalid(
            "Recipe process-tree teardown is unavailable on this platform",
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedTool {
    pub executable: PathBuf,
    pub identity: PathBuf,
    pub version: String,
}

fn sandbox_tool_paths(tool: &VerifiedTool) -> (&Path, &Path) {
    (&tool.executable, &tool.identity)
}

pub struct PreparedTarget {
    pub generation: PathBuf,
    cleanup_root: PathBuf,
    pack_dir: PathBuf,
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub env: BTreeMap<String, String>,
    pub tools: BTreeMap<String, VerifiedTool>,
    pub recipe_digest: String,
    pub primary_version: String,
}

/// The private generation and launch inputs for one invocation.
struct PreparedBuild {
    generation: PathBuf,
    pack_dir: PathBuf,
    variables: BTreeMap<String, String>,
    tools: BTreeMap<String, VerifiedTool>,
    recipe_digest: String,
    primary_version: String,
}

pub enum RuntimeSource<'a> {
    File(&'a Path),
    Inline(&'a str),
}

impl PreparedTarget {
    pub fn sandbox_invocation(
        &self,
        mut command: std::process::Command,
        environment: &ChildProcessEnvironment,
    ) -> std::io::Result<std::process::Command> {
        if !environment.uses_workload_sandbox() {
            return Ok(command);
        }
        let home = self.cleanup_root.join("home");
        let scratch = self.cleanup_root.join("scratch");
        let artifacts = scratch.join("artifacts");
        std::fs::create_dir(&artifacts)?;
        command.env("ATTUNE_ARTIFACTS_DIR", artifacts);
        let inputs = [self.pack_dir.as_path(), self.generation.as_path()];
        let tools: Vec<_> = self.tools.values().map(sandbox_tool_paths).collect();
        environment.sandbox_staged_workload(command, &inputs, &[&home, &scratch], &tools)
    }

    pub fn validate_selected_version(&self, expected: &str) -> Result<()> {
        if self.primary_version == expected
            || crate::version_matching::matches_constraint(
                &self.primary_version,
                &format!("={expected}"),
            )
            .unwrap_or(false)
        {
            Ok(())
        } else {
            Err(invalid(
                "Prepared toolchain version differs from the admitted selection",
            ))
        }
    }
    /// Only call after the owning workload and its writers have been joined.
    pub async fn cleanup(self) -> Result<()> {
        tokio::fs::remove_dir_all(&self.cleanup_root).await?;
        if self.generation != self.cleanup_root {
            tokio::fs::remove_dir_all(self.generation).await?;
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

impl RuntimeRecipe {
    pub fn digest(&self) -> Result<String> {
        let bytes =
            serde_jcs::to_vec(self).map_err(|_| invalid("Cannot canonicalize runtime recipe"))?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    pub fn validate(&self) -> Result<()> {
        crate::execution_env::permitted_caller_env(
            &serde_json::json!({"permitted_caller_env":self.permitted_caller_env}),
        )
        .map_err(|_| invalid("Invalid runtime recipe caller environment declaration"))?;
        if self.tools.is_empty() || self.tools.len() > 16 {
            return Err(invalid("Runtime recipes require 1 to 16 tools"));
        }
        if self.tools.iter().filter(|tool| tool.primary).count() != 1 {
            return Err(invalid("Runtime recipes require exactly one primary tool"));
        }
        let mut names = std::collections::BTreeSet::new();
        for tool in &self.tools {
            if tool.name.is_empty()
                || !tool
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_')
                || !names.insert(&tool.name)
                || tool.binaries.is_empty()
                || tool.binaries.len() > 8
                || tool.version_pattern.len() > 4096
            {
                return Err(invalid("Invalid or duplicate runtime tool declaration"));
            }
            let regex = regex::RegexBuilder::new(&tool.version_pattern)
                .size_limit(1024 * 1024)
                .build()
                .map_err(|_| invalid("Invalid runtime version pattern"))?;
            if !regex.capture_names().any(|name| name == Some("version")) {
                return Err(invalid(
                    "Runtime version pattern needs a named version capture",
                ));
            }
        }
        if self.prepare.len() > 32 {
            return Err(invalid("Too many runtime preparation steps"));
        }
        for step in self.prepare.iter().chain(std::iter::once(&self.launch)) {
            if step.command.is_empty()
                || step.command.len() > 128
                || step.command.iter().any(|part| part.contains('\0'))
                || !(1..=3600).contains(&step.timeout_seconds)
            {
                return Err(invalid("Invalid runtime command or timeout"));
            }
            if let Some(path) = &step.when_file_exists {
                relative_path(path)?;
            }
        }
        if self.launch.when_file_exists.is_some() {
            return Err(invalid("Runtime launch cannot be conditional"));
        }
        for name in self.env.keys() {
            if name.is_empty()
                || name.to_ascii_uppercase().starts_with("ATTUNE_")
                || name.bytes().next().is_some_and(|b| b.is_ascii_digit())
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(invalid("Invalid or reserved runtime environment variable"));
            }
        }
        for extension in &self.extensions {
            if extension.is_empty()
                || !extension
                    .trim_start_matches('.')
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric())
            {
                return Err(invalid("Invalid runtime source extension"));
            }
        }
        Ok(())
    }

    pub fn require_admission(&self, environment: &ChildProcessEnvironment) -> Result<String> {
        if !cfg!(unix) {
            return Err(invalid(
                "Runtime recipes require Unix process-group supervision",
            ));
        }
        self.validate()?;
        let digest = self.digest()?;
        if !environment.admits_runtime_recipe(&digest) {
            return Err(invalid(format!(
                "Runtime recipe {digest} is not admitted by this process"
            )));
        }
        Ok(digest)
    }

    pub async fn discover(
        &self,
        environment: &ChildProcessEnvironment,
    ) -> Result<BTreeMap<String, VerifiedTool>> {
        self.require_admission(environment)?;
        // A dropped probe can still have writers awaiting termination. Keep its
        // private home unless every attempted command completed with confirmed
        // teardown; tempfile Drop must not remove an uncertain writer's files.
        let probe_home = tempfile::Builder::new()
            .prefix("attune-runtime-probe-")
            .tempdir()?
            .keep();
        let mut all_probes_confirmed = true;
        let mut verified = BTreeMap::new();
        for tool in &self.tools {
            let expression = regex::Regex::new(&tool.version_pattern)
                .map_err(|_| invalid("Invalid runtime version pattern"))?;
            let mut found = None;
            for binary in &tool.binaries {
                let Ok(executable) = resolve_executable(binary, environment) else {
                    continue;
                };
                let mut command = Command::new(&executable);
                environment.apply(command.as_std_mut());
                command
                    .args(&tool.args)
                    .current_dir(&probe_home)
                    .env("HOME", &probe_home)
                    .env("TMPDIR", &probe_home)
                    .env("XDG_CONFIG_HOME", probe_home.join("config"))
                    .env("XDG_CACHE_HOME", probe_home.join("cache"));
                let identity = executable.canonicalize()?;
                let command = Command::from(environment.sandbox_staged_workload(
                    command.into_std(),
                    &[],
                    &[&probe_home],
                    &[(&executable, &identity)],
                )?);
                let Ok(output) = collect(command, Duration::from_secs(10)).await else {
                    all_probes_confirmed = false;
                    continue;
                };
                if let Some(version) = expression
                    .captures(&output)
                    .and_then(|capture| capture.name("version"))
                {
                    found = Some(VerifiedTool {
                        identity,
                        executable,
                        version: version.as_str().to_owned(),
                    });
                    break;
                }
            }
            let Some(tool_instance) = found else {
                if all_probes_confirmed {
                    tokio::fs::remove_dir_all(&probe_home).await?;
                }
                return Err(invalid(format!(
                    "Required runtime tool {} is unavailable",
                    tool.name
                )));
            };
            verified.insert(tool.name.clone(), tool_instance);
        }
        if all_probes_confirmed {
            tokio::fs::remove_dir_all(probe_home).await?;
        }
        Ok(verified)
    }

    /// No invocation parameters, secret inputs, or action/sensor tokens enter here.
    pub async fn prepare(
        &self,
        pack_dir: &Path,
        source: RuntimeSource<'_>,
        generation_root: &Path,
        environment: &ChildProcessEnvironment,
    ) -> Result<PreparedTarget> {
        let build = self
            .prepare_build(pack_dir, source, generation_root, environment)
            .await?;
        if environment.uses_workload_sandbox() {
            return self.bind_private_invocation(&build, generation_root, environment);
        }
        // Per-invocation recipes own their entire generation and can use the
        // same home and scratch paths for both preparation and launch.
        let cleanup_root = build.generation.clone();
        self.bind_launch(&build, cleanup_root, None, environment)
    }

    async fn prepare_build(
        &self,
        pack_dir: &Path,
        source: RuntimeSource<'_>,
        generation_root: &Path,
        environment: &ChildProcessEnvironment,
    ) -> Result<PreparedBuild> {
        let recipe_digest = self.require_admission(environment)?;
        let tools = self.discover(environment).await?;
        let primary_version = self
            .tools
            .iter()
            .find(|tool| tool.primary)
            .and_then(|tool| tools.get(&tool.name))
            .map(|tool| tool.version.clone())
            .ok_or_else(|| invalid("Missing verified primary runtime tool"))?;
        let existing_pack = match pack_dir.canonicalize() {
            Ok(path) => Some(path),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && matches!(&source, RuntimeSource::Inline(_)) =>
            {
                None
            }
            Err(error) => return Err(error.into()),
        };
        std::fs::create_dir_all(generation_root)?;
        let generation_root = generation_root.canonicalize()?;
        if existing_pack
            .as_ref()
            .is_some_and(|pack| generation_root.starts_with(pack))
        {
            return Err(invalid(
                "Runtime generation storage must be outside the workload release",
            ));
        }
        let generation = tempfile::Builder::new()
            .prefix("generation-")
            .tempdir_in(&generation_root)?
            .keep();
        let pack_dir = match existing_pack {
            Some(path) => path,
            None => {
                let path = generation.join("input");
                std::fs::create_dir(&path)?;
                path
            }
        };
        let source_path = match &source {
            RuntimeSource::File(path) => {
                let path = path.canonicalize()?;
                if !path.is_file() || !path.starts_with(&pack_dir) {
                    return Err(invalid(
                        "Runtime source must be a contained regular workload file",
                    ));
                }
                Some(path)
            }
            RuntimeSource::Inline(_) => None,
        };
        // Diagnostic ownership only: builders in the trusted profile share a UID.
        // This file never grants reuse, publication, authority, or permission to
        // delete a generation without separate writer-termination evidence.
        let record = serde_json::to_vec(&serde_json::json!({
            "recipe_digest":recipe_digest,
            "workload_root":pack_dir,
            "manager_pid":std::process::id(),
            "created":chrono::Utc::now(),
            "reuse":false,
        }))
        .map_err(|_| invalid("Cannot encode runtime generation ownership"))?;
        std::fs::write(generation.join(".attune-generation.json"), record)?;
        // Keep the directory on errors/drop: publication never occurs and cleanup
        // requires positive ownership/teardown evidence, not a stale marker.
        let workspace = generation.join("workspace");
        let use_workspace = self.workspace || source_path.is_none();
        if use_workspace {
            copy_workspace(&pack_dir, &workspace)?;
        } else {
            std::fs::create_dir(&workspace)?;
        }
        let output = generation.join("output");
        let scratch = generation.join("scratch");
        let home = generation.join("home");
        for path in [&output, &scratch, &home] {
            std::fs::create_dir(path)?;
        }
        let source_file = match source {
            RuntimeSource::File(_) => {
                let path = source_path.ok_or_else(|| invalid("Missing runtime source"))?;
                if use_workspace {
                    workspace.join(
                        path.strip_prefix(&pack_dir)
                            .map_err(|_| invalid("Invalid runtime source mapping"))?,
                    )
                } else {
                    path
                }
            }
            RuntimeSource::Inline(code) => {
                let extension = self
                    .extensions
                    .first()
                    .map(|s| s.trim_start_matches('.'))
                    .unwrap_or("txt");
                let path = workspace.join(format!(
                    ".attune-inline-{}.{extension}",
                    uuid::Uuid::new_v4()
                ));
                std::fs::write(&path, code)?;
                path
            }
        };
        let mut vars = BTreeMap::from([
            ("pack_dir".into(), pack_dir.display().to_string()),
            ("workspace_dir".into(), workspace.display().to_string()),
            ("env_dir".into(), generation.display().to_string()),
            ("output_dir".into(), output.display().to_string()),
            ("scratch_dir".into(), scratch.display().to_string()),
            ("source_file".into(), source_file.display().to_string()),
        ]);
        for (name, tool) in &tools {
            vars.insert(
                format!("tool_{name}"),
                tool.executable.display().to_string(),
            );
        }
        let env = self.expand_environment(&vars, &home, &scratch)?;
        for step in &self.prepare {
            if let Some(path) = &step.when_file_exists {
                let input = pack_dir.join(relative_path(path)?);
                if !input.try_exists()? {
                    continue;
                }
                if !input.canonicalize()?.starts_with(&pack_dir) || !input.is_file() {
                    return Err(invalid("Runtime step input escapes workload release"));
                }
            }
            let argv = expand_command(&step.command, &vars)?;
            let executable = resolve_executable(&argv[0], environment)?;
            let cwd = resolve_cwd(step, &vars, &workspace, &pack_dir, &generation, None)?;
            let mut command = Command::new(executable);
            environment.apply(command.as_std_mut());
            command.args(&argv[1..]).current_dir(cwd).envs(&env);
            let tools: Vec<_> = tools.values().map(sandbox_tool_paths).collect();
            let inputs = (!pack_dir.starts_with(&generation)).then_some(pack_dir.as_path());
            let command = Command::from(environment.sandbox_staged_workload(
                command.into_std(),
                inputs.as_slice(),
                &[&generation],
                &tools,
            )?);
            collect(command, Duration::from_secs(step.timeout_seconds)).await?;
        }
        Ok(PreparedBuild {
            generation,
            pack_dir,
            variables: vars,
            tools,
            recipe_digest,
            primary_version,
        })
    }

    fn bind_private_invocation(
        &self,
        build: &PreparedBuild,
        invocation_root: &Path,
        environment: &ChildProcessEnvironment,
    ) -> Result<PreparedTarget> {
        let invocation_root = invocation_root.canonicalize()?;
        if invocation_root.starts_with(&build.generation)
            || invocation_root.starts_with(&build.pack_dir)
        {
            return Err(invalid(
                "Runtime invocation storage must be outside the build and workload release",
            ));
        }
        let invocation = tempfile::Builder::new()
            .prefix("invocation-")
            .tempdir_in(invocation_root)?;
        std::fs::create_dir(invocation.path().join("home"))?;
        std::fs::create_dir(invocation.path().join("scratch"))?;
        let target = self.bind_launch(
            build,
            invocation.path().to_path_buf(),
            Some(invocation.path()),
            environment,
        )?;
        let _ = invocation.keep();
        Ok(target)
    }

    fn expand_environment(
        &self,
        vars: &BTreeMap<String, String>,
        home: &Path,
        scratch: &Path,
    ) -> Result<BTreeMap<String, String>> {
        let mut env = BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            (
                "XDG_CONFIG_HOME".into(),
                home.join("config").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".into(),
                home.join("cache").display().to_string(),
            ),
            ("TMPDIR".into(), scratch.display().to_string()),
        ]);
        for (name, value) in &self.env {
            env.insert(name.clone(), expand(value, vars)?);
        }
        Ok(env)
    }

    fn bind_launch(
        &self,
        build: &PreparedBuild,
        cleanup_root: PathBuf,
        invocation: Option<&Path>,
        environment: &ChildProcessEnvironment,
    ) -> Result<PreparedTarget> {
        let mut vars = build.variables.clone();
        let scratch = cleanup_root.join("scratch");
        if invocation.is_some() {
            vars.insert("scratch_dir".into(), scratch.display().to_string());
        }
        let env = self.expand_environment(&vars, &cleanup_root.join("home"), &scratch)?;
        let mut argv = expand_command(&self.launch.command, &vars)?;
        let executable = resolve_executable(&argv[0], environment)?;
        let identity = executable.canonicalize()?;
        let permitted = build.tools.values().any(|tool| tool.identity == identity)
            || identity.starts_with(build.generation.canonicalize()?)
            || identity.starts_with(&build.pack_dir);
        if !permitted {
            return Err(invalid(
                "Runtime launch executable was not declared or prepared",
            ));
        }
        // Keep the venv invocation path: resolving its symlink for argv[0] would
        // select the system Python environment instead of the prepared one.
        if !Path::new(&argv[0]).is_absolute() {
            argv[0] = executable.display().to_string();
        }
        let working_dir = resolve_cwd(
            &self.launch,
            &vars,
            &build.pack_dir,
            &build.pack_dir,
            &build.generation,
            invocation,
        )?;
        Ok(PreparedTarget {
            generation: build.generation.clone(),
            cleanup_root,
            pack_dir: build.pack_dir.clone(),
            command: argv,
            working_dir,
            env,
            tools: build.tools.clone(),
            recipe_digest: build.recipe_digest.clone(),
            primary_version: build.primary_version.clone(),
        })
    }
}

fn relative_path(value: &str) -> Result<&Path> {
    let path = Path::new(value);
    if value.is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("Expected a contained runtime input path"));
    }
    Ok(path)
}

fn expand(value: &str, vars: &BTreeMap<String, String>) -> Result<String> {
    // Replace once, so source names containing template syntax cannot cause a
    // second substitution or depend on hash-map iteration order.
    let expression = regex::Regex::new(r"\{([a-z_]+)\}").expect("constant template pattern");
    let mut result = String::new();
    let mut end = 0;
    for capture in expression.captures_iter(value) {
        let whole = capture.get(0).expect("whole capture");
        result.push_str(&value[end..whole.start()]);
        if whole.start() > 0 && value.as_bytes()[whole.start() - 1] == b'$' {
            result.push_str(whole.as_str());
        } else {
            result.push_str(
                vars.get(&capture[1])
                    .ok_or_else(|| invalid("Unknown runtime template variable"))?,
            );
        }
        end = whole.end();
    }
    result.push_str(&value[end..]);
    Ok(result)
}

fn expand_command(command: &[String], vars: &BTreeMap<String, String>) -> Result<Vec<String>> {
    command.iter().map(|part| expand(part, vars)).collect()
}

fn resolve_cwd(
    step: &RecipeCommand,
    vars: &BTreeMap<String, String>,
    default: &Path,
    pack: &Path,
    generation: &Path,
    invocation: Option<&Path>,
) -> Result<PathBuf> {
    let path = match &step.cwd {
        Some(value) => PathBuf::from(expand(value, vars)?),
        None => default.to_owned(),
    };
    let path = path.canonicalize()?;
    if !path.is_dir()
        || !(path.starts_with(pack)
            || path.starts_with(generation)
            || invocation.is_some_and(|root| path.starts_with(root)))
    {
        return Err(invalid("Runtime working directory is outside owned roots"));
    }
    Ok(path)
}

pub fn resolve_executable(binary: &str, environment: &ChildProcessEnvironment) -> Result<PathBuf> {
    let path = Path::new(binary);
    let candidates = if path.is_absolute() {
        vec![path.to_owned()]
    } else if path.components().count() == 1 {
        std::env::split_paths(environment.get("PATH").unwrap_or_default())
            .filter(|path| path.is_absolute())
            .map(|path| path.join(binary))
            .collect()
    } else {
        return Err(invalid(
            "Runtime executable must be absolute or a PATH name",
        ));
    };
    for candidate in candidates {
        let Ok(metadata) = candidate.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        return Ok(candidate);
    }
    Err(invalid("Runtime executable is unavailable"))
}

fn copy_workspace(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir(destination)?;
    let mut bytes = 0u64;
    for (index, entry) in walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        if index > MAX_WORKSPACE_FILES {
            return Err(invalid("Runtime workspace file limit exceeded"));
        }
        let entry = entry.map_err(|_| invalid("Cannot enumerate runtime workspace"))?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| invalid("Invalid runtime workspace path"))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir(&target)?;
        } else if entry.file_type().is_file() {
            bytes = bytes
                .checked_add(
                    entry
                        .metadata()
                        .map_err(|_| invalid("Cannot inspect workspace input"))?
                        .len(),
                )
                .ok_or_else(|| invalid("Runtime workspace size overflow"))?;
            if bytes > MAX_WORKSPACE_BYTES {
                return Err(invalid("Runtime workspace byte limit exceeded"));
            }
            std::fs::copy(entry.path(), &target)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = target.metadata()?.permissions();
                permissions.set_mode(permissions.mode() | 0o600);
                std::fs::set_permissions(target, permissions)?;
            }
        } else {
            return Err(invalid(
                "Runtime workspace contains a symlink or special file",
            ));
        }
    }
    Ok(())
}

/// Bounded setup/probe capture. Failure diagnostics deliberately exclude child
/// output, which can contain dependency credentials or arbitrary pack content.
async fn collect(mut command: Command, deadline: Duration) -> Result<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let owned_group = child.id();
    let group = ProcessGroup(std::sync::atomic::AtomicU32::new(owned_group.unwrap_or(0)));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("Missing runtime stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| invalid("Missing runtime stderr pipe"))?;
    let readers = async { tokio::try_join!(read_bounded(stdout), read_bounded(stderr)) };
    tokio::pin!(readers);
    let mut captured = None;
    let work = async {
        let status = loop {
            tokio::select! {
                result = child.wait() => break result?,
                result = &mut readers, if captured.is_none() => captured = Some(result?),
            }
        };
        group.kill()?;
        #[cfg(unix)]
        stop_owned_process_group(owned_group).await?;
        let (out, err) = match captured.take() {
            Some(value) => value,
            None => readers.await?,
        };
        if !status.success() {
            return Err(invalid("Runtime command failed"));
        }
        Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        ))
    };
    match tokio::time::timeout(deadline, work).await {
        Ok(Ok(result)) => Ok(result),
        result => {
            group.kill()?;
            child.kill().await.ok();
            child.wait().await?;
            match result {
                Ok(Err(error)) => Err(error),
                _ => Err(invalid("Runtime command timed out")),
            }
        }
    }
}

async fn read_bounded(reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(CAPTURE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > CAPTURE_BYTES {
        return Err(invalid("Runtime command output limit exceeded"));
    }
    Ok(bytes)
}

struct ProcessGroup(std::sync::atomic::AtomicU32);
impl ProcessGroup {
    fn kill(&self) -> Result<()> {
        let pid = self.0.swap(0, std::sync::atomic::Ordering::AcqRel);
        #[cfg(not(unix))]
        let _ = pid;
        #[cfg(unix)]
        if pid != 0 && unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                self.0.store(pid, std::sync::atomic::Ordering::Release);
                return Err(error.into());
            }
        }
        Ok(())
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}
