//! Runtime Module
//!
//! Provides runtime abstraction and implementations for executing actions
//! in different environments. The primary runtime is `ProcessRuntime`, a
//! generic, configuration-driven runtime that reads its behavior from the
//! database `runtime.execution_config` JSONB column.
//!
//! Language-specific runtimes (Python, Node.js, etc.) are NOT implemented
//! as separate Rust types. Instead, the `ProcessRuntime` handles all
//! languages by using the interpreter, environment, and dependency
//! configuration stored in the database.
//!
//! ## Runtime Version Selection
//!
//! When an action declares a `runtime_version_constraint` (e.g., `">=3.12"`),
//! the executor resolves the best matching `RuntimeVersion` from the database
//! and passes its `execution_config` through `ExecutionContext::runtime_config_override`.
//! The `ProcessRuntime` uses this override instead of its built-in config,
//! enabling version-specific interpreter binaries, environment commands, etc.
//!
//! The environment directory is also overridden to include the version suffix
//! (e.g., `python-3.12` instead of `python`) so that different versions
//! maintain isolated environments.

pub mod dependency;
pub mod local;
pub mod log_writer;
pub mod native;
pub mod parameter_passing;
pub mod process;
pub mod process_executor;

// Re-export runtime implementations
pub use local::LocalRuntime;
pub use native::NativeRuntime;
pub use process::ProcessRuntime;

use async_trait::async_trait;
use attune_common::models::runtime::RuntimeExecutionConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

// Re-export dependency management types
pub use dependency::{
    DependencyError, DependencyManager, DependencyManagerRegistry, DependencyResult,
    DependencySpec, EnvironmentInfo,
};
pub use log_writer::{BoundedLogFileWriter, BoundedLogResult, BoundedLogWriter};
pub use parameter_passing::{ParameterDeliveryConfig, PreparedParameters};

// Re-export parameter types from common
pub use attune_common::models::{OutputFormat, ParameterDelivery, ParameterFormat};

/// Runtime execution result
pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

/// Runtime execution errors
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout after {0} seconds")]
    Timeout(u64),

    #[error("Runtime not found: {0}")]
    RuntimeNotFound(String),

    #[error("Invalid action: {0}")]
    InvalidAction(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Process error: {0}")]
    ProcessError(String),

    #[error("Setup error: {0}")]
    SetupError(String),

    #[error("Cleanup error: {0}")]
    CleanupError(String),
}

/// Action execution context
pub struct ExecutionContext {
    /// Execution ID
    pub execution_id: i64,

    /// Action reference (pack.action)
    pub action_ref: String,

    /// Action parameters
    pub parameters: HashMap<String, serde_json::Value>,

    /// Environment variables
    pub env: HashMap<String, String>,

    /// Secrets (passed securely via stdin, not environment variables).
    /// Values are JSON — strings, objects, arrays, numbers, or booleans.
    pub secrets: HashMap<String, serde_json::Value>,

    /// Execution timeout in seconds
    pub timeout: Option<u64>,

    /// Working directory
    pub working_dir: Option<PathBuf>,

    /// Action entry point (script, function, etc.)
    pub entry_point: String,

    /// Action code/script content
    pub code: Option<String>,

    /// Action code file path (alternative to code)
    pub code_path: Option<PathBuf>,

    /// Runtime name (python, shell, etc.) - used to select the correct runtime
    pub runtime_name: Option<String>,

    /// Optional override of the runtime's execution config, set when a specific
    /// runtime version has been selected (e.g., Python 3.12 vs the parent
    /// "Python" runtime). When present, `ProcessRuntime` uses this config
    /// instead of its built-in one for interpreter resolution, environment
    /// setup, and dependency management.
    pub runtime_config_override: Option<RuntimeExecutionConfig>,

    /// Optional override of the environment directory suffix. When a specific
    /// runtime version is selected, the env dir includes the version
    /// (e.g., `python-3.12` instead of `python`) for per-version isolation.
    /// Format: just the directory name, not the full path.
    pub runtime_env_dir_suffix: Option<String>,

    /// The selected runtime version string for logging/diagnostics
    /// (e.g., "3.12.1"). `None` means the parent runtime config is used as-is.
    pub selected_runtime_version: Option<String>,

    /// Maximum stdout size in bytes (for log truncation)
    pub max_stdout_bytes: usize,

    /// Maximum stderr size in bytes (for log truncation)
    pub max_stderr_bytes: usize,

    /// Optional live stdout log path for incremental writes during execution.
    pub stdout_log_path: Option<PathBuf>,

    /// Optional live stderr log path for incremental writes during execution.
    pub stderr_log_path: Option<PathBuf>,

    /// Optional pre-opened log writers backed by the artifact transport.
    /// When set, these take priority over `stdout_log_path`/`stderr_log_path`.
    pub stdout_log_writer: Option<BoundedLogFileWriter>,
    pub stderr_log_writer: Option<BoundedLogFileWriter>,

    /// How parameters should be delivered to the action
    pub parameter_delivery: ParameterDelivery,

    /// Format for parameter serialization
    pub parameter_format: ParameterFormat,

    /// Format for output parsing
    pub output_format: OutputFormat,

    /// Optional cancellation token for process termination.
    /// When triggered, the executor sends SIGTERM → SIGKILL
    /// with a short grace period.
    pub cancel_token: Option<CancellationToken>,
}

impl std::fmt::Debug for ExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionContext")
            .field("execution_id", &self.execution_id)
            .field("action_ref", &self.action_ref)
            .field("entry_point", &self.entry_point)
            .field("runtime_name", &self.runtime_name)
            .field("stdout_log_path", &self.stdout_log_path)
            .field("stderr_log_path", &self.stderr_log_path)
            .field(
                "stdout_log_writer",
                &self.stdout_log_writer.as_ref().map(|_| "..."),
            )
            .field(
                "stderr_log_writer",
                &self.stderr_log_writer.as_ref().map(|_| "..."),
            )
            .finish_non_exhaustive()
    }
}

impl ExecutionContext {
    /// Create a test context with default values (for tests)
    #[cfg(test)]
    pub fn test_context(action_ref: String, code: Option<String>) -> Self {
        use std::collections::HashMap;
        Self {
            execution_id: 1,
            action_ref,
            parameters: HashMap::new(),
            env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "run".to_string(),
            code,
            code_path: None,
            runtime_name: None,
            runtime_config_override: None,
            runtime_env_dir_suffix: None,
            selected_runtime_version: None,
            max_stdout_bytes: 10 * 1024 * 1024,
            max_stderr_bytes: 10 * 1024 * 1024,
            stdout_log_path: None,
            stderr_log_path: None,
            stdout_log_writer: None,
            stderr_log_writer: None,
            parameter_delivery: ParameterDelivery::default(),
            parameter_format: ParameterFormat::default(),
            output_format: OutputFormat::default(),
            cancel_token: None,
        }
    }
}

/// Action execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Exit code (0 = success)
    pub exit_code: i32,

    /// Standard output
    pub stdout: String,

    /// Standard error
    pub stderr: String,

    /// Execution result data (parsed from stdout or returned by action)
    pub result: Option<serde_json::Value>,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Error message if execution failed
    pub error: Option<String>,

    /// Whether stdout was truncated due to size limits
    #[serde(default)]
    pub stdout_truncated: bool,

    /// Whether stderr was truncated due to size limits
    #[serde(default)]
    pub stderr_truncated: bool,

    /// Number of bytes truncated from stdout (0 if not truncated)
    #[serde(default)]
    pub stdout_bytes_truncated: usize,

    /// Number of bytes truncated from stderr (0 if not truncated)
    #[serde(default)]
    pub stderr_bytes_truncated: usize,

    /// Whether the execution was terminated because it exceeded its timeout.
    #[serde(default)]
    pub timed_out: bool,
}

impl ExecutionResult {
    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && self.error.is_none()
    }

    /// Create a success result
    pub fn success(stdout: String, result: Option<serde_json::Value>, duration_ms: u64) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr: String::new(),
            result,
            duration_ms,
            error: None,
            stdout_truncated: false,
            stderr_truncated: false,
            stdout_bytes_truncated: 0,
            stderr_bytes_truncated: 0,
            timed_out: false,
        }
    }

    /// Create a failure result
    pub fn failure(exit_code: i32, stderr: String, error: String, duration_ms: u64) -> Self {
        Self {
            exit_code,
            stdout: String::new(),
            stderr,
            result: None,
            duration_ms,
            error: Some(error),
            stdout_truncated: false,
            stderr_truncated: false,
            stdout_bytes_truncated: 0,
            stderr_bytes_truncated: 0,
            timed_out: false,
        }
    }
}

/// Runtime trait for executing actions
#[async_trait]
pub trait Runtime: Send + Sync {
    /// Get the runtime name
    fn name(&self) -> &str;

    /// Check if this runtime can execute the given action
    fn can_execute(&self, context: &ExecutionContext) -> bool;

    /// Execute an action
    async fn execute(&self, context: ExecutionContext) -> RuntimeResult<ExecutionResult>;

    /// Setup the runtime environment (called once on worker startup)
    async fn setup(&self) -> RuntimeResult<()> {
        Ok(())
    }

    /// Cleanup the runtime environment (called on worker shutdown)
    async fn cleanup(&self) -> RuntimeResult<()> {
        Ok(())
    }

    /// Validate the runtime is properly configured
    async fn validate(&self) -> RuntimeResult<()> {
        Ok(())
    }
}

/// Runtime registry for managing multiple runtime implementations
pub struct RuntimeRegistry {
    runtimes: Vec<Box<dyn Runtime>>,
}

impl RuntimeRegistry {
    /// Create a new runtime registry
    pub fn new() -> Self {
        Self {
            runtimes: Vec::new(),
        }
    }

    /// Register a runtime
    pub fn register(&mut self, runtime: Box<dyn Runtime>) {
        self.runtimes.push(runtime);
    }

    /// Get a runtime that can execute the given context
    pub fn get_runtime(&self, context: &ExecutionContext) -> RuntimeResult<&dyn Runtime> {
        // If runtime_name is specified, use it to select the runtime directly
        if let Some(ref runtime_name) = context.runtime_name {
            return self
                .runtimes
                .iter()
                .find(|r| r.name() == runtime_name)
                .map(|r| r.as_ref())
                .ok_or_else(|| {
                    RuntimeError::RuntimeNotFound(format!(
                        "Runtime '{}' not found for action: {} (available: {})",
                        runtime_name,
                        context.action_ref,
                        self.list_runtimes().join(", ")
                    ))
                });
        }

        // Otherwise, fall back to can_execute check
        self.runtimes
            .iter()
            .find(|r| r.can_execute(context))
            .map(|r| r.as_ref())
            .ok_or_else(|| {
                RuntimeError::RuntimeNotFound(format!(
                    "No runtime found for action: {} (available: {})",
                    context.action_ref,
                    self.list_runtimes().join(", ")
                ))
            })
    }

    /// Setup all registered runtimes
    pub async fn setup_all(&self) -> RuntimeResult<()> {
        for runtime in &self.runtimes {
            runtime.setup().await?;
        }
        Ok(())
    }

    /// Cleanup all registered runtimes
    pub async fn cleanup_all(&self) -> RuntimeResult<()> {
        for runtime in &self.runtimes {
            runtime.cleanup().await?;
        }
        Ok(())
    }

    /// Validate all registered runtimes
    pub async fn validate_all(&self) -> RuntimeResult<()> {
        for runtime in &self.runtimes {
            runtime.validate().await?;
        }
        Ok(())
    }

    /// List all registered runtimes
    pub fn list_runtimes(&self) -> Vec<&str> {
        self.runtimes.iter().map(|r| r.name()).collect()
    }
}

impl Default for RuntimeRegistry {
    fn default() -> Self {
        Self::new()
    }
}
