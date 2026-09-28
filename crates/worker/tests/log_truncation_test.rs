//! Integration tests for log size truncation
//!
//! Tests that verify stdout/stderr are properly truncated when they exceed
//! configured size limits, preventing OOM issues with large output.

use attune_common::models::runtime::{
    InlineExecutionConfig, InlineExecutionStrategy, InterpreterConfig, RuntimeExecutionConfig,
};
use attune_worker::runtime::process::ProcessRuntime;
use attune_worker::runtime::{ExecutionContext, Runtime};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

fn make_python_process_runtime(packs_base_dir: PathBuf) -> ProcessRuntime {
    let config = RuntimeExecutionConfig {
        interpreter: InterpreterConfig {
            binary: "python3".to_string(),
            args: vec!["-u".to_string()],
            file_extension: Some(".py".to_string()),
        },
        inline_execution: InlineExecutionConfig::default(),
        environment: None,
        dependencies: None,
        env_vars: std::collections::HashMap::new(),
    };
    ProcessRuntime::new(
        "python".to_string(),
        config,
        packs_base_dir.clone(),
        packs_base_dir.join("../runtime_envs"),
    )
}

fn make_shell_process_runtime(packs_base_dir: PathBuf) -> ProcessRuntime {
    let config = RuntimeExecutionConfig {
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
    ProcessRuntime::new(
        "shell".to_string(),
        config,
        packs_base_dir.clone(),
        packs_base_dir.join("../runtime_envs"),
    )
}

fn make_python_context(
    execution_id: i64,
    action_ref: &str,
    code: &str,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> ExecutionContext {
    ExecutionContext {
        execution_id,
        action_ref: action_ref.to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        secrets: HashMap::new(),
        timeout: Some(10),
        working_dir: None,
        entry_point: "inline".to_string(),
        code: Some(code.to_string()),
        code_path: None,
        runtime_name: Some("python".to_string()),
        runtime_config_override: None,
        runtime_env_dir_suffix: None,
        selected_runtime_version: None,
        max_stdout_bytes,
        max_stderr_bytes,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::default(),
        out_schema: None,
        cancel_token: None,
    }
}

#[tokio::test]
async fn test_python_stdout_truncation() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // Create a Python one-liner that outputs more than the limit
    let code = "import sys\nfor i in range(100):\n    print('x' * 10)";

    let context = make_python_context(1, "test.large_output", code, 500, 1024);

    let result = runtime.execute(context).await.unwrap();

    // Should succeed but with truncated output
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout_truncated);
    assert!(
        result.stdout.contains("[OUTPUT TRUNCATED"),
        "Expected truncation marker in stdout, got: {}",
        result.stdout
    );
    assert!(result.stdout_bytes_truncated > 0);
    assert!(result.stdout.len() <= 600); // some overhead for the truncation message
}

#[tokio::test]
async fn test_python_stderr_truncation() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // Python one-liner that outputs to stderr
    let code = "import sys\nfor i in range(100):\n    sys.stderr.write('error message line\\n')";

    let context = make_python_context(2, "test.large_stderr", code, 10 * 1024 * 1024, 300);

    let result = runtime.execute(context).await.unwrap();

    // Should succeed but with truncated stderr
    assert_eq!(result.exit_code, 0);
    assert!(!result.stdout_truncated);
    assert!(result.stderr_truncated);
    assert!(
        result.stderr.contains("[OUTPUT TRUNCATED"),
        "Expected truncation marker in stderr, got: {}",
        result.stderr
    );
    assert!(result.stderr_bytes_truncated > 0);
}

#[tokio::test]
async fn test_shell_stdout_truncation() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_shell_process_runtime(tmp.path().to_path_buf());

    // Shell script that outputs more than the limit
    let code = r#"
for i in $(seq 1 100); do
    echo "This is a long line of text that will add up quickly"
done
"#;

    let context = ExecutionContext {
        execution_id: 3,
        action_ref: "test.shell_large_output".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        secrets: HashMap::new(),
        timeout: Some(10),
        working_dir: None,
        entry_point: "shell".to_string(),
        code: Some(code.to_string()),
        code_path: None,
        runtime_name: Some("shell".to_string()),
        runtime_config_override: None,
        runtime_env_dir_suffix: None,
        selected_runtime_version: None,
        max_stdout_bytes: 400, // Small limit
        max_stderr_bytes: 1024,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::default(),
        out_schema: None,
        cancel_token: None,
    };

    let result = runtime.execute(context).await.unwrap();

    // Should succeed but with truncated output
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout_truncated);
    assert!(
        result.stdout.contains("[OUTPUT TRUNCATED"),
        "Expected truncation marker, got: {}",
        result.stdout
    );
    assert!(result.stdout_bytes_truncated > 0);
}

#[tokio::test]
async fn test_no_truncation_under_limit() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // Small output that won't trigger truncation
    let code = "print('Hello, World!')";

    let context = make_python_context(
        4,
        "test.small_output",
        code,
        10 * 1024 * 1024,
        10 * 1024 * 1024,
    );

    let result = runtime.execute(context).await.unwrap();

    // Should succeed without truncation
    assert_eq!(result.exit_code, 0);
    assert!(!result.stdout_truncated);
    assert!(!result.stderr_truncated);
    assert_eq!(result.stdout_bytes_truncated, 0);
    assert_eq!(result.stderr_bytes_truncated, 0);
    assert!(
        result.stdout.contains("Hello, World!"),
        "Expected Hello, World! in stdout, got: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_both_streams_truncated() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // Script that outputs to both stdout and stderr
    let code = "import sys\nfor i in range(50):\n    print('stdout line ' + str(i))\n    sys.stderr.write('stderr line ' + str(i) + '\\n')";

    let context = make_python_context(5, "test.dual_truncation", code, 300, 300);

    let result = runtime.execute(context).await.unwrap();

    // Should succeed but with both streams truncated
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout_truncated);
    assert!(result.stderr_truncated);
    assert!(result.stdout.contains("[OUTPUT TRUNCATED"));
    assert!(result.stderr.contains("[OUTPUT TRUNCATED"));
    assert!(result.stdout_bytes_truncated > 0);
    assert!(result.stderr_bytes_truncated > 0);
}

#[tokio::test]
async fn test_truncation_with_timeout() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // Script that produces output then times out
    let code = "import time\nfor i in range(1000):\n    print(f'Line {i}')\ntime.sleep(30)";

    let mut context = make_python_context(6, "test.timeout_truncation", code, 500, 1024);
    context.timeout = Some(2); // Short timeout

    let result = runtime.execute(context).await.unwrap();

    // Should timeout with truncated logs
    assert!(!result.is_success());
    assert!(result.error.is_some());
    assert!(
        result.error.as_ref().unwrap().contains("timed out"),
        "Expected timeout error, got: {:?}",
        result.error
    );
}

#[tokio::test]
async fn test_small_output_no_truncation() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // Output a small amount that won't trigger truncation
    let code = "import sys; sys.stdout.write('Small output')";

    let context = make_python_context(
        7,
        "test.exact_limit",
        code,
        10 * 1024 * 1024,
        10 * 1024 * 1024,
    );

    let result = runtime.execute(context).await.unwrap();

    // Should succeed without truncation
    assert_eq!(result.exit_code, 0);
    assert!(!result.stdout_truncated);
    assert!(
        result.stdout.contains("Small output"),
        "Expected 'Small output' in stdout, got: {:?}",
        result.stdout
    );
}

#[tokio::test]
async fn test_shell_process_runtime_truncation() {
    // Test truncation through ProcessRuntime with shell config too
    let tmp = TempDir::new().unwrap();

    let config = RuntimeExecutionConfig {
        interpreter: InterpreterConfig {
            binary: "/bin/bash".to_string(),
            args: vec![],
            file_extension: Some(".sh".to_string()),
        },
        inline_execution: InlineExecutionConfig::default(),
        environment: None,
        dependencies: None,
        env_vars: std::collections::HashMap::new(),
    };
    let runtime = ProcessRuntime::new(
        "shell".to_string(),
        config,
        tmp.path().to_path_buf(),
        tmp.path().join("runtime_envs"),
    );

    let context = ExecutionContext {
        execution_id: 8,
        action_ref: "test.shell_process_truncation".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        secrets: HashMap::new(),
        timeout: Some(10),
        working_dir: None,
        entry_point: "inline".to_string(),
        code: Some(
            "for i in $(seq 1 200); do echo \"output line $i padding text here\"; done".to_string(),
        ),
        code_path: None,
        runtime_name: Some("shell".to_string()),
        runtime_config_override: None,
        runtime_env_dir_suffix: None,
        selected_runtime_version: None,
        max_stdout_bytes: 500,
        max_stderr_bytes: 1024,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::default(),
        out_schema: None,
        cancel_token: None,
    };

    let result = runtime.execute(context).await.unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout_truncated);
    assert!(result.stdout.contains("[OUTPUT TRUNCATED"));
    assert!(result.stdout_bytes_truncated > 0);
}
