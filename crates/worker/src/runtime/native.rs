//! Native Runtime
//!
//! Executes compiled native binaries directly without any shell or interpreter wrapper.
//! This runtime is used for Rust binaries and other compiled executables.

use super::{
    parameter_passing::{self, ParameterDeliveryConfig},
    BoundedLogFileWriter, ExecutionContext, ExecutionResult, OutputFormat, Runtime, RuntimeError,
    RuntimeResult,
};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Native runtime for executing compiled binaries
pub struct NativeRuntime {
    work_dir: Option<std::path::PathBuf>,
    child_environment: attune_common::child_process_environment::ChildProcessEnvironment,
}

impl NativeRuntime {
    /// Create a new native runtime
    pub fn new() -> Self {
        Self {
            work_dir: None,
            child_environment: Default::default(),
        }
    }

    /// Create a native runtime with custom working directory
    pub fn with_work_dir(work_dir: std::path::PathBuf) -> Self {
        Self {
            work_dir: Some(work_dir),
            child_environment: Default::default(),
        }
    }

    pub fn with_child_environment(
        mut self,
        environment: attune_common::child_process_environment::ChildProcessEnvironment,
    ) -> Self {
        self.child_environment = environment;
        self
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
        output_format: OutputFormat,
        out_schema: Option<&serde_json::Value>,
        _stdout_log_path: Option<&Path>,
        _stderr_log_path: Option<&Path>,
        stdout_log_writer: Option<BoundedLogFileWriter>,
        stderr_log_writer: Option<BoundedLogFileWriter>,
        cancel_token: Option<CancellationToken>,
    ) -> RuntimeResult<ExecutionResult> {
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

        parameter_passing::apply_runtime_environment(&mut cmd, env, &self.child_environment);

        super::process_executor::execute_streaming_cancellable(
            cmd,
            &std::collections::HashMap::new(),
            parameters_stdin,
            timeout,
            max_stdout_bytes,
            max_stderr_bytes,
            output_format,
            out_schema,
            cancel_token,
            None,
            None,
            stdout_log_writer,
            stderr_log_writer,
        )
        .await
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
        parameter_passing::merge_execution_environment(&mut env, &context.execution_env);

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
            context.output_format,
            context.out_schema.as_ref(),
            context.stdout_log_path.as_deref(),
            context.stderr_log_path.as_deref(),
            context.stdout_log_writer,
            context.stderr_log_writer,
            context.cancel_token,
        )
        .await
    }

    async fn setup(&self) -> RuntimeResult<()> {
        info!("Setting up Native runtime");

        // Verify we can execute native binaries (basic check)
        #[cfg(unix)]
        {
            use std::process::Command;
            let mut command = Command::new("uname");
            self.child_environment.apply(&mut command);
            let output = command.arg("-s").output().map_err(|e| {
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
            let mut command = Command::new("echo");
            self.child_environment.apply(&mut command);
            command.arg("test").output().map_err(|e| {
                RuntimeError::SetupError(format!("Native runtime validation failed: {}", e))
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use async_trait::async_trait;
    #[cfg(unix)]
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[cfg(unix)]
    use std::sync::Arc;
    #[cfg(unix)]
    use tokio::sync::Notify;
    #[cfg(unix)]
    use tokio::time::{sleep, Duration};

    #[cfg(unix)]
    #[derive(Debug, Default)]
    struct HangingLogTransport {
        active: AtomicUsize,
        started: Notify,
    }

    #[cfg(unix)]
    struct ActiveCommit<'a>(&'a AtomicUsize);

    #[cfg(unix)]
    impl Drop for ActiveCommit<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[cfg(unix)]
    #[async_trait]
    impl attune_common::artifact_transport::ArtifactFileTransport for HangingLogTransport {
        async fn write_file(
            &self,
            _: &str,
            _: &[u8],
            _: Option<&str>,
        ) -> attune_common::Result<()> {
            unreachable!()
        }

        async fn file_exists(&self, _: &str) -> attune_common::Result<bool> {
            unreachable!()
        }

        async fn file_size(&self, _: &str) -> attune_common::Result<Option<u64>> {
            unreachable!()
        }

        async fn delete_file(&self, _: &str) -> attune_common::Result<()> {
            unreachable!()
        }
        async fn append_log_file(&self, _: &str, _: &[u8]) -> attune_common::Result<()> {
            Ok(())
        }

        async fn commit_log_segment(&self, _: i64, _: i64, _: &[u8]) -> attune_common::Result<()> {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = ActiveCommit(&self.active);
            self.started.notify_one();
            std::future::pending().await
        }

        async fn seal_log_stream(&self, _: i64, _: bool) -> attune_common::Result<()> {
            Ok(())
        }

        async fn open_reader(
            &self,
            _: &str,
            _: u64,
        ) -> attune_common::Result<attune_common::artifact_transport::BoxAsyncReader> {
            unreachable!()
        }

        fn transport_mode(&self) -> &'static str {
            "test"
        }

        fn base_dir(&self) -> &str {
            ""
        }
    }

    #[cfg(unix)]
    fn write_executable(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let path = dir.path().join("native-test");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
            .unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[cfg(unix)]
    fn hanging_writer(
        transport: Arc<HangingLogTransport>,
        finalization_timeout_ms: u64,
    ) -> BoundedLogFileWriter {
        let segmented = attune_common::log_stream::SegmentedLogWriter::new(
            transport,
            1,
            attune_common::log_stream::SegmentedLogConfig {
                initial_segment_bytes: 1,
                max_segment_bytes: 1,
                flush_interval_ms: 60_000,
                retry_max_attempts: 3,
                retry_attempt_timeout_ms: 60_000,
                retry_initial_backoff_ms: 1,
                retry_max_backoff_ms: 2,
                finalization_timeout_ms,
            },
        )
        .unwrap();
        BoundedLogFileWriter::from_segmented_writer(segmented, 1024, true)
    }

    #[cfg(unix)]
    fn native_context(
        path: PathBuf,
        timeout: Option<u64>,
        cancel_token: Option<CancellationToken>,
        stdout_log_writer: BoundedLogFileWriter,
    ) -> ExecutionContext {
        let mut context = ExecutionContext::test_context("test.native".to_string(), None);
        context.code_path = Some(path);
        context.runtime_name = Some("native".to_string());
        context.timeout = timeout;
        context.cancel_token = cancel_token;
        context.stdout_log_writer = Some(stdout_log_writer);
        context
    }

    #[tokio::test]
    async fn test_native_runtime_name() {
        let runtime = NativeRuntime::new();
        assert_eq!(runtime.name(), "native");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_env_reaches_native_processes_and_preserves_internal_context() {
        let mut context = ExecutionContext::test_context("test.env".into(), None);
        context.code_path = Some(PathBuf::from("/usr/bin/env"));
        context.env.insert("ATTUNE_EXEC_ID".into(), "42".into());
        context
            .env
            .insert("ATTUNE_TEST_OVERRIDE".into(), "internal".into());
        context.execution_env = std::collections::HashMap::from([
            ("EXECUTION_ENV_TEST".into(), "visible".into()),
            ("ATTUNE_EXEC_ID".into(), "bad".into()),
        ]);
        let result = NativeRuntime::new().execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result
            .stdout
            .lines()
            .any(|line| line == "EXECUTION_ENV_TEST=visible"));
        assert!(result
            .stdout
            .lines()
            .any(|line| line == "ATTUNE_EXEC_ID=42"));
        assert!(!result
            .stdout
            .lines()
            .any(|line| line == "ATTUNE_EXEC_ID=bad"));
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

    #[cfg(unix)]
    #[tokio::test]
    async fn native_text_output_with_secret_schema_uses_suppressed_mirror_path() {
        use attune_common::runtime_log_mirror::RuntimeLogSource;
        use serde_json::json;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = write_executable(&temp_dir, r#"printf '{"token":"secret"}\n'"#);
        let transport = Arc::new(HangingLogTransport::default());
        let writer = attune_common::log_stream::SharedFileLogWriter::new(
            transport,
            1,
            "native-text.log".to_string(),
        );
        let source = RuntimeLogSource::Execution {
            execution_id: 1,
            parent_execution_id: None,
            action_ref: "test.native".to_string(),
            pack_ref: "test".to_string(),
            trace_tag: None,
            worker_id: 1,
            worker_name: "test-worker".to_string(),
            worker_instance: uuid::Uuid::nil(),
        };
        let mut context = ExecutionContext::test_context("test.native".to_string(), None);
        context.code_path = Some(path);
        context.runtime_name = Some("native".to_string());
        context.output_format = OutputFormat::Text;
        context.out_schema = Some(json!({
            "token": {"type": "string", "secret": true}
        }));
        context.stdout_log_writer = Some(
            BoundedLogFileWriter::from_shared_file_writer(writer, 1024, true, 1_000)
                .with_mirror_source(Some(source)),
        );

        let result = NativeRuntime::new().execute(context).await.unwrap();

        assert!(result.result.is_none());
        assert_eq!(result.stdout, "{\"token\":\"secret\"}\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_log_upload_is_aborted_on_cancellation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = write_executable(&temp_dir, "printf 'x\\n'; sleep 30");
        let transport = Arc::new(HangingLogTransport::default());
        let cancel_token = CancellationToken::new();
        let trigger = cancel_token.clone();
        let started = transport.clone();
        let cancellation = tokio::spawn(async move {
            started.started.notified().await;
            trigger.cancel();
        });
        let context = native_context(
            path,
            Some(60),
            Some(cancel_token),
            hanging_writer(transport.clone(), 5_000),
        );

        let result = NativeRuntime::new().execute(context).await.unwrap();
        cancellation.await.unwrap();

        assert!(result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("cancelled")));
        assert!(result.logs_incomplete);
        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert!(result.duration_ms < 5_000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_log_upload_is_aborted_on_timeout() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = write_executable(&temp_dir, "printf 'x\\n'; sleep 30");
        let transport = Arc::new(HangingLogTransport::default());
        let context = native_context(
            path,
            Some(1),
            None,
            hanging_writer(transport.clone(), 5_000),
        );

        let result = NativeRuntime::new().execute(context).await.unwrap();

        assert!(result.timed_out);
        assert!(result.logs_incomplete);
        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert!(result.duration_ms < 5_000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_after_child_exit_aborts_blocked_log_upload() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = write_executable(&temp_dir, "printf 'x\\n'");
        let transport = Arc::new(HangingLogTransport::default());
        let cancel_token = CancellationToken::new();
        let trigger = cancel_token.clone();
        let started = transport.clone();
        let cancellation = tokio::spawn(async move {
            started.started.notified().await;
            sleep(Duration::from_millis(25)).await;
            trigger.cancel();
        });
        let context = native_context(
            path,
            Some(60),
            Some(cancel_token),
            hanging_writer(transport.clone(), 5_000),
        );

        let result = NativeRuntime::new().execute(context).await.unwrap();
        cancellation.await.unwrap();

        assert!(result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("cancelled")));
        assert!(result.logs_incomplete);
        assert_eq!(transport.active.load(Ordering::SeqCst), 0);
        assert!(result.duration_ms < 1_000);
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
