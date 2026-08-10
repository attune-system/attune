//! Process Runtime Implementation
//!
//! A generic, configuration-driven runtime that executes actions as subprocesses.
//! Instead of having separate Rust implementations for each language (Python,
//! Node.js, etc.), this runtime reads its behavior from the database
//! `runtime.execution_config` JSONB column.
//!
//! The execution config describes:
//! - **Interpreter**: which binary to invoke and with what arguments
//! - **Environment**: how to create isolated environments (virtualenv, node_modules)
//! - **Dependencies**: how to detect and install pack dependencies
//!
//! At pack install time, the config drives environment creation and dependency
//! installation. At action execution time, it drives interpreter selection,
//! working directory, and process invocation.

use super::{
    parameter_passing::{self, ParameterDeliveryConfig},
    process_executor, ExecutionContext, ExecutionResult, Runtime, RuntimeError, RuntimeResult,
};
use async_trait::async_trait;
use attune_common::models::runtime::{
    EnvironmentConfig, InlineExecutionStrategy, RuntimeExecutionConfig,
};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

/// Per-directory locks for lazy environment setup to prevent concurrent
/// setup of the same environment from corrupting it. When two executions
/// for the same pack arrive concurrently (e.g. in agent mode), both may
/// see `!env_dir.exists()` and race to run `setup_pack_environment`.
/// This map provides a per-directory async mutex so that only one setup
/// runs at a time for each env_dir path.
static ENV_SETUP_LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

fn get_env_setup_lock(env_dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let locks = ENV_SETUP_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut map = locks.lock().unwrap();
    map.entry(env_dir.to_path_buf())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn bash_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

fn shell_identifier(key: &str) -> String {
    let mut identifier = String::with_capacity(key.len());
    for (index, ch) in key.chars().enumerate() {
        let valid = ch == '_' || ch.is_ascii_alphanumeric();
        if valid && !(index == 0 && ch.is_ascii_digit()) {
            identifier.push(ch);
        } else {
            identifier.push('_');
        }
    }

    if identifier.is_empty() {
        "_".to_string()
    } else {
        identifier
    }
}

fn format_command_for_log(cmd: &Command) -> String {
    let program = cmd.as_std().get_program().to_string_lossy().into_owned();
    let args = cmd
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let cwd = cmd
        .as_std()
        .get_current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| "<inherit>".to_string());
    let env = cmd
        .as_std()
        .get_envs()
        .map(|(key, value)| {
            let key = key.to_string_lossy().into_owned();
            let value = value
                .map(|v| {
                    if is_sensitive_env_var(&key) {
                        "<redacted>".to_string()
                    } else {
                        v.to_string_lossy().into_owned()
                    }
                })
                .unwrap_or_else(|| "<unset>".to_string());
            format!("{key}={value}")
        })
        .collect::<Vec<_>>();

    format!(
        "program={program}, args={args:?}, cwd={cwd}, env={env:?}",
        args = args,
        env = env,
    )
}

fn is_sensitive_env_var(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
        || upper.ends_with("_KEY")
        || upper == "KEY"
}

/// A generic runtime driven by `RuntimeExecutionConfig` from the database.
///
/// Each `ProcessRuntime` instance corresponds to a row in the `runtime` table.
/// The worker creates one per registered runtime at startup (loaded from DB).
pub struct ProcessRuntime {
    /// Runtime name (lowercase, used for matching in RuntimeRegistry).
    /// Corresponds to `runtime.name` lowercased (e.g., "python", "shell").
    runtime_name: String,

    /// Execution configuration parsed from `runtime.execution_config` JSONB.
    config: RuntimeExecutionConfig,

    /// Base directory where all packs are stored.
    /// Action file paths are resolved relative to this.
    packs_base_dir: PathBuf,

    /// Base directory for isolated runtime environments (virtualenvs, etc.).
    /// Environments are stored at `{runtime_envs_dir}/{pack_ref}/{runtime_name}`.
    /// This keeps the pack directory clean and read-only.
    runtime_envs_dir: PathBuf,
}

impl ProcessRuntime {
    /// Create a new ProcessRuntime from database configuration.
    ///
    /// # Arguments
    /// * `runtime_name` - Lowercase runtime name (e.g., "python", "shell", "node")
    /// * `config` - Parsed `RuntimeExecutionConfig` from the runtime table
    /// * `packs_base_dir` - Base directory for pack storage
    /// * `runtime_envs_dir` - Base directory for isolated runtime environments
    pub fn new(
        runtime_name: String,
        config: RuntimeExecutionConfig,
        packs_base_dir: PathBuf,
        runtime_envs_dir: PathBuf,
    ) -> Self {
        Self {
            runtime_name,
            config,
            packs_base_dir,
            runtime_envs_dir,
        }
    }

    /// Resolve the pack directory from an action reference.
    ///
    /// Action refs are formatted as `pack_ref.action_name`, so the pack_ref
    /// is everything before the first dot.
    #[allow(dead_code)] // Completes logical API surface; exercised in unit tests
    fn resolve_pack_dir(&self, action_ref: &str) -> PathBuf {
        let pack_ref = action_ref.split('.').next().unwrap_or(action_ref);
        self.packs_base_dir.join(pack_ref)
    }

    /// Extract the pack_ref from an action reference.
    fn extract_pack_ref<'a>(&self, action_ref: &'a str) -> &'a str {
        action_ref.split('.').next().unwrap_or(action_ref)
    }

    /// Compute the external environment directory for a pack.
    ///
    /// Returns `{runtime_envs_dir}/{pack_ref}/{runtime_name}`,
    /// e.g., `/opt/attune/runtime_envs/python_example/python`.
    fn env_dir_for_pack(&self, pack_ref: &str) -> PathBuf {
        self.runtime_envs_dir
            .join(pack_ref)
            .join(&self.runtime_name)
    }

    /// Get the interpreter path, checking for an external pack environment first.
    #[cfg(test)]
    fn resolve_interpreter(&self, pack_dir: &Path, env_dir: Option<&Path>) -> PathBuf {
        self.config.resolve_interpreter_with_env(pack_dir, env_dir)
    }

    fn interpreter_is_available(interpreter: &Path) -> bool {
        if interpreter.is_absolute() || interpreter.components().count() > 1 {
            return interpreter.exists();
        }

        env::var_os("PATH")
            .map(|paths| env::split_paths(&paths).any(|dir| dir.join(interpreter).exists()))
            .unwrap_or(false)
    }

    /// Set up the runtime environment for a pack at an external location.
    ///
    /// Environments are created at `{runtime_envs_dir}/{pack_ref}/{runtime_name}`
    /// to keep the pack directory clean and read-only.
    ///
    /// # Arguments
    /// * `pack_dir` - Absolute path to the pack directory (for manifest files)
    /// * `env_dir` - Absolute path to the environment directory to create
    pub async fn setup_pack_environment(
        &self,
        pack_dir: &Path,
        env_dir: &Path,
    ) -> RuntimeResult<()> {
        let env_cfg = match &self.config.environment {
            Some(cfg) if cfg.env_type != "none" => cfg,
            _ => {
                debug!(
                    "No environment configuration for runtime '{}', skipping setup",
                    self.runtime_name
                );
                return Ok(());
            }
        };

        let vars = self
            .config
            .build_template_vars_with_env(pack_dir, Some(env_dir));

        if !env_dir.exists() {
            // Environment does not exist yet — create it.
            self.create_environment(env_cfg, pack_dir, env_dir, &vars)
                .await?;
        } else {
            // Environment directory exists — verify the interpreter is usable.
            // A venv created by a different container may contain broken symlinks
            // (e.g. python3 -> /usr/bin/python3 when this container has it at
            // /usr/local/bin/python3).
            if self.env_needs_recreate(env_cfg, pack_dir, env_dir) {
                if let Err(e) = std::fs::remove_dir_all(env_dir) {
                    warn!(
                        "Failed to remove broken environment at {}: {}. Skipping recreate.",
                        env_dir.display(),
                        e,
                    );
                    // Still try to install dependencies even if we couldn't recreate
                    self.install_dependencies(pack_dir, env_dir).await?;
                    return Ok(());
                }

                self.create_environment(env_cfg, pack_dir, env_dir, &vars)
                    .await?;
            }
        }

        // Install dependencies if configured and manifest file exists
        self.install_dependencies(pack_dir, env_dir).await?;

        Ok(())
    }

    /// Check whether an existing environment directory has a broken or missing
    /// interpreter and needs to be recreated.
    ///
    /// Returns `true` if the environment should be deleted and recreated.
    fn env_needs_recreate(
        &self,
        env_cfg: &EnvironmentConfig,
        pack_dir: &Path,
        env_dir: &Path,
    ) -> bool {
        let interp_template = match env_cfg.interpreter_path {
            Some(ref t) => t,
            None => {
                debug!(
                    "Environment already exists at {}, skipping creation \
                     (no interpreter_path to verify)",
                    env_dir.display()
                );
                return false;
            }
        };

        let mut check_vars = std::collections::HashMap::new();
        check_vars.insert("env_dir", env_dir.to_string_lossy().to_string());
        check_vars.insert("pack_dir", pack_dir.to_string_lossy().to_string());
        let resolved = RuntimeExecutionConfig::resolve_template(interp_template, &check_vars);
        let resolved_path = std::path::PathBuf::from(&resolved);

        if resolved_path.exists() {
            debug!(
                "Environment already exists at {} with valid interpreter at {}",
                env_dir.display(),
                resolved_path.display(),
            );
            return false;
        }

        // Interpreter not reachable — distinguish broken symlinks for diagnostics
        let is_broken_symlink = std::fs::symlink_metadata(&resolved_path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        if is_broken_symlink {
            let target = std::fs::read_link(&resolved_path)
                .map(|t| t.display().to_string())
                .unwrap_or_else(|_| "<unreadable>".to_string());
            warn!(
                "Environment at {} has broken interpreter symlink: '{}' -> '{}'. \
                 Removing and recreating...",
                env_dir.display(),
                resolved_path.display(),
                target,
            );
        } else {
            warn!(
                "Environment at {} exists but interpreter not found at '{}'. \
                 Removing and recreating...",
                env_dir.display(),
                resolved_path.display(),
            );
        }
        true
    }

    /// Run the environment create_command to produce a new environment at `env_dir`.
    ///
    /// Ensures parent directories exist, resolves the create command template,
    /// executes it, and logs the result.
    async fn create_environment(
        &self,
        env_cfg: &EnvironmentConfig,
        pack_dir: &Path,
        env_dir: &Path,
        vars: &std::collections::HashMap<&str, String>,
    ) -> RuntimeResult<()> {
        if env_cfg.create_command.is_empty() {
            return Err(RuntimeError::SetupError(format!(
                "Environment type '{}' requires a create_command but none configured",
                env_cfg.env_type
            )));
        }

        // Ensure parent directories exist
        if let Some(parent) = env_dir.parent() {
            attune_common::utils::create_shared_dir_all(parent)
                .await
                .map_err(|e| {
                    RuntimeError::SetupError(format!(
                        "Failed to create environment parent directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
        }

        let resolved_cmd = RuntimeExecutionConfig::resolve_command(&env_cfg.create_command, vars);
        info!(
            "Creating {} environment at {}: {:?}",
            env_cfg.env_type,
            env_dir.display(),
            resolved_cmd
        );

        let (program, args) = resolved_cmd
            .split_first()
            .ok_or_else(|| RuntimeError::SetupError("Empty create_command".to_string()))?;

        let output = Command::new(program)
            .args(args)
            .current_dir(pack_dir)
            .output()
            .await
            .map_err(|e| {
                RuntimeError::SetupError(format!(
                    "Failed to run environment create command '{}': {}",
                    program, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::SetupError(format!(
                "Environment creation failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        info!(
            "Created {} environment at {}",
            env_cfg.env_type,
            env_dir.display()
        );

        Ok(())
    }

    /// Install dependencies for a pack if a manifest file is present.
    ///
    /// Reads the dependency configuration from `execution_config.dependencies`
    /// and runs the install command if the manifest file (e.g., requirements.txt)
    /// exists in the pack directory.
    ///
    /// # Arguments
    /// * `pack_dir` - Absolute path to the pack directory (for manifest files)
    /// * `env_dir` - Absolute path to the environment directory
    pub async fn install_dependencies(&self, pack_dir: &Path, env_dir: &Path) -> RuntimeResult<()> {
        let dep_cfg = match &self.config.dependencies {
            Some(cfg) => cfg,
            None => {
                debug!(
                    "No dependency configuration for runtime '{}', skipping",
                    self.runtime_name
                );
                return Ok(());
            }
        };

        let manifest_path = pack_dir.join(&dep_cfg.manifest_file);
        if !manifest_path.exists() {
            debug!(
                "No dependency manifest '{}' found in {}, skipping installation",
                dep_cfg.manifest_file,
                pack_dir.display()
            );
            return Ok(());
        }

        if dep_cfg.install_command.is_empty() {
            warn!(
                "Dependency manifest '{}' found but no install_command configured for runtime '{}'",
                dep_cfg.manifest_file, self.runtime_name
            );
            return Ok(());
        }

        // Check whether dependencies have already been installed for the current
        // manifest content. We store a SHA-256 checksum of the manifest file in a
        // marker file inside env_dir. If the checksum matches, we skip the
        // (potentially expensive) install command.
        let marker_path = env_dir.join(".attune_deps_installed");
        let current_checksum = Self::file_checksum(&manifest_path).await;

        if let Some(ref checksum) = current_checksum {
            if let Ok(stored) = tokio::fs::read_to_string(&marker_path).await {
                if stored.trim() == checksum.as_str() {
                    debug!(
                        "Dependencies already installed for runtime '{}' in {} (manifest unchanged)",
                        self.runtime_name,
                        env_dir.display(),
                    );
                    return Ok(());
                }
            }
        }

        // Build template vars with the external env_dir
        let vars = self
            .config
            .build_template_vars_with_env(pack_dir, Some(env_dir));
        let resolved_cmd = RuntimeExecutionConfig::resolve_command(&dep_cfg.install_command, &vars);

        info!(
            "Installing dependencies for pack at {} using: {:?}",
            pack_dir.display(),
            resolved_cmd
        );

        let (program, args) = resolved_cmd
            .split_first()
            .ok_or_else(|| RuntimeError::SetupError("Empty install_command".to_string()))?;

        let output = Command::new(program)
            .args(args)
            .current_dir(pack_dir)
            .output()
            .await
            .map_err(|e| {
                RuntimeError::SetupError(format!(
                    "Failed to run dependency install command '{}': {}",
                    program, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::SetupError(format!(
                "Dependency installation failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        info!(
            "Dependencies installed successfully for runtime '{}' in {}",
            self.runtime_name,
            env_dir.display()
        );
        debug!("Install output: {}", stdout.trim());

        // Write the checksum marker so subsequent calls skip the install.
        if let Some(checksum) = current_checksum {
            if let Err(e) = tokio::fs::write(&marker_path, checksum.as_bytes()).await {
                warn!(
                    "Failed to write dependency marker file {}: {}",
                    marker_path.display(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Compute a hex-encoded SHA-256 checksum of a file's contents.
    /// Returns `None` if the file cannot be read.
    async fn file_checksum(path: &Path) -> Option<String> {
        use sha2::{Digest, Sha256};
        let data = tokio::fs::read(path).await.ok()?;
        let hash = Sha256::digest(&data);
        Some(hash.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// Check whether a pack has dependencies that need to be installed.
    pub fn pack_has_dependencies(&self, pack_dir: &Path) -> bool {
        self.config.has_dependencies(pack_dir)
    }

    /// Check whether the environment for a pack exists at the external location.
    pub fn environment_exists(&self, pack_ref: &str) -> bool {
        let env_dir = self.env_dir_for_pack(pack_ref);
        env_dir.exists()
    }

    /// Get a reference to the execution config.
    pub fn config(&self) -> &RuntimeExecutionConfig {
        &self.config
    }

    fn build_shell_inline_wrapper(
        &self,
        merged_parameters: &HashMap<String, serde_json::Value>,
        code: &str,
    ) -> RuntimeResult<String> {
        let mut script = String::new();
        script.push_str("#!/bin/bash\n");
        script.push_str("set -e\n\n");

        script.push_str("# Action parameters\n");
        for (key, value) in merged_parameters {
            let value_str = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => serde_json::to_string(value)?,
            };
            let escaped = bash_single_quote_escape(&value_str);
            // Define shell variables for the inline action without exporting
            // them into the process environment. This keeps secrets available
            // to the current script while preventing leakage via `printenv`
            // or to child processes spawned by the action.
            let identifier = shell_identifier(key);
            script.push_str(&format!(
                "PARAM_{}='{}'\n",
                identifier.to_uppercase(),
                escaped
            ));
            script.push_str(&format!("{}='{}'\n", identifier, escaped));
        }
        script.push('\n');
        script.push_str("# Action code\n");
        script.push_str(code);

        Ok(script)
    }

    async fn materialize_inline_code(
        &self,
        execution_id: i64,
        merged_parameters: &HashMap<String, serde_json::Value>,
        code: &str,
        effective_config: &RuntimeExecutionConfig,
    ) -> RuntimeResult<(PathBuf, bool)> {
        let inline_dir = std::env::temp_dir().join("attune").join("inline_actions");
        tokio::fs::create_dir_all(&inline_dir).await.map_err(|e| {
            RuntimeError::ExecutionFailed(format!(
                "Failed to create inline action directory {}: {}",
                inline_dir.display(),
                e
            ))
        })?;

        let extension = effective_config
            .inline_execution
            .extension
            .as_deref()
            .unwrap_or("");
        let extension = if extension.is_empty() {
            String::new()
        } else if extension.starts_with('.') {
            extension.to_string()
        } else {
            format!(".{}", extension)
        };

        let inline_path = inline_dir.join(format!("exec_{}{}", execution_id, extension));
        let inline_code = if effective_config.inline_execution.inject_shell_helpers {
            self.build_shell_inline_wrapper(merged_parameters, code)?
        } else {
            code.to_string()
        };

        tokio::fs::write(&inline_path, inline_code)
            .await
            .map_err(|e| {
                RuntimeError::ExecutionFailed(format!(
                    "Failed to write inline action file {}: {}",
                    inline_path.display(),
                    e
                ))
            })?;

        Ok((
            inline_path,
            effective_config.inline_execution.inject_shell_helpers,
        ))
    }

    async fn ensure_runtime_environment(
        &self,
        action_ref: &str,
        pack_dir: &Path,
        env_dir: &Path,
        effective_config: &RuntimeExecutionConfig,
    ) {
        if effective_config.environment.is_none() || !pack_dir.exists() {
            return;
        }

        let env_lock = get_env_setup_lock(env_dir);
        let _guard = env_lock.lock().await;

        if !env_dir.exists() {
            info!(
                "Runtime environment for pack '{}' not found at {}. \
                 Creating on first use (lazy setup).",
                action_ref,
                env_dir.display(),
            );

            let setup_runtime = ProcessRuntime::new(
                self.runtime_name.clone(),
                effective_config.clone(),
                self.packs_base_dir.clone(),
                self.runtime_envs_dir.clone(),
            );
            match setup_runtime
                .setup_pack_environment(pack_dir, env_dir)
                .await
            {
                Ok(()) => {
                    info!(
                        "Successfully created environment for pack '{}' at {} (lazy setup)",
                        action_ref,
                        env_dir.display(),
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to create environment for pack '{}' at {}: {}. \
                         Proceeding with interpreter fallback logic.",
                        action_ref,
                        env_dir.display(),
                        e,
                    );
                }
            }
        }

        if env_dir.exists() {
            if let Some(ref env_cfg) = effective_config.environment {
                if let Some(ref interp_template) = env_cfg.interpreter_path {
                    let mut vars = std::collections::HashMap::new();
                    vars.insert("env_dir", env_dir.to_string_lossy().to_string());
                    vars.insert("pack_dir", pack_dir.to_string_lossy().to_string());
                    let resolved = RuntimeExecutionConfig::resolve_template(interp_template, &vars);
                    let resolved_path = std::path::PathBuf::from(&resolved);

                    let is_broken_symlink = !resolved_path.exists()
                        && std::fs::symlink_metadata(&resolved_path)
                            .map(|m| m.file_type().is_symlink())
                            .unwrap_or(false);

                    if is_broken_symlink {
                        let target = std::fs::read_link(&resolved_path)
                            .map(|t| t.display().to_string())
                            .unwrap_or_else(|_| "<unreadable>".to_string());
                        warn!(
                            "Detected broken symlink at '{}' -> '{}' in venv for pack '{}'. \
                             Removing broken environment and recreating...",
                            resolved_path.display(),
                            target,
                            action_ref,
                        );

                        if let Err(e) = std::fs::remove_dir_all(env_dir) {
                            warn!(
                                "Failed to remove broken environment at {}: {}. \
                                 Will continue to interpreter fallback logic.",
                                env_dir.display(),
                                e,
                            );
                        } else {
                            let setup_runtime = ProcessRuntime::new(
                                self.runtime_name.clone(),
                                effective_config.clone(),
                                self.packs_base_dir.clone(),
                                self.runtime_envs_dir.clone(),
                            );
                            match setup_runtime
                                .setup_pack_environment(pack_dir, env_dir)
                                .await
                            {
                                Ok(()) => {
                                    info!(
                                        "Successfully recreated environment for pack '{}' at {}",
                                        action_ref,
                                        env_dir.display(),
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to recreate environment for pack '{}' at {}: {}. \
                                         Will continue to interpreter fallback logic.",
                                        action_ref,
                                        env_dir.display(),
                                        e,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Runtime for ProcessRuntime {
    fn name(&self) -> &str {
        &self.runtime_name
    }

    fn can_execute(&self, context: &ExecutionContext) -> bool {
        // Match by runtime_name if specified in the context.
        // When an explicit runtime_name is provided, it is authoritative —
        // we only match if the name matches; we do NOT fall through to
        // extension-based matching because the caller has already decided
        // which runtime should handle this action.
        if let Some(ref name) = context.runtime_name {
            return name.eq_ignore_ascii_case(&self.runtime_name);
        }

        // No runtime_name specified — fall back to file extension matching
        if let Some(ref code_path) = context.code_path {
            if self.config.matches_file_extension(code_path) {
                return true;
            }
        }

        // Check entry_point extension
        if self
            .config
            .matches_file_extension(Path::new(&context.entry_point))
        {
            return true;
        }

        false
    }

    async fn execute(&self, context: ExecutionContext) -> RuntimeResult<ExecutionResult> {
        if let Some(ref ver) = context.selected_runtime_version {
            info!(
                "Executing action '{}' (execution_id: {}) with runtime '{}' version {}, \
                 parameter delivery: {:?}, format: {:?}, output format: {:?}",
                context.action_ref,
                context.execution_id,
                self.runtime_name,
                ver,
                context.parameter_delivery,
                context.parameter_format,
                context.output_format,
            );
        } else {
            info!(
                "Executing action '{}' (execution_id: {}) with runtime '{}', \
                 parameter delivery: {:?}, format: {:?}, output format: {:?}",
                context.action_ref,
                context.execution_id,
                self.runtime_name,
                context.parameter_delivery,
                context.parameter_format,
                context.output_format,
            );
        }

        let pack_ref = self.extract_pack_ref(&context.action_ref);
        let pack_dir = self.packs_base_dir.join(pack_ref);
        let base_env_dir = self.env_dir_for_pack(pack_ref);

        // Compute external env_dir for this pack/runtime combination.
        // When a specific runtime version is selected, the env dir includes a
        // version suffix (e.g., "python-3.12") for per-version isolation.
        // Pattern: {runtime_envs_dir}/{pack_ref}/{runtime_name[-version]}
        let mut env_dir = if let Some(ref suffix) = context.runtime_env_dir_suffix {
            self.runtime_envs_dir.join(pack_ref).join(suffix)
        } else {
            base_env_dir.clone()
        };

        let mut effective_config = context
            .runtime_config_override
            .clone()
            .unwrap_or_else(|| self.config.clone());
        let mut selected_runtime_version = context.selected_runtime_version.clone();

        self.ensure_runtime_environment(
            &context.action_ref,
            &pack_dir,
            &env_dir,
            &effective_config,
        )
        .await;

        let mut env_dir_opt = if effective_config.environment.is_some() {
            Some(env_dir.as_path())
        } else {
            None
        };
        let mut interpreter = effective_config.resolve_interpreter_with_env(&pack_dir, env_dir_opt);

        if context.runtime_config_override.is_some()
            && !Self::interpreter_is_available(&interpreter)
        {
            warn!(
                "Resolved interpreter '{}' for action '{}' using runtime version '{}' is not available on this worker. \
                 Falling back to base runtime interpreter '{}'.",
                interpreter.display(),
                context.action_ref,
                context
                    .selected_runtime_version
                    .as_deref()
                    .unwrap_or("unknown"),
                self.config.interpreter.binary,
            );

            effective_config = self.config.clone();
            selected_runtime_version = None;
            env_dir = base_env_dir;

            self.ensure_runtime_environment(
                &context.action_ref,
                &pack_dir,
                &env_dir,
                &effective_config,
            )
            .await;

            env_dir_opt = if effective_config.environment.is_some() {
                Some(env_dir.as_path())
            } else {
                None
            };
            interpreter = effective_config.resolve_interpreter_with_env(&pack_dir, env_dir_opt);
        }

        if !Self::interpreter_is_available(&interpreter) {
            return Err(RuntimeError::SetupError(format!(
                "Interpreter '{}' is not available for action '{}' on this worker",
                interpreter.display(),
                context.action_ref,
            )));
        }

        info!(
            "Resolved interpreter: {} (env_dir: {}, env_exists: {}, pack_dir: {}, version: {})",
            interpreter.display(),
            env_dir.display(),
            env_dir.exists(),
            pack_dir.display(),
            selected_runtime_version.as_deref().unwrap_or("default"),
        );

        // Prepare environment and parameters according to delivery method
        let mut env = context.env.clone();

        // Inject runtime-specific environment variables from execution_config.
        // These are template-based (e.g., NODE_PATH={env_dir}/node_modules) and
        // resolved against the current pack/env directories.
        if !effective_config.env_vars.is_empty() {
            let vars = effective_config.build_template_vars_with_env(&pack_dir, env_dir_opt);
            for (key, env_var_config) in &effective_config.env_vars {
                if parameter_passing::is_reserved_runtime_env_var(key) {
                    warn!(
                        "Ignoring runtime-configured reserved environment variable {} for action {}",
                        key, context.action_ref
                    );
                    continue;
                }
                let resolved = env_var_config.resolve(&vars, env.get(key).map(String::as_str));
                debug!("Setting runtime env var: {}={}", key, resolved);
                env.insert(key.clone(), resolved);
            }
        }
        // Merge secrets into parameters as a single JSON document.
        // Actions receive everything via one readline() on stdin.
        // Secret values are already JsonValue (string, object, array, etc.)
        // so they are inserted directly without wrapping.
        let merged_parameters =
            parameter_passing::merge_parameters_and_secrets(&context.parameters, &context.secrets);

        let param_config = ParameterDeliveryConfig {
            delivery: context.parameter_delivery,
            format: context.parameter_format,
        };
        let prepared_params =
            parameter_passing::prepare_parameters(&merged_parameters, &mut env, param_config)?;
        let mut parameters_stdin = prepared_params.stdin_content();

        // Determine working directory: use context override, or pack dir
        let working_dir = context
            .working_dir
            .as_deref()
            .filter(|p| p.exists())
            .or_else(|| {
                if pack_dir.exists() {
                    Some(pack_dir.as_path())
                } else {
                    None
                }
            });

        // Build the command based on whether we have a file or inline code
        let mut temp_inline_file: Option<PathBuf> = None;
        let cmd = if let Some(ref code_path) = context.code_path {
            // File-based execution: interpreter [args] <action_file>
            debug!("Executing file: {}", code_path.display());
            process_executor::build_action_command(
                &interpreter,
                &effective_config.interpreter.args,
                code_path,
                working_dir,
                &env,
            )
        } else if let Some(ref code) = context.code {
            match effective_config.inline_execution.strategy {
                InlineExecutionStrategy::Direct => {
                    debug!("Executing inline code directly ({} bytes)", code.len());
                    let mut cmd = process_executor::build_inline_command(&interpreter, code, &env);
                    if let Some(dir) = working_dir {
                        cmd.current_dir(dir);
                    }
                    cmd
                }
                InlineExecutionStrategy::TempFile => {
                    debug!("Executing inline code via temp file ({} bytes)", code.len());
                    let (inline_path, consumes_parameters) = self
                        .materialize_inline_code(
                            context.execution_id,
                            &merged_parameters,
                            code,
                            &effective_config,
                        )
                        .await?;
                    if consumes_parameters {
                        parameters_stdin = None;
                    }
                    temp_inline_file = Some(inline_path.clone());
                    process_executor::build_action_command(
                        &interpreter,
                        &effective_config.interpreter.args,
                        &inline_path,
                        working_dir,
                        &env,
                    )
                }
            }
        } else {
            // No code_path and no inline code — try treating entry_point as a file
            // relative to the pack's actions directory
            let action_file = pack_dir.join("actions").join(&context.entry_point);
            if action_file.exists() {
                debug!("Executing action file: {}", action_file.display());
                process_executor::build_action_command(
                    &interpreter,
                    &effective_config.interpreter.args,
                    &action_file,
                    working_dir,
                    &env,
                )
            } else {
                error!(
                    "No code, code_path, or action file found for action '{}'. \
                     Tried: {}",
                    context.action_ref,
                    action_file.display()
                );
                return Err(RuntimeError::InvalidAction(format!(
                    "No executable content found for action '{}'. \
                     Expected file at: {}",
                    context.action_ref,
                    action_file.display()
                )));
            }
        };

        // Log the spawned process accurately instead of using Command's shell-like Debug output.
        info!(
            "Running command: {} (action: '{}', execution_id: {}, working_dir: {:?})",
            format_command_for_log(&cmd),
            context.action_ref,
            context.execution_id,
            working_dir
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<none>".to_string()),
        );

        // Execute with streaming output capture (with optional cancellation support).
        // Secrets are already merged into parameters — no separate secrets arg needed.
        let result = process_executor::execute_streaming_cancellable(
            cmd,
            &HashMap::new(),
            parameters_stdin,
            context.timeout,
            context.max_stdout_bytes,
            context.max_stderr_bytes,
            context.output_format,
            context.cancel_token.clone(),
            context.stdout_log_path.as_deref(),
            context.stderr_log_path.as_deref(),
            context.stdout_log_writer,
            context.stderr_log_writer,
        )
        .await;

        if let Some(path) = temp_inline_file {
            let _ = tokio::fs::remove_file(path).await;
        }

        result
    }

    async fn setup(&self) -> RuntimeResult<()> {
        info!("Setting up ProcessRuntime '{}'", self.runtime_name);

        let binary = &self.config.interpreter.binary;

        // Verify the interpreter is available on the system
        let result = Command::new(binary).arg("--version").output().await;

        match result {
            Ok(output) => {
                if output.status.success() {
                    let version = String::from_utf8_lossy(&output.stdout);
                    let stderr_version = String::from_utf8_lossy(&output.stderr);
                    // Some interpreters print version to stderr (e.g., Python on some systems)
                    let version_str = if version.trim().is_empty() {
                        stderr_version.trim().to_string()
                    } else {
                        version.trim().to_string()
                    };
                    info!(
                        "ProcessRuntime '{}' ready: {} ({})",
                        self.runtime_name, binary, version_str
                    );
                } else {
                    warn!(
                        "Interpreter '{}' for runtime '{}' returned non-zero exit code \
                         on --version check (may still work for execution)",
                        binary, self.runtime_name
                    );
                }
            }
            Err(e) => {
                // The interpreter isn't available — this is a warning, not a hard failure,
                // because the runtime might only be used in containers where the interpreter
                // is available at execution time.
                warn!(
                    "Interpreter '{}' for runtime '{}' not found: {}. \
                     Actions using this runtime may fail.",
                    binary, self.runtime_name, e
                );
            }
        }

        Ok(())
    }

    async fn cleanup(&self) -> RuntimeResult<()> {
        info!("Cleaning up ProcessRuntime '{}'", self.runtime_name);
        Ok(())
    }

    async fn validate(&self) -> RuntimeResult<()> {
        debug!("Validating ProcessRuntime '{}'", self.runtime_name);

        let binary = &self.config.interpreter.binary;

        // Check if interpreter is available
        let output = Command::new(binary).arg("--version").output().await;

        match output {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                // Non-zero exit but binary exists — warn but don't fail
                warn!(
                    "Interpreter '{}' returned exit code {} on validation",
                    binary,
                    output.status.code().unwrap_or(-1)
                );
                Ok(())
            }
            Err(e) => {
                warn!(
                    "Interpreter '{}' for runtime '{}' not available: {}",
                    binary, self.runtime_name, e
                );
                // Don't fail validation — the interpreter might be available in containers
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::models::runtime::{
        DependencyConfig, EnvironmentConfig, InlineExecutionConfig, InlineExecutionStrategy,
        InterpreterConfig, RuntimeEnvVarConfig, RuntimeEnvVarOperation, RuntimeEnvVarSpec,
        RuntimeExecutionConfig,
    };
    use attune_common::models::{OutputFormat, ParameterDelivery, ParameterFormat};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_shell_config() -> RuntimeExecutionConfig {
        RuntimeExecutionConfig {
            interpreter: InterpreterConfig {
                binary: "/bin/bash".to_string(),
                args: vec![],
                file_extension: Some(".sh".to_string()),
            },
            inline_execution: InlineExecutionConfig {
                strategy: InlineExecutionStrategy::TempFile,
                extension: Some(".sh".to_string()),
                inject_shell_helpers: true,
            },
            environment: None,
            dependencies: None,
            env_vars: HashMap::new(),
        }
    }

    fn make_python_config() -> RuntimeExecutionConfig {
        RuntimeExecutionConfig {
            interpreter: InterpreterConfig {
                binary: "python3".to_string(),
                args: vec!["-u".to_string()],
                file_extension: Some(".py".to_string()),
            },
            inline_execution: InlineExecutionConfig::default(),
            environment: Some(EnvironmentConfig {
                env_type: "virtualenv".to_string(),
                dir_name: ".venv".to_string(),
                create_command: vec![
                    "python3".to_string(),
                    "-m".to_string(),
                    "venv".to_string(),
                    "{env_dir}".to_string(),
                ],
                interpreter_path: Some("{env_dir}/bin/python3".to_string()),
            }),
            dependencies: Some(DependencyConfig {
                manifest_file: "requirements.txt".to_string(),
                install_command: vec![
                    "{interpreter}".to_string(),
                    "-m".to_string(),
                    "pip".to_string(),
                    "install".to_string(),
                    "-r".to_string(),
                    "{manifest_path}".to_string(),
                ],
            }),
            env_vars: HashMap::new(),
        }
    }

    #[test]
    fn test_can_execute_by_runtime_name() {
        let runtime = ProcessRuntime::new(
            "python".to_string(),
            make_python_config(),
            PathBuf::from("/tmp/packs"),
            PathBuf::from("/tmp/runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 1,
            action_ref: "mypack.hello".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "hello.py".to_string(),
            code: None,
            code_path: None,
            runtime_name: Some("python".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        assert!(runtime.can_execute(&context));
    }

    #[test]
    fn test_can_execute_by_file_extension() {
        let runtime = ProcessRuntime::new(
            "python".to_string(),
            make_python_config(),
            PathBuf::from("/tmp/packs"),
            PathBuf::from("/tmp/runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 1,
            action_ref: "mypack.hello".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "hello.py".to_string(),
            code: None,
            code_path: Some(PathBuf::from("/tmp/packs/mypack/actions/hello.py")),
            runtime_name: None,
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        assert!(runtime.can_execute(&context));
    }

    #[test]
    fn test_cannot_execute_wrong_extension() {
        let runtime = ProcessRuntime::new(
            "python".to_string(),
            make_python_config(),
            PathBuf::from("/tmp/packs"),
            PathBuf::from("/tmp/runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 1,
            action_ref: "mypack.hello".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "hello.sh".to_string(),
            code: None,
            code_path: Some(PathBuf::from("/tmp/packs/mypack/actions/hello.sh")),
            runtime_name: None,
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        assert!(!runtime.can_execute(&context));
    }

    #[test]
    fn test_resolve_pack_dir() {
        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            PathBuf::from("/opt/attune/packs"),
            PathBuf::from("/opt/attune/runtime_envs"),
        );

        let pack_dir = runtime.resolve_pack_dir("mypack.echo");
        assert_eq!(pack_dir, PathBuf::from("/opt/attune/packs/mypack"));
    }

    #[test]
    fn test_resolve_interpreter_no_env() {
        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            PathBuf::from("/tmp/packs"),
            PathBuf::from("/tmp/runtime_envs"),
        );

        let interpreter = runtime.resolve_interpreter(Path::new("/tmp/packs/mypack"), None);
        assert_eq!(interpreter, PathBuf::from("/bin/bash"));
    }

    #[test]
    fn test_env_dir_for_pack() {
        let runtime = ProcessRuntime::new(
            "python".to_string(),
            make_python_config(),
            PathBuf::from("/opt/attune/packs"),
            PathBuf::from("/opt/attune/runtime_envs"),
        );

        let env_dir = runtime.env_dir_for_pack("python_example");
        assert_eq!(
            env_dir,
            PathBuf::from("/opt/attune/runtime_envs/python_example/python")
        );
    }

    #[tokio::test]
    async fn test_execute_shell_file() {
        let temp_dir = TempDir::new().unwrap();
        let packs_dir = temp_dir.path().join("packs");
        let pack_dir = packs_dir.join("testpack");
        let actions_dir = pack_dir.join("actions");
        std::fs::create_dir_all(&actions_dir).unwrap();

        // Write a simple shell script
        let script_path = actions_dir.join("hello.sh");
        std::fs::write(
            &script_path,
            "#!/bin/bash\necho 'hello from process runtime'",
        )
        .unwrap();

        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            packs_dir,
            temp_dir.path().join("runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 1,
            action_ref: "testpack.hello".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "hello.sh".to_string(),
            code: None,
            code_path: Some(script_path),
            runtime_name: Some("shell".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello from process runtime"));
    }

    #[tokio::test]
    async fn test_execute_python_file() {
        let temp_dir = TempDir::new().unwrap();
        let packs_dir = temp_dir.path().join("packs");
        let pack_dir = packs_dir.join("testpack");
        let actions_dir = pack_dir.join("actions");
        std::fs::create_dir_all(&actions_dir).unwrap();

        // Write a simple Python script
        let script_path = actions_dir.join("hello.py");
        std::fs::write(&script_path, "print('hello from python process runtime')").unwrap();

        let config = RuntimeExecutionConfig {
            interpreter: InterpreterConfig {
                binary: "python3".to_string(),
                args: vec![],
                file_extension: Some(".py".to_string()),
            },
            inline_execution: InlineExecutionConfig::default(),
            environment: None,
            dependencies: None,
            env_vars: HashMap::new(),
        };

        let runtime = ProcessRuntime::new(
            "python".to_string(),
            config,
            packs_dir,
            temp_dir.path().join("runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 2,
            action_ref: "testpack.hello".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "hello.py".to_string(),
            code: None,
            code_path: Some(script_path),
            runtime_name: Some("python".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello from python process runtime"));
    }

    #[tokio::test]
    async fn test_execute_falls_back_from_unavailable_version_override() {
        let temp_dir = TempDir::new().unwrap();
        let packs_dir = temp_dir.path().join("packs");
        let pack_dir = packs_dir.join("testpack");
        let actions_dir = pack_dir.join("actions");
        std::fs::create_dir_all(&actions_dir).unwrap();

        let script_path = actions_dir.join("hello.py");
        std::fs::write(&script_path, "print('hello from base runtime fallback')").unwrap();

        let base_config = RuntimeExecutionConfig {
            interpreter: InterpreterConfig {
                binary: "python3".to_string(),
                args: vec![],
                file_extension: Some(".py".to_string()),
            },
            inline_execution: InlineExecutionConfig::default(),
            environment: None,
            dependencies: None,
            env_vars: HashMap::new(),
        };

        let override_config = RuntimeExecutionConfig {
            interpreter: InterpreterConfig {
                binary: "__missing_python3_13__".to_string(),
                args: vec![],
                file_extension: Some(".py".to_string()),
            },
            inline_execution: InlineExecutionConfig::default(),
            environment: Some(EnvironmentConfig {
                env_type: "virtualenv".to_string(),
                dir_name: ".venv".to_string(),
                create_command: vec![
                    "__missing_python3_13__".to_string(),
                    "-m".to_string(),
                    "venv".to_string(),
                    "{env_dir}".to_string(),
                ],
                interpreter_path: Some("{env_dir}/bin/__missing_python3_13__".to_string()),
            }),
            dependencies: None,
            env_vars: HashMap::new(),
        };

        let runtime = ProcessRuntime::new(
            "python".to_string(),
            base_config,
            packs_dir,
            temp_dir.path().join("runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 22,
            action_ref: "testpack.hello".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "hello.py".to_string(),
            code: None,
            code_path: Some(script_path),
            runtime_name: Some("python".to_string()),
            runtime_config_override: Some(override_config),
            runtime_env_dir_suffix: Some("python-3.13".to_string()),
            selected_runtime_version: Some("3.13".to_string()),
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello from base runtime fallback"));
    }

    #[tokio::test]
    async fn test_execute_python_file_with_pack_lib_on_pythonpath() {
        let temp_dir = TempDir::new().unwrap();
        let packs_dir = temp_dir.path().join("packs");
        let pack_dir = packs_dir.join("testpack");
        let actions_dir = pack_dir.join("actions");
        let lib_dir = pack_dir.join("lib");
        std::fs::create_dir_all(&actions_dir).unwrap();
        std::fs::create_dir_all(&lib_dir).unwrap();

        std::fs::write(
            lib_dir.join("helper.py"),
            "def message():\n    return 'hello from pack lib'\n",
        )
        .unwrap();
        std::fs::write(
            actions_dir.join("hello.py"),
            "import helper\nimport os\nprint(helper.message())\nprint(os.environ['PYTHONPATH'])\n",
        )
        .unwrap();

        let mut env_vars = HashMap::new();
        env_vars.insert(
            "PYTHONPATH".to_string(),
            RuntimeEnvVarConfig::Spec(RuntimeEnvVarSpec {
                value: "{pack_dir}/lib".to_string(),
                operation: RuntimeEnvVarOperation::Prepend,
                separator: ":".to_string(),
            }),
        );

        let runtime = ProcessRuntime::new(
            "python".to_string(),
            RuntimeExecutionConfig {
                interpreter: InterpreterConfig {
                    binary: "python3".to_string(),
                    args: vec![],
                    file_extension: Some(".py".to_string()),
                },
                inline_execution: InlineExecutionConfig::default(),
                environment: None,
                dependencies: None,
                env_vars,
            },
            packs_dir,
            temp_dir.path().join("runtime_envs"),
        );

        let mut env = HashMap::new();
        env.insert("PYTHONPATH".to_string(), "/existing/pythonpath".to_string());

        let context = ExecutionContext {
            execution_id: 3,
            action_ref: "testpack.hello".to_string(),
            parameters: HashMap::new(),
            env,
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "hello.py".to_string(),
            code: None,
            code_path: Some(actions_dir.join("hello.py")),
            runtime_name: Some("python".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello from pack lib"));
        assert!(result
            .stdout
            .contains(&format!("{}/lib:/existing/pythonpath", pack_dir.display())));
    }

    #[tokio::test]
    async fn test_execute_inline_code() {
        let temp_dir = TempDir::new().unwrap();

        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            temp_dir.path().to_path_buf(),
            temp_dir.path().join("runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 3,
            action_ref: "adhoc.test".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "inline".to_string(),
            code: Some("echo 'inline shell code'".to_string()),
            code_path: None,
            runtime_name: Some("shell".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("inline shell code"));
    }

    #[tokio::test]
    async fn test_execute_inline_code_with_merged_inputs() {
        let temp_dir = TempDir::new().unwrap();

        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            temp_dir.path().to_path_buf(),
            temp_dir.path().join("runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 30,
            action_ref: "adhoc.test_inputs".to_string(),
            parameters: {
                let mut map = HashMap::new();
                map.insert("name".to_string(), serde_json::json!("Alice"));
                map.insert(
                    "test.api_url".to_string(),
                    serde_json::json!("https://api.example.com/v1"),
                );
                map
            },
            env: HashMap::new(),
            secrets: {
                let mut map = HashMap::new();
                map.insert("api_key".to_string(), serde_json::json!("secret-123"));
                map
            },
            timeout: Some(10),
            working_dir: None,
            entry_point: "inline".to_string(),
            code: Some(
                "echo \"$name/$api_key/$PARAM_NAME/$PARAM_API_KEY/$test_api_url/$PARAM_TEST_API_URL\""
                    .to_string(),
            ),
            code_path: None,
            runtime_name: Some("shell".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains(
            "Alice/secret-123/Alice/secret-123/https://api.example.com/v1/https://api.example.com/v1"
        ));
    }

    #[tokio::test]
    async fn test_execute_entry_point_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let packs_dir = temp_dir.path().join("packs");
        let pack_dir = packs_dir.join("testpack");
        let actions_dir = pack_dir.join("actions");
        std::fs::create_dir_all(&actions_dir).unwrap();

        // Write a script at the expected path
        std::fs::write(
            actions_dir.join("greet.sh"),
            "#!/bin/bash\necho 'found via entry_point'",
        )
        .unwrap();

        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            packs_dir,
            temp_dir.path().join("runtime_envs"),
        );

        // No code_path, no code — should resolve via entry_point
        let context = ExecutionContext {
            execution_id: 4,
            action_ref: "testpack.greet".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "greet.sh".to_string(),
            code: None,
            code_path: None,
            runtime_name: Some("shell".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("found via entry_point"));
    }

    #[tokio::test]
    async fn test_setup_pack_environment_no_config() {
        let temp_dir = TempDir::new().unwrap();
        let pack_dir = temp_dir.path().join("testpack");
        let env_dir = temp_dir
            .path()
            .join("runtime_envs")
            .join("testpack")
            .join("shell");
        std::fs::create_dir_all(&pack_dir).unwrap();

        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            temp_dir.path().to_path_buf(),
            temp_dir.path().join("runtime_envs"),
        );

        // Should succeed immediately (no environment to create)
        runtime
            .setup_pack_environment(&pack_dir, &env_dir)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_pack_has_dependencies() {
        let temp_dir = TempDir::new().unwrap();
        let pack_dir = temp_dir.path().join("testpack");
        std::fs::create_dir_all(&pack_dir).unwrap();

        let runtime = ProcessRuntime::new(
            "python".to_string(),
            make_python_config(),
            temp_dir.path().to_path_buf(),
            temp_dir.path().join("runtime_envs"),
        );

        // No requirements.txt yet
        assert!(!runtime.pack_has_dependencies(&pack_dir));

        // Create requirements.txt
        std::fs::write(pack_dir.join("requirements.txt"), "requests>=2.28.0\n").unwrap();
        assert!(runtime.pack_has_dependencies(&pack_dir));
    }

    #[tokio::test]
    async fn test_setup_and_validate() {
        let temp_dir = TempDir::new().unwrap();

        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            temp_dir.path().to_path_buf(),
            temp_dir.path().join("runtime_envs"),
        );

        // Setup and validate should succeed for shell (bash is always available)
        runtime.setup().await.unwrap();
        runtime.validate().await.unwrap();
    }

    #[tokio::test]
    async fn test_working_dir_set_to_pack_dir() {
        let temp_dir = TempDir::new().unwrap();
        let packs_dir = temp_dir.path().join("packs");
        let pack_dir = packs_dir.join("testpack");
        let actions_dir = pack_dir.join("actions");
        std::fs::create_dir_all(&actions_dir).unwrap();

        // Write a script that prints the working directory
        let script_path = actions_dir.join("pwd.sh");
        std::fs::write(&script_path, "#!/bin/bash\npwd").unwrap();

        let runtime = ProcessRuntime::new(
            "shell".to_string(),
            make_shell_config(),
            packs_dir,
            temp_dir.path().join("runtime_envs"),
        );

        let context = ExecutionContext {
            execution_id: 5,
            action_ref: "testpack.pwd".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "pwd.sh".to_string(),
            code: None,
            code_path: Some(script_path),
            runtime_name: Some("shell".to_string()),
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        };

        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        // Working dir should be the pack dir
        let output_path = result.stdout.trim();
        let actual_dir = std::fs::canonicalize(output_path).unwrap();
        let expected_dir = std::fs::canonicalize(&pack_dir).unwrap();
        assert_eq!(
            actual_dir, expected_dir,
            "Working directory should be set to the pack directory"
        );
    }
}
