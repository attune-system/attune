//! Local Runtime Module
//!
//! Provides local execution capabilities by combining Process and Native runtimes.
//! This module serves as a facade for all local process-based execution.
//!
//! The `ProcessRuntime` is used for Python (and other interpreted languages),
//! driven by `RuntimeExecutionConfig` rather than language-specific Rust code.

use super::native::NativeRuntime;
use super::process::ProcessRuntime;
use super::{ExecutionContext, ExecutionResult, Runtime, RuntimeError, RuntimeResult};
use async_trait::async_trait;
use attune_common::models::runtime::{
    InlineExecutionConfig, InlineExecutionStrategy, InterpreterConfig, RuntimeExecutionConfig,
};
use std::path::PathBuf;
use tracing::{debug, info};

/// Local runtime that delegates to Process, Shell, or Native based on action type
pub struct LocalRuntime {
    native: NativeRuntime,
    python: ProcessRuntime,
    shell: ProcessRuntime,
}

impl LocalRuntime {
    /// Create a new local runtime with default settings.
    ///
    /// Uses a default Python `RuntimeExecutionConfig` for the process runtime,
    /// since this is a fallback when runtimes haven't been loaded from the database.
    pub fn new() -> Self {
        let python_config = RuntimeExecutionConfig {
            interpreter: InterpreterConfig {
                binary: "python3".to_string(),
                args: vec![],
                file_extension: Some(".py".to_string()),
            },
            inline_execution: InlineExecutionConfig::default(),
            environment: None,
            dependencies: None,
            env_vars: std::collections::HashMap::new(),
        };

        let shell_config = RuntimeExecutionConfig {
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
            env_vars: std::collections::HashMap::new(),
        };

        Self {
            native: NativeRuntime::new(),
            python: ProcessRuntime::new(
                "python".to_string(),
                python_config,
                PathBuf::from("/opt/attune/packs"),
                PathBuf::from("/opt/attune/runtime_envs"),
            ),
            shell: ProcessRuntime::new(
                "shell".to_string(),
                shell_config,
                PathBuf::from("/opt/attune/packs"),
                PathBuf::from("/opt/attune/runtime_envs"),
            ),
        }
    }

    /// Create a local runtime with custom runtimes
    pub fn with_runtimes(
        native: NativeRuntime,
        python: ProcessRuntime,
        shell: ProcessRuntime,
    ) -> Self {
        Self {
            native,
            python,
            shell,
        }
    }

    pub fn with_child_environment(
        mut self,
        environment: attune_common::child_process_environment::ChildProcessEnvironment,
    ) -> Self {
        self.native = self.native.with_child_environment(environment.clone());
        self.python = self.python.with_child_environment(environment.clone());
        self.shell = self.shell.with_child_environment(environment);
        self
    }

    /// Get the appropriate runtime for the given context
    fn select_runtime(&self, context: &ExecutionContext) -> RuntimeResult<&dyn Runtime> {
        if self.native.can_execute(context) {
            debug!("Selected Native runtime for action: {}", context.action_ref);
            Ok(&self.native)
        } else if self.python.can_execute(context) {
            debug!(
                "Selected Python (ProcessRuntime) for action: {}",
                context.action_ref
            );
            Ok(&self.python)
        } else if self.shell.can_execute(context) {
            debug!(
                "Selected Shell (ProcessRuntime) for action: {}",
                context.action_ref
            );
            Ok(&self.shell)
        } else {
            Err(RuntimeError::RuntimeNotFound(format!(
                "No suitable local runtime found for action: {}",
                context.action_ref
            )))
        }
    }
}

impl Default for LocalRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runtime for LocalRuntime {
    fn name(&self) -> &str {
        "local"
    }

    fn can_execute(&self, context: &ExecutionContext) -> bool {
        self.native.can_execute(context)
            || self.python.can_execute(context)
            || self.shell.can_execute(context)
    }

    async fn execute(&self, context: ExecutionContext) -> RuntimeResult<ExecutionResult> {
        info!(
            "Executing local action: {} (execution_id: {})",
            context.action_ref, context.execution_id
        );

        let runtime = self.select_runtime(&context)?;
        runtime.execute(context).await
    }

    async fn setup(&self) -> RuntimeResult<()> {
        info!("Setting up Local runtime");

        self.native.setup().await?;
        self.python.setup().await?;
        self.shell.setup().await?;

        info!("Local runtime setup complete");
        Ok(())
    }

    async fn cleanup(&self) -> RuntimeResult<()> {
        info!("Cleaning up Local runtime");

        self.native.cleanup().await?;
        self.python.cleanup().await?;
        self.shell.cleanup().await?;

        Ok(())
    }

    async fn validate(&self) -> RuntimeResult<()> {
        debug!("Validating Local runtime");

        self.native.validate().await?;
        self.python.validate().await?;
        self.shell.validate().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{OutputFormat, ParameterDelivery, ParameterFormat};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_local_runtime_shell() {
        let runtime = LocalRuntime::new();

        let context = ExecutionContext {
            execution_id: 2,
            action_ref: "test.shell_action".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            execution_env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "run.sh".to_string(),
            code: Some("#!/bin/bash\necho 'hello from shell'".to_string()),
            code_path: None,
            runtime_name: Some("shell".to_string()),
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
            out_schema: None,
            cancel_token: None,
        };

        assert!(runtime.can_execute(&context));
        let result = runtime.execute(context).await.unwrap();
        assert!(result.is_success());
        assert!(result.stdout.contains("hello from shell"));
    }

    #[tokio::test]
    async fn test_local_runtime_unknown() {
        let runtime = LocalRuntime::new();

        let context = ExecutionContext {
            execution_id: 3,
            action_ref: "test.unknown_action".to_string(),
            parameters: HashMap::new(),
            env: HashMap::new(),
            execution_env: HashMap::new(),
            secrets: HashMap::new(),
            timeout: Some(10),
            working_dir: None,
            entry_point: "run".to_string(),
            code: Some("some code".to_string()),
            code_path: None,
            runtime_name: Some("unknown".to_string()),
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
            out_schema: None,
            cancel_token: None,
        };

        assert!(!runtime.can_execute(&context));
    }
}
