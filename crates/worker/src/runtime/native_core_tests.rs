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
