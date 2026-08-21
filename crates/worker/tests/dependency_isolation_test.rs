//! Integration tests for runtime environment and dependency isolation
//!
//! Tests the end-to-end flow of creating isolated runtime environments
//! for packs using the ProcessRuntime configuration-driven approach.
//!
//! Environment directories are placed at:
//!   {runtime_envs_dir}/{pack_ref}/{runtime_name}
//! e.g., /tmp/.../runtime_envs/testpack/python
//! This keeps the pack directory clean and read-only.

use attune_common::models::runtime::{
    DependencyConfig, EnvironmentConfig, InlineExecutionConfig, InterpreterConfig,
    RuntimeExecutionConfig,
};
use attune_common::test_executor::{DiscoveryConfig, RunnerConfig, TestConfig, TestExecutor};
use attune_worker::runtime::process::ProcessRuntime;
use attune_worker::runtime::ExecutionContext;
use attune_worker::runtime::Runtime;
use attune_worker::runtime::{OutputFormat, ParameterDelivery, ParameterFormat};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

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
        env_vars: std::collections::HashMap::new(),
    }
}

fn make_shell_config() -> RuntimeExecutionConfig {
    RuntimeExecutionConfig {
        interpreter: InterpreterConfig {
            binary: "/bin/bash".to_string(),
            args: vec![],
            file_extension: Some(".sh".to_string()),
        },
        inline_execution: InlineExecutionConfig::default(),
        environment: None,
        dependencies: None,
        env_vars: std::collections::HashMap::new(),
    }
}

fn make_context(action_ref: &str, entry_point: &str, runtime_name: &str) -> ExecutionContext {
    ExecutionContext {
        execution_id: 1,
        action_ref: action_ref.to_string(),
        parameters: HashMap::new(),
        env: HashMap::new(),
        secrets: HashMap::new(),
        timeout: Some(30),
        working_dir: None,
        entry_point: entry_point.to_string(),
        code: None,
        code_path: None,
        runtime_name: Some(runtime_name.to_string()),
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

#[tokio::test]
async fn test_python_venv_creation_via_process_runtime() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Setup the pack environment (creates venv at external location)
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Failed to create venv environment");

    // Verify venv was created at the external runtime_envs location
    assert!(
        env_dir.exists(),
        "Virtualenv directory should exist at external location"
    );

    let venv_python = env_dir.join("bin").join("python3");
    assert!(
        venv_python.exists(),
        "Virtualenv python3 binary should exist"
    );

    // Verify pack directory was NOT modified
    assert!(
        !pack_dir.join(".venv").exists(),
        "Pack directory should not contain .venv — environments are external"
    );
}

#[tokio::test]
async fn test_venv_creation_is_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Create environment first time
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Failed to create environment");

    assert!(env_dir.exists());

    // Create environment second time — should succeed without error
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Second setup should succeed (idempotent)");

    assert!(env_dir.exists());
}

#[tokio::test]
async fn test_dependency_installation() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");

    // Write a requirements.txt with a simple, fast-to-install package
    std::fs::write(
        pack_dir.join("requirements.txt"),
        "pip>=21.0\n", // pip is already installed, so this is fast
    )
    .unwrap();

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Setup creates the venv and installs dependencies
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Failed to setup environment with dependencies");

    assert!(env_dir.exists());
}

#[tokio::test]
async fn test_unittest_uses_prepared_runtime_dependencies() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    let dependency_dir = pack_dir.join("test_dependency");
    let tests_dir = pack_dir.join("tests");
    std::fs::create_dir_all(&dependency_dir).unwrap();
    std::fs::create_dir_all(&tests_dir).unwrap();

    // Install a local package so this regression test is deterministic and offline.
    std::fs::write(
        dependency_dir.join("setup.py"),
        "from setuptools import setup\nsetup(name='test-dependency', version='0.1', py_modules=['test_dependency'])\n",
    )
    .unwrap();
    std::fs::write(
        dependency_dir.join("test_dependency.py"),
        "VALUE = 'installed from requirements'\n",
    )
    .unwrap();
    std::fs::write(
        pack_dir.join("requirements.txt"),
        format!("{}\n", dependency_dir.display()),
    )
    .unwrap();
    std::fs::write(
        tests_dir.join("test_dependency.py"),
        "import unittest\nfrom test_dependency import VALUE\n\nclass DependencyTest(unittest.TestCase):\n    def test_requirement_is_available(self):\n        self.assertEqual(VALUE, 'installed from requirements')\n",
    )
    .unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");
    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir.clone(),
        runtime_envs_dir,
    );
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Failed to prepare Python test environment");

    let test_config = TestConfig {
        enabled: true,
        discovery: DiscoveryConfig {
            method: "directory".to_string(),
            path: Some("tests".to_string()),
        },
        runners: HashMap::from([(
            "python".to_string(),
            RunnerConfig {
                r#type: "unittest".to_string(),
                entry_point: "tests/test_dependency.py".to_string(),
                timeout: Some(30),
                result_format: Some("simple".to_string()),
            },
        )]),
        result_format: None,
        result_path: None,
        min_pass_rate: Some(1.0),
        on_failure: Some("block".to_string()),
    };
    let result = TestExecutor::new(packs_base_dir)
        .execute_pack_tests_at_with_python_interpreter(
            &pack_dir,
            "testpack",
            "0.1.0",
            &test_config,
            Some(&env_dir.join("bin/python3")),
        )
        .await
        .expect("Failed to execute unittest");

    assert_eq!(result.status, "passed");
    assert_eq!(result.passed, 1);
}

#[tokio::test]
async fn test_no_environment_for_shell_runtime() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("shell");

    let runtime = ProcessRuntime::new(
        "shell".to_string(),
        make_shell_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Shell runtime has no environment config — should be a no-op
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Shell setup should succeed (no environment to create)");

    // No environment should exist
    assert!(!env_dir.exists());
    assert!(!pack_dir.join(".venv").exists());
    assert!(!pack_dir.join("node_modules").exists());
}

#[tokio::test]
async fn test_pack_has_dependencies_detection() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // No requirements.txt yet
    assert!(
        !runtime.pack_has_dependencies(&pack_dir),
        "Should not detect dependencies without manifest file"
    );

    // Create requirements.txt
    std::fs::write(pack_dir.join("requirements.txt"), "requests>=2.28.0\n").unwrap();

    assert!(
        runtime.pack_has_dependencies(&pack_dir),
        "Should detect dependencies when manifest file exists"
    );
}

#[tokio::test]
async fn test_environment_exists_detection() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // No venv yet — environment_exists uses pack_ref string
    assert!(
        !runtime.environment_exists("testpack"),
        "Environment should not exist before setup"
    );

    // Create the venv
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Failed to create environment");

    assert!(
        runtime.environment_exists("testpack"),
        "Environment should exist after setup"
    );
}

#[tokio::test]
async fn test_multiple_pack_isolation() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");

    let pack_a_dir = packs_base_dir.join("pack_a");
    let pack_b_dir = packs_base_dir.join("pack_b");
    std::fs::create_dir_all(&pack_a_dir).unwrap();
    std::fs::create_dir_all(&pack_b_dir).unwrap();

    let env_dir_a = runtime_envs_dir.join("pack_a").join("python");
    let env_dir_b = runtime_envs_dir.join("pack_b").join("python");

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Setup environments for two different packs
    runtime
        .setup_pack_environment(&pack_a_dir, &env_dir_a)
        .await
        .expect("Failed to setup pack_a");

    runtime
        .setup_pack_environment(&pack_b_dir, &env_dir_b)
        .await
        .expect("Failed to setup pack_b");

    // Each pack should have its own venv at the external location
    assert!(env_dir_a.exists(), "pack_a should have its own venv");
    assert!(env_dir_b.exists(), "pack_b should have its own venv");
    assert_ne!(
        env_dir_a, env_dir_b,
        "Venvs should be in different directories"
    );

    // Pack directories should remain clean
    assert!(
        !pack_a_dir.join(".venv").exists(),
        "pack_a dir should not contain .venv"
    );
    assert!(
        !pack_b_dir.join(".venv").exists(),
        "pack_b dir should not contain .venv"
    );
}

#[tokio::test]
async fn test_execute_python_action_with_venv() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    let actions_dir = pack_dir.join("actions");
    std::fs::create_dir_all(&actions_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");

    // Write a Python script
    std::fs::write(
        actions_dir.join("hello.py"),
        r#"
import sys
print(f"Python from: {sys.executable}")
print("Hello from venv action!")
"#,
    )
    .unwrap();

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Setup the venv first
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Failed to setup venv");

    // Now execute the action
    let mut context = make_context("testpack.hello", "hello.py", "python");
    context.code_path = Some(actions_dir.join("hello.py"));

    let result = runtime.execute(context).await.unwrap();

    assert_eq!(result.exit_code, 0, "Action should succeed");
    assert!(
        result.stdout.contains("Hello from venv action!"),
        "Should see output from action. Got: {}",
        result.stdout
    );
    // Verify it's using the venv Python (at external runtime_envs location)
    assert!(
        result.stdout.contains("runtime_envs"),
        "Should be using the venv python from external runtime_envs dir. Got: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_execute_shell_action_no_venv() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    let actions_dir = pack_dir.join("actions");
    std::fs::create_dir_all(&actions_dir).unwrap();

    std::fs::write(
        actions_dir.join("greet.sh"),
        "#!/bin/bash\necho 'Hello from shell!'",
    )
    .unwrap();

    let runtime = ProcessRuntime::new(
        "shell".to_string(),
        make_shell_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    let mut context = make_context("testpack.greet", "greet.sh", "shell");
    context.code_path = Some(actions_dir.join("greet.sh"));

    let result = runtime.execute(context).await.unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("Hello from shell!"));
}

#[tokio::test]
async fn test_working_directory_is_pack_dir() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    let actions_dir = pack_dir.join("actions");
    std::fs::create_dir_all(&actions_dir).unwrap();

    // Script that prints the working directory
    std::fs::write(actions_dir.join("cwd.sh"), "#!/bin/bash\npwd").unwrap();

    let runtime = ProcessRuntime::new(
        "shell".to_string(),
        make_shell_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    let mut context = make_context("testpack.cwd", "cwd.sh", "shell");
    context.code_path = Some(actions_dir.join("cwd.sh"));

    let result = runtime.execute(context).await.unwrap();

    assert_eq!(result.exit_code, 0);
    let output_path = std::fs::canonicalize(result.stdout.trim()).unwrap();
    let expected_path = std::fs::canonicalize(&pack_dir).unwrap();
    assert_eq!(
        output_path, expected_path,
        "Working directory should be the pack directory"
    );
}

#[tokio::test]
async fn test_interpreter_resolution_with_venv() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");

    let config = make_python_config();
    let runtime = ProcessRuntime::new(
        "python".to_string(),
        config.clone(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Before venv creation — should resolve to system python
    let interpreter = config.resolve_interpreter_with_env(&pack_dir, Some(&env_dir));
    assert_eq!(
        interpreter,
        PathBuf::from("python3"),
        "Without venv, should use system python"
    );

    // Create venv at external location
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Failed to create venv");

    // After venv creation — should resolve to venv python at external location
    let interpreter = config.resolve_interpreter_with_env(&pack_dir, Some(&env_dir));
    let expected_venv_python = env_dir.join("bin").join("python3");
    assert_eq!(
        interpreter, expected_venv_python,
        "With venv, should use venv python from external runtime_envs dir"
    );
}

#[tokio::test]
async fn test_skip_deps_install_without_manifest() {
    let temp_dir = TempDir::new().unwrap();
    let packs_base_dir = temp_dir.path().join("packs");
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");
    let pack_dir = packs_base_dir.join("testpack");
    std::fs::create_dir_all(&pack_dir).unwrap();

    let env_dir = runtime_envs_dir.join("testpack").join("python");

    // No requirements.txt — install_dependencies should be a no-op
    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        packs_base_dir,
        runtime_envs_dir,
    );

    // Setup should still create the venv but skip dependency installation
    runtime
        .setup_pack_environment(&pack_dir, &env_dir)
        .await
        .expect("Setup should succeed without manifest");

    assert!(
        env_dir.exists(),
        "Venv should still be created at external location"
    );
}

#[tokio::test]
async fn test_runtime_config_matches_file_extension() {
    let config = make_python_config();

    assert!(config.matches_file_extension(std::path::Path::new("hello.py")));
    assert!(config.matches_file_extension(std::path::Path::new(
        "/opt/attune/packs/mypack/actions/script.py"
    )));
    assert!(!config.matches_file_extension(std::path::Path::new("hello.sh")));
    assert!(!config.matches_file_extension(std::path::Path::new("hello.js")));

    let shell_config = make_shell_config();
    assert!(shell_config.matches_file_extension(std::path::Path::new("run.sh")));
    assert!(!shell_config.matches_file_extension(std::path::Path::new("run.py")));
}

#[tokio::test]
async fn test_dependency_spec_builder_still_works() {
    // The DependencySpec types are still available for generic use
    use attune_worker::runtime::DependencySpec;

    let spec = DependencySpec::new("python")
        .with_dependency("requests==2.28.0")
        .with_dependency("flask>=2.0.0")
        .with_version_range(Some("3.8".to_string()), Some("3.11".to_string()));

    assert_eq!(spec.runtime, "python");
    assert_eq!(spec.dependencies.len(), 2);
    assert!(spec.has_dependencies());
    assert_eq!(spec.min_version, Some("3.8".to_string()));
    assert_eq!(spec.max_version, Some("3.11".to_string()));
}

#[tokio::test]
async fn test_process_runtime_setup_and_validate() {
    let temp_dir = TempDir::new().unwrap();
    let runtime_envs_dir = temp_dir.path().join("runtime_envs");

    let shell_runtime = ProcessRuntime::new(
        "shell".to_string(),
        make_shell_config(),
        temp_dir.path().to_path_buf(),
        runtime_envs_dir.clone(),
    );

    // Setup and validate should succeed for shell
    shell_runtime.setup().await.unwrap();
    shell_runtime.validate().await.unwrap();

    let python_runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        temp_dir.path().to_path_buf(),
        runtime_envs_dir,
    );

    // Setup and validate should succeed for python (warns if not available)
    python_runtime.setup().await.unwrap();
    python_runtime.validate().await.unwrap();
}

#[tokio::test]
async fn test_can_execute_by_runtime_name() {
    let temp_dir = TempDir::new().unwrap();

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        temp_dir.path().to_path_buf(),
        temp_dir.path().join("runtime_envs"),
    );

    let context = make_context("mypack.hello", "hello.py", "python");
    assert!(runtime.can_execute(&context));

    let wrong_context = make_context("mypack.hello", "hello.py", "shell");
    assert!(!runtime.can_execute(&wrong_context));
}

#[tokio::test]
async fn test_can_execute_by_file_extension() {
    let temp_dir = TempDir::new().unwrap();

    let runtime = ProcessRuntime::new(
        "python".to_string(),
        make_python_config(),
        temp_dir.path().to_path_buf(),
        temp_dir.path().join("runtime_envs"),
    );

    let mut context = make_context("mypack.hello", "hello.py", "");
    context.runtime_name = None;
    context.code_path = Some(PathBuf::from("/tmp/packs/mypack/actions/hello.py"));
    assert!(runtime.can_execute(&context));

    context.code_path = Some(PathBuf::from("/tmp/packs/mypack/actions/hello.sh"));
    context.entry_point = "hello.sh".to_string();
    assert!(!runtime.can_execute(&context));
}
