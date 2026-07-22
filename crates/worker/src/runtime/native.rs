//! Native Runtime
//!
//! Executes compiled native binaries directly without any shell or interpreter wrapper.
//! This runtime is used for Rust binaries and other compiled executables.

use super::{
    parameter_passing::{self, ParameterDeliveryConfig},
    BoundedLogFileWriter, BoundedLogWriter, ExecutionContext, ExecutionResult, Runtime,
    RuntimeError, RuntimeResult,
};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};

/// Native runtime for executing compiled binaries
pub struct NativeRuntime {
    work_dir: Option<std::path::PathBuf>,
}

impl NativeRuntime {
    const ETXTBSY_RETRY_ATTEMPTS: usize = 5;
    const ETXTBSY_RETRY_DELAY_MS: u64 = 25;

    /// Create a new native runtime
    pub fn new() -> Self {
        Self { work_dir: None }
    }

    /// Create a native runtime with custom working directory
    pub fn with_work_dir(work_dir: std::path::PathBuf) -> Self {
        Self {
            work_dir: Some(work_dir),
        }
    }

    /// Execute a native binary with parameters and environment variables
    #[allow(clippy::too_many_arguments)]
    async fn execute_binary(
        &self,
        binary_path: PathBuf,
        _secrets: &std::collections::HashMap<String, serde_json::Value>,
        env: &std::collections::HashMap<String, String>,
        parameters_stdin: Option<&str>,
        timeout: Option<u64>,
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
        stdout_log_path: Option<&Path>,
        stderr_log_path: Option<&Path>,
        stdout_log_writer: Option<BoundedLogFileWriter>,
        stderr_log_writer: Option<BoundedLogFileWriter>,
    ) -> RuntimeResult<ExecutionResult> {
        let start = Instant::now();

        // Check if binary exists and is executable
        if !binary_path.exists() {
            return Err(RuntimeError::ExecutionFailed(format!(
                "Binary not found: {}",
                binary_path.display()
            )));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(&binary_path)?;
            let permissions = metadata.permissions();
            if permissions.mode() & 0o111 == 0 {
                return Err(RuntimeError::ExecutionFailed(format!(
                    "Binary is not executable: {}",
                    binary_path.display()
                )));
            }
        }

        debug!("Executing native binary: {}", binary_path.display());

        // Build command
        let mut cmd = Command::new(&binary_path);

        // Set working directory
        if let Some(ref work_dir) = self.work_dir {
            cmd.current_dir(work_dir);
        }

        // Add the explicit execution environment after removing any API token
        // inherited by the worker process.
        parameter_passing::apply_runtime_environment(&mut cmd, env);

        // Configure stdio
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Run the binary in its own process group so a timeout can terminate
        // the whole group (the binary plus anything it spawned) with
        // SIGTERM -> grace -> SIGKILL, consistent with ProcessRuntime.
        if let Err(e) = super::process_executor::configure_child_process(&mut cmd) {
            return Err(RuntimeError::ExecutionFailed(format!(
                "Failed to configure child process group: {}",
                e
            )));
        }

        // Some filesystems can transiently return ETXTBSY immediately after an
        // executable is written and chmod'd. Treat that as a short-lived spawn
        // race and retry a few times before failing hard.
        let mut last_spawn_error = None;
        let mut child = None;
        for attempt in 0..Self::ETXTBSY_RETRY_ATTEMPTS {
            match cmd.spawn() {
                Ok(process) => {
                    child = Some(process);
                    break;
                }
                Err(e) if e.raw_os_error() == Some(26) => {
                    debug!(
                        "Native binary temporarily busy on spawn attempt {}/{} for {}",
                        attempt + 1,
                        Self::ETXTBSY_RETRY_ATTEMPTS,
                        binary_path.display()
                    );
                    last_spawn_error = Some(e);
                    sleep(Duration::from_millis(Self::ETXTBSY_RETRY_DELAY_MS)).await;
                }
                Err(e) => {
                    return Err(RuntimeError::ExecutionFailed(format!(
                        "Failed to spawn binary: {}",
                        e
                    )));
                }
            }
        }

        let mut child = child.ok_or_else(|| {
            let error = last_spawn_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unknown error".to_string());
            RuntimeError::ExecutionFailed(format!(
                "Failed to spawn binary after retrying ETXTBSY: {}",
                error
            ))
        })?;

        #[cfg(windows)]
        let process_tree =
            super::process_executor::WindowsProcessTree::assign(&child).map_err(|e| {
                RuntimeError::ExecutionFailed(format!(
                    "Failed to assign native binary to Windows process tree: {}",
                    e
                ))
            })?;

        // Write parameters to stdin as a single JSON line.
        // Secrets are merged into the parameters map by the caller, so the
        // action reads everything with a single readline().
        let stdin_write_error = if let Some(mut stdin) = child.stdin.take() {
            let mut error = None;

            if let Some(params_data) = parameters_stdin {
                if let Err(e) = stdin.write_all(params_data.as_bytes()).await {
                    error = Some(format!("Failed to write parameters to stdin: {}", e));
                } else if let Err(e) = stdin.write_all(b"\n").await {
                    error = Some(format!("Failed to write newline to stdin: {}", e));
                }
            }

            // Close stdin
            if let Err(e) = stdin.shutdown().await {
                if error.is_none() {
                    error = Some(format!("Failed to close stdin: {}", e));
                }
            }
            error
        } else {
            None
        };

        // Capture stdout and stderr with size limits
        let stdout_handle = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::ProcessError("Failed to capture stdout".to_string()))?;
        let stderr_handle = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::ProcessError("Failed to capture stderr".to_string()))?;

        let mut stdout_writer = BoundedLogWriter::new_stdout(max_stdout_bytes);
        let mut stderr_writer = BoundedLogWriter::new_stderr(max_stderr_bytes);
        // Prefer pre-opened transport writers over path-based file writers
        let mut stdout_file = stdout_log_writer
            .or_else(|| open_live_log_file(stdout_log_path, max_stdout_bytes, true));
        let mut stderr_file = stderr_log_writer
            .or_else(|| open_live_log_file(stderr_log_path, max_stderr_bytes, false));

        // Create buffered readers
        let mut stdout_reader = BufReader::new(stdout_handle);
        let mut stderr_reader = BufReader::new(stderr_handle);

        // Stream both outputs concurrently
        let stdout_task = async {
            let mut line = Vec::new();
            loop {
                line.clear();
                match stdout_reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if stdout_writer.write_all(&line).await.is_err() {
                            break;
                        }
                        if let Some(file) = stdout_file.as_mut() {
                            let _ = file.write_all(&line).await;
                        }
                    }
                    Err(_) => break,
                }
            }
            stdout_writer
        };

        let stderr_task = async {
            let mut line = Vec::new();
            loop {
                line.clear();
                match stderr_reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if stderr_writer.write_all(&line).await.is_err() {
                            break;
                        }
                        if let Some(file) = stderr_file.as_mut() {
                            let _ = file.write_all(&line).await;
                        }
                    }
                    Err(_) => break,
                }
            }
            stderr_writer
        };

        // Wait for both streams to complete
        let (stdout_writer, stderr_writer) = tokio::join!(stdout_task, stderr_task);

        // Wait for process with timeout
        let mut timed_out = false;
        let wait_result = if let Some(timeout_secs) = timeout {
            match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await {
                Ok(result) => result,
                Err(_) => {
                    warn!(
                        "Native binary execution timed out after {} seconds; \
                         sending SIGTERM to process group",
                        timeout_secs
                    );
                    timed_out = true;
                    #[cfg(windows)]
                    process_tree.terminate();
                    // SIGTERM the process group, then escalate to SIGKILL after a
                    // 10s grace period (handled inside wait_for_terminated_child).
                    super::process_executor::terminate_process(&mut child, "timed-out");
                    super::process_executor::wait_for_terminated_child(&mut child).await
                }
            }
        } else {
            child.wait().await
        };

        let status = wait_result.map_err(|e| {
            RuntimeError::ExecutionFailed(format!("Failed to wait for process: {}", e))
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;
        let exit_code = status.code().unwrap_or(-1);

        // Extract logs with truncation info
        let stdout_log = stdout_writer.into_result();
        let stderr_log = stderr_writer.into_result();

        debug!(
            "Native binary completed with exit code {} in {}ms",
            exit_code, duration_ms
        );

        if stdout_log.truncated {
            warn!(
                "stdout truncated: {} bytes over limit",
                stdout_log.bytes_truncated
            );
        }
        if stderr_log.truncated {
            warn!(
                "stderr truncated: {} bytes over limit",
                stderr_log.bytes_truncated
            );
        }

        // Parse result from stdout if successful
        let result = if exit_code == 0 {
            serde_json::from_str(&stdout_log.content).ok()
        } else {
            None
        };

        // Determine error message
        let error = if timed_out {
            Some(format!(
                "Execution timed out after {} seconds",
                timeout.unwrap_or(0)
            ))
        } else if exit_code != 0 {
            Some(format!(
                "Native binary exited with code {}: {}",
                exit_code,
                stderr_log.content.trim()
            ))
        } else if let Some(stdin_err) = stdin_write_error {
            // Ignore broken pipe errors for fast-exiting successful actions
            // These occur when the process exits before we finish writing secrets to stdin
            let is_broken_pipe =
                stdin_err.contains("Broken pipe") || stdin_err.contains("os error 32");
            let is_fast_exit = duration_ms < 500;
            let is_success = exit_code == 0;

            if is_broken_pipe && is_fast_exit && is_success {
                debug!(
                    "Ignoring broken pipe error for fast-exiting successful action ({}ms)",
                    duration_ms
                );
                None
            } else {
                Some(stdin_err)
            }
        } else {
            None
        };

        Ok(ExecutionResult {
            exit_code,
            // Only populate stdout if result wasn't parsed (avoid duplication)
            stdout: if result.is_some() {
                String::new()
            } else {
                stdout_log.content
            },
            stderr: stderr_log.content,
            result,
            duration_ms,
            error,
            stdout_truncated: stdout_log.truncated,
            stderr_truncated: stderr_log.truncated,
            stdout_bytes_truncated: stdout_log.bytes_truncated,
            stderr_bytes_truncated: stderr_log.bytes_truncated,
            timed_out,
        })
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runtime for NativeRuntime {
    fn name(&self) -> &str {
        "native"
    }

    fn can_execute(&self, context: &ExecutionContext) -> bool {
        // Check if runtime_name is explicitly set to "native"
        if let Some(ref runtime_name) = context.runtime_name {
            return runtime_name.to_lowercase() == "native";
        }

        // Otherwise, check if code_path points to an executable binary
        // This is a heuristic - native binaries typically don't have common script extensions
        if let Some(ref code_path) = context.code_path {
            let extension = code_path.extension().and_then(|e| e.to_str()).unwrap_or("");

            // Exclude common script extensions
            let is_script = matches!(
                extension,
                "py" | "js" | "sh" | "bash" | "rb" | "pl" | "php" | "lua"
            );

            // If it's not a script and the file exists, it might be a native binary
            !is_script && code_path.exists()
        } else {
            false
        }
    }

    async fn execute(&self, context: ExecutionContext) -> RuntimeResult<ExecutionResult> {
        info!(
            "Executing native action: {} (execution_id: {}) with parameter delivery: {:?}, format: {:?}",
            context.action_ref, context.execution_id, context.parameter_delivery, context.parameter_format
        );

        // Merge secrets into parameters as a single JSON document.
        // Actions receive everything via one readline() on stdin.
        // Secret values are already JsonValue (string, object, array, etc.)
        // so they are inserted directly without wrapping.
        let merged_parameters =
            parameter_passing::merge_parameters_and_secrets(&context.parameters, &context.secrets);

        // Prepare environment and parameters according to delivery method
        let mut env = context.env.clone();
        let config = ParameterDeliveryConfig {
            delivery: context.parameter_delivery,
            format: context.parameter_format,
        };

        let prepared_params =
            parameter_passing::prepare_parameters(&merged_parameters, &mut env, config)?;

        // Get stdin content if parameters are delivered via stdin
        let parameters_stdin = prepared_params.stdin_content();

        // Get the binary path
        let binary_path = context.code_path.ok_or_else(|| {
            RuntimeError::InvalidAction("Native runtime requires code_path to be set".to_string())
        })?;

        self.execute_binary(
            binary_path,
            &std::collections::HashMap::new(),
            &env,
            parameters_stdin,
            context.timeout,
            context.max_stdout_bytes,
            context.max_stderr_bytes,
            context.stdout_log_path.as_deref(),
            context.stderr_log_path.as_deref(),
            context.stdout_log_writer,
            context.stderr_log_writer,
        )
        .await
    }

    async fn setup(&self) -> RuntimeResult<()> {
        info!("Setting up Native runtime");

        // Verify we can execute native binaries (basic check)
        #[cfg(unix)]
        {
            use std::process::Command;
            let output = Command::new("uname").arg("-s").output().map_err(|e| {
                RuntimeError::SetupError(format!("Failed to verify native runtime: {}", e))
            })?;

            if !output.status.success() {
                return Err(RuntimeError::SetupError(
                    "Failed to execute native commands".to_string(),
                ));
            }

            debug!("Native runtime setup complete");
        }

        Ok(())
    }

    async fn cleanup(&self) -> RuntimeResult<()> {
        info!("Cleaning up Native runtime");
        // No cleanup needed for native runtime
        Ok(())
    }

    async fn validate(&self) -> RuntimeResult<()> {
        debug!("Validating Native runtime");

        // Basic validation - ensure we can execute commands
        #[cfg(unix)]
        {
            use std::process::Command;
            Command::new("echo").arg("test").output().map_err(|e| {
                RuntimeError::SetupError(format!("Native runtime validation failed: {}", e))
            })?;
        }

        Ok(())
    }
}

fn open_live_log_file(
    path: Option<&Path>,
    max_bytes: usize,
    is_stdout: bool,
) -> Option<BoundedLogFileWriter> {
    let path = path?;
    let writer = if is_stdout {
        BoundedLogFileWriter::new_stdout(path, max_bytes)
    } else {
        BoundedLogFileWriter::new_stderr(path, max_bytes)
    };
    Some(writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_native_runtime_name() {
        let runtime = NativeRuntime::new();
        assert_eq!(runtime.name(), "native");
    }

    #[tokio::test]
    async fn test_native_runtime_can_execute() {
        let runtime = NativeRuntime::new();

        // Test with explicit runtime_name
        let mut context = ExecutionContext::test_context("test.action".to_string(), None);
        context.runtime_name = Some("native".to_string());
        assert!(runtime.can_execute(&context));

        // Test with uppercase runtime_name
        context.runtime_name = Some("NATIVE".to_string());
        assert!(runtime.can_execute(&context));

        // Test with wrong runtime_name
        context.runtime_name = Some("python".to_string());
        assert!(!runtime.can_execute(&context));
    }

    #[tokio::test]
    async fn test_native_runtime_setup() {
        let runtime = NativeRuntime::new();
        let result = runtime.setup().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_native_runtime_validate() {
        let runtime = NativeRuntime::new();
        let result = runtime.validate().await;
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_native_runtime_execute_simple() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("test_binary.sh");

        // Create a simple shell script as our "binary" and ensure the file
        // handle is closed before executing it (Linux returns ETXTBSY for
        // executables still open for writing).
        {
            use std::io::Write;

            let mut file = fs::File::create(&binary_path).unwrap();
            file.write_all(b"#!/bin/bash\necho 'Hello from native runtime'\n")
                .unwrap();
            file.sync_all().unwrap();
        }

        // Make it executable
        let metadata = fs::metadata(&binary_path).unwrap();
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary_path, permissions).unwrap();

        let runtime = NativeRuntime::with_work_dir(temp_dir.path().to_path_buf());
        let mut context = ExecutionContext::test_context("test.native".to_string(), None);
        context.code_path = Some(binary_path);
        context.runtime_name = Some("native".to_string());

        let result = runtime.execute(context).await;
        assert!(result.is_ok(), "execute failed: {:?}", result.err());

        let exec_result = result.unwrap();
        assert_eq!(exec_result.exit_code, 0);
        assert!(exec_result.stdout.contains("Hello from native runtime"));
    }

    #[tokio::test]
    async fn test_native_runtime_missing_binary() {
        let runtime = NativeRuntime::new();
        let mut context = ExecutionContext::test_context("test.native".to_string(), None);
        context.code_path = Some(std::path::PathBuf::from("/nonexistent/binary"));
        context.runtime_name = Some("native".to_string());

        let result = runtime.execute(context).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RuntimeError::ExecutionFailed(_)
        ));
    }

    #[tokio::test]
    async fn test_native_runtime_no_code_path() {
        let runtime = NativeRuntime::new();
        let mut context = ExecutionContext::test_context("test.native".to_string(), None);
        context.runtime_name = Some("native".to_string());
        // code_path is None

        let result = runtime.execute(context).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RuntimeError::InvalidAction(_)
        ));
    }
}
