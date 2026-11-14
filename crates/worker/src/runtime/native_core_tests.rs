use super::*;

#[tokio::test]
async fn native_directory_errors_happen_before_spawn() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("unknown"), b"never execute").unwrap();
    let mut context = ExecutionContext::test_context("test.directory".into(), None);
    context.code_path = Some(root.path().to_owned());
    let error = NativeRuntime::new().execute(context).await.unwrap_err();
    assert!(matches!(error, RuntimeError::InvalidAction(_)));
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn host_variant() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "linux-amd64"
    } else {
        "linux-arm64"
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[tokio::test]
async fn native_directory_dispatch_preserves_declared_caller_and_worker_environment() {
    let root = tempfile::tempdir().unwrap();
    // Exercise a real native executable, without a shell/PATH dispatcher.
    std::fs::copy("/usr/bin/env", root.path().join(host_variant())).unwrap();
    let mut context = ExecutionContext::test_context("test.directory_env".into(), None);
    context.code_path = Some(root.path().to_owned());
    context.runtime_name = Some("native".into());
    context.env.insert("ATTUNE_EXEC_ID".into(), "42".into());
    context
        .env
        .insert("APP_REGION".into(), "runtime-default".into());
    context
        .execution_env
        .insert("APP_REGION".into(), "west".into());
    let declaration = serde_json::json!({"permitted_caller_env":["APP_REGION"]});
    assert!(!attune_common::execution_env::has_runtime_process_configuration(&declaration));
    attune_common::execution_env::validate_caller_environment(
        &declaration,
        &serde_json::to_value(&context.execution_env).unwrap(),
    )
    .unwrap();
    let result = NativeRuntime::new().execute(context).await.unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(result.stdout.lines().any(|line| line == "APP_REGION=west"));
    assert!(result
        .stdout
        .lines()
        .any(|line| line == "ATTUNE_EXEC_ID=42"));
    assert!(!result.stdout.contains("APP_REGION=runtime-default"));
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[tokio::test]
async fn native_directory_rejects_startup_environment_before_spawn() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("variants");
    std::fs::create_dir(&directory).unwrap();
    let executable = directory.join(host_variant());
    // An owned marker fixture proves that even a valid, executable directory
    // cannot cause effects when stored caller overrides violate startup policy.
    std::fs::write(&executable, b"#!/bin/sh\nprintf started > started\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        attune_common::native_entrypoint::resolve(
            &directory,
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .unwrap(),
        executable
    );
    let mut context = ExecutionContext::test_context("test.directory_env".into(), None);
    context.code_path = Some(directory);
    context.runtime_name = Some("native".into());
    context
        .execution_env
        .insert("BASH_ENV".into(), "private-caller-value".into());
    let declaration = serde_json::json!({"permitted_caller_env":["APP_REGION"]});
    assert!(attune_common::execution_env::validate_caller_environment(
        &declaration,
        &serde_json::to_value(&context.execution_env).unwrap(),
    )
    .is_err());
    // Runtime validation independently rejects invalid stored overrides too.
    let error = NativeRuntime::with_work_dir(root.path().to_owned())
        .execute(context)
        .await
        .unwrap_err();
    assert!(matches!(error, RuntimeError::InvalidAction(_)));
    assert!(error.to_string().contains("BASH_ENV"));
    assert!(!error.to_string().contains("private-caller-value"));
    assert!(!root.path().join("started").exists());
}
