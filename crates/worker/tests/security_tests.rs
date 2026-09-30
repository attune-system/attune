//! Security Tests for Secret Handling
//!
//! These tests verify that secrets are NOT exposed in process environment
//! or command-line arguments, ensuring secure secret passing via stdin.

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
    let runtime_envs_dir = packs_base_dir
        .parent()
        .unwrap_or(&packs_base_dir)
        .join("runtime_envs");
    ProcessRuntime::new(
        "python".to_string(),
        config,
        packs_base_dir,
        runtime_envs_dir,
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
    let runtime_envs_dir = packs_base_dir
        .parent()
        .unwrap_or(&packs_base_dir)
        .join("runtime_envs");
    ProcessRuntime::new(
        "shell".to_string(),
        config,
        packs_base_dir,
        runtime_envs_dir,
    )
}

#[tokio::test]
async fn test_python_secrets_not_in_environ() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // Inline Python code that checks environment for secrets
    let code = r#"
import os, json

environ_str = str(os.environ)

# Secrets should NOT be in environment
has_secret_in_env = 'super_secret_key_do_not_expose' in environ_str
has_password_in_env = 'secret_pass_123' in environ_str
legacy_secret_keys = {'SECRET_API_KEY', 'SECRET_PASSWORD'}
has_secret_prefix = any(k in os.environ for k in legacy_secret_keys)

result = {
    'secrets_in_environ': has_secret_in_env or has_password_in_env or has_secret_prefix,
    'environ_check': not has_secret_prefix
}
print(json.dumps(result))
"#;

    let context = ExecutionContext {
        execution_id: 1,
        action_ref: "security.test_environ".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
        secrets: {
            let mut s = HashMap::new();
            s.insert(
                "api_key".to_string(),
                serde_json::json!("super_secret_key_do_not_expose"),
            );
            s.insert("password".to_string(), serde_json::json!("secret_pass_123"));
            s
        },
        timeout: Some(10),
        working_dir: None,
        entry_point: "inline".to_string(),
        code: Some(code.to_string()),
        code_path: None,
        runtime_name: Some("python".to_string()),
        runtime_config_override: None,
        runtime_env_dir_suffix: None,
        selected_runtime_version: None,
        max_stdout_bytes: 10 * 1024 * 1024,
        max_stderr_bytes: 10 * 1024 * 1024,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::Json,
        out_schema: None,
        cancel_token: None,
    };

    let result = runtime.execute(context).await.unwrap();
    assert_eq!(
        result.exit_code, 0,
        "Execution should succeed. stderr: {}",
        result.stderr
    );

    let result_data = result.result.expect("Should have parsed JSON result");

    // Critical security check: secrets should NOT be in environment
    assert_eq!(
        result_data.get("secrets_in_environ").unwrap(),
        &serde_json::json!(false),
        "SECURITY FAILURE: Secrets found in process environment!"
    );

    // Verify no SECRET_ prefix in environment
    assert_eq!(
        result_data.get("environ_check").unwrap(),
        &serde_json::json!(true),
        "Environment should not contain SECRET_ prefix variables"
    );
}

#[tokio::test]
async fn test_shell_secrets_not_in_environ() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_shell_process_runtime(tmp.path().to_path_buf());

    let context = ExecutionContext {
        execution_id: 2,
        action_ref: "security.test_shell_environ".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
        secrets: {
            let mut s = HashMap::new();
            s.insert(
                "api_key".to_string(),
                serde_json::json!("super_secret_key_do_not_expose"),
            );
            s.insert("password".to_string(), serde_json::json!("secret_pass_123"));
            s
        },
        timeout: Some(10),
        working_dir: None,
        entry_point: "shell".to_string(),
        code: Some(
            r#"
# Check if secrets are in environment variables
if printenv | grep -q "super_secret_key_do_not_expose"; then
    echo "SECURITY_FAIL: Secret found in environment"
    exit 1
fi

if printenv | grep -q "secret_pass_123"; then
    echo "SECURITY_FAIL: Password found in environment"
    exit 1
fi

if printenv | grep -q "SECRET_API_KEY"; then
    echo "SECURITY_FAIL: SECRET_ prefix found in environment"
    exit 1
fi

# Shell inline execution receives the merged input set as ordinary variables
api_key="$api_key"
password="$password"

if [ "$api_key" != "super_secret_key_do_not_expose" ]; then
    echo "ERROR: Secret not accessible via merged inputs"
    exit 1
fi

if [ "$password" != "secret_pass_123" ]; then
    echo "ERROR: Password not accessible via merged inputs"
    exit 1
fi

echo "SECURITY_PASS: Secrets not in inherited environment and accessible via merged inputs"
"#
            .to_string(),
        ),
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
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::default(),
        out_schema: None,
        cancel_token: None,
    };

    let result = runtime.execute(context).await.unwrap();

    // Check execution succeeded
    assert!(
        result.is_success(),
        "Execution should succeed. stderr: {}",
        result.stderr
    );
    assert_eq!(result.exit_code, 0, "Exit code should be 0");

    // Verify security pass message
    assert!(
        result.stdout.contains("SECURITY_PASS"),
        "Security checks should pass. stdout: {}",
        result.stdout
    );
    assert!(
        !result.stdout.contains("SECURITY_FAIL"),
        "Should not have security failures. stdout: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_python_secrets_isolated_between_actions() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // First action with secret A — read it from stdin
    let code1 = r#"
import sys, json

# Read secrets from stdin (the process executor writes them as JSON on stdin)
secrets_line = sys.stdin.readline().strip()
secrets = json.loads(secrets_line) if secrets_line else {}
print(json.dumps({'secret_a': secrets.get('secret_a')}))
"#;

    let context1 = ExecutionContext {
        execution_id: 3,
        action_ref: "security.action1".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
        secrets: {
            let mut s = HashMap::new();
            s.insert("secret_a".to_string(), serde_json::json!("value_a"));
            s
        },
        timeout: Some(10),
        working_dir: None,
        entry_point: "inline".to_string(),
        code: Some(code1.to_string()),
        code_path: None,
        runtime_name: Some("python".to_string()),
        runtime_config_override: None,
        runtime_env_dir_suffix: None,
        selected_runtime_version: None,
        max_stdout_bytes: 10 * 1024 * 1024,
        max_stderr_bytes: 10 * 1024 * 1024,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::Json,
        out_schema: None,
        cancel_token: None,
    };

    let result1 = runtime.execute(context1).await.unwrap();
    assert_eq!(
        result1.exit_code, 0,
        "First action should succeed. stderr: {}",
        result1.stderr
    );

    // Second action with secret B — should NOT see secret A
    let code2 = r#"
import sys, json

secrets_line = sys.stdin.readline().strip()
secrets = json.loads(secrets_line) if secrets_line else {}
print(json.dumps({
    'secret_a_leaked': secrets.get('secret_a') is not None,
    'secret_b_present': secrets.get('secret_b') == 'value_b'
}))
"#;

    let context2 = ExecutionContext {
        execution_id: 4,
        action_ref: "security.action2".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
        secrets: {
            let mut s = HashMap::new();
            s.insert("secret_b".to_string(), serde_json::json!("value_b"));
            s
        },
        timeout: Some(10),
        working_dir: None,
        entry_point: "inline".to_string(),
        code: Some(code2.to_string()),
        code_path: None,
        runtime_name: Some("python".to_string()),
        runtime_config_override: None,
        runtime_env_dir_suffix: None,
        selected_runtime_version: None,
        max_stdout_bytes: 10 * 1024 * 1024,
        max_stderr_bytes: 10 * 1024 * 1024,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::Json,
        out_schema: None,
        cancel_token: None,
    };

    let result2 = runtime.execute(context2).await.unwrap();
    assert_eq!(
        result2.exit_code, 0,
        "Second action should succeed. stderr: {}",
        result2.stderr
    );

    let result_data = result2.result.expect("Should have parsed JSON result");

    // Verify secrets don't leak between actions
    assert_eq!(
        result_data.get("secret_a_leaked").unwrap(),
        &serde_json::json!(false),
        "Secret from previous action should not leak"
    );
    assert_eq!(
        result_data.get("secret_b_present").unwrap(),
        &serde_json::json!(true),
        "Current action's secret should be present"
    );
}

#[tokio::test]
async fn test_python_empty_secrets() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_python_process_runtime(tmp.path().to_path_buf());

    // With no secrets, stdin should have nothing (or empty) — action should still work
    let code = r#"
print("ok")
"#;

    let context = ExecutionContext {
        execution_id: 5,
        action_ref: "security.no_secrets".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
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
        max_stdout_bytes: 10 * 1024 * 1024,
        max_stderr_bytes: 10 * 1024 * 1024,
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
    assert_eq!(
        result.exit_code, 0,
        "Should handle empty secrets gracefully. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("ok"),
        "Should produce expected output. stdout: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_shell_empty_secrets() {
    let tmp = TempDir::new().unwrap();
    let runtime = make_shell_process_runtime(tmp.path().to_path_buf());

    let context = ExecutionContext {
        execution_id: 6,
        action_ref: "security.no_secrets".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
        secrets: HashMap::new(),
        timeout: Some(10),
        working_dir: None,
        entry_point: "shell".to_string(),
        code: Some(
            r#"
# Unset merged inputs should expand to empty string
if [ -z "$nonexistent" ] && [ -z "$PARAM_NONEXISTENT" ]; then
    echo "PASS: Missing input expands to empty string"
else
    echo "FAIL: Expected empty string for missing input"
    exit 1
fi
"#
            .to_string(),
        ),
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
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::default(),
        out_schema: None,
        cancel_token: None,
    };

    let result = runtime.execute(context).await.unwrap();
    assert!(
        result.is_success(),
        "Should handle empty secrets gracefully. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("PASS"),
        "Should pass. stdout: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_process_runtime_secrets_not_in_environ() {
    // Verify ProcessRuntime (used for all runtimes now) doesn't leak secrets to env
    let tmp = TempDir::new().unwrap();
    let pack_dir = tmp.path().join("testpack");
    let actions_dir = pack_dir.join("actions");
    std::fs::create_dir_all(&actions_dir).unwrap();

    // Write a script that dumps environment
    std::fs::write(
        actions_dir.join("check_env.sh"),
        r#"#!/bin/bash
if printenv | grep -q "SUPER_SECRET_VALUE"; then
    echo "FAIL: Secret leaked to environment"
    exit 1
fi
echo "PASS: No secrets in environment"
"#,
    )
    .unwrap();

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
        execution_id: 7,
        action_ref: "testpack.check_env".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
        secrets: {
            let mut s = HashMap::new();
            s.insert(
                "db_password".to_string(),
                serde_json::json!("SUPER_SECRET_VALUE"),
            );
            s
        },
        timeout: Some(10),
        working_dir: None,
        entry_point: "check_env.sh".to_string(),
        code: None,
        code_path: Some(actions_dir.join("check_env.sh")),
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
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::default(),
        out_schema: None,
        cancel_token: None,
    };

    let result = runtime.execute(context).await.unwrap();
    assert_eq!(
        result.exit_code, 0,
        "Check should pass. stdout: {}, stderr: {}",
        result.stdout, result.stderr
    );
    assert!(
        result.stdout.contains("PASS"),
        "Should confirm no secrets in env. stdout: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_python_process_runtime_secrets_not_in_environ() {
    // Same check but via ProcessRuntime with Python interpreter
    let tmp = TempDir::new().unwrap();
    let pack_dir = tmp.path().join("testpack");
    let actions_dir = pack_dir.join("actions");
    std::fs::create_dir_all(&actions_dir).unwrap();

    std::fs::write(
        actions_dir.join("check_env.py"),
        r#"
import os, json

env_dump = str(os.environ)
leaked = "TOP_SECRET_API_KEY" in env_dump
print(json.dumps({"leaked": leaked}))
"#,
    )
    .unwrap();

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
    let runtime = ProcessRuntime::new(
        "python".to_string(),
        config,
        tmp.path().to_path_buf(),
        tmp.path().join("runtime_envs"),
    );

    let context = ExecutionContext {
        execution_id: 8,
        action_ref: "testpack.check_env".to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
        secrets: {
            let mut s = HashMap::new();
            s.insert(
                "api_key".to_string(),
                serde_json::json!("TOP_SECRET_API_KEY"),
            );
            s
        },
        timeout: Some(10),
        working_dir: None,
        entry_point: "check_env.py".to_string(),
        code: None,
        code_path: Some(actions_dir.join("check_env.py")),
        runtime_name: Some("python".to_string()),
        runtime_config_override: None,
        runtime_env_dir_suffix: None,
        selected_runtime_version: None,
        max_stdout_bytes: 10 * 1024 * 1024,
        max_stderr_bytes: 10 * 1024 * 1024,
        stdout_log_path: None,
        stderr_log_path: None,
        stdout_log_writer: None,
        stderr_log_writer: None,
        parameter_delivery: attune_worker::runtime::ParameterDelivery::default(),
        parameter_format: attune_worker::runtime::ParameterFormat::default(),
        output_format: attune_worker::runtime::OutputFormat::Json,
        out_schema: None,
        cancel_token: None,
    };

    let result = runtime.execute(context).await.unwrap();
    assert_eq!(
        result.exit_code, 0,
        "Python env check should succeed. stderr: {}",
        result.stderr
    );

    let result_data = result.result.expect("Should have parsed JSON result");
    assert_eq!(
        result_data.get("leaked").unwrap(),
        &serde_json::json!(false),
        "SECURITY FAILURE: Secret leaked to Python process environment!"
    );
}
