//! Explicit owned native archive acceptance lane; run with prebuilt musl fixtures.
use attune_common::models::{OutputFormat, ParameterDelivery, ParameterFormat};
use attune_common::pack_registry::{extract_archive, PackStorage, SafeExtractionLimits};
use attune_worker::runtime::{ExecutionContext, NativeRuntime, Runtime};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

fn context(action_ref: String, code: Option<String>) -> ExecutionContext {
    ExecutionContext {
        execution_id: 1,
        action_ref,
        parameters: HashMap::new(),
        env: HashMap::new(),
        execution_env: HashMap::new(),
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
        output_policy: attune_common::output_policy::OutputPolicy::default()
            .resolve(None, OutputFormat::default())
            .expect("public fixture output policy"),
        out_schema: None,
        cancel_token: None,
    }
}

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[tokio::main]
async fn main() {
    let binaries = std::env::var_os("ATTUNE_CORE_ACTION_BIN_ROOT")
        .map(PathBuf::from)
        .expect("set ATTUNE_CORE_ACTION_BIN_ROOT to core-actions/target");
    let owned = tempfile::tempdir().unwrap();
    let source = owned.path().join("source");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/core"),
        &source,
    );
    for action in ["echo", "sleep", "noop"] {
        let directory = source.join("actions/bin").join(action);
        std::fs::create_dir_all(&directory).unwrap();
        for (target, variant) in [
            ("x86_64-unknown-linux-musl", "linux-amd64"),
            ("aarch64-unknown-linux-musl", "linux-arm64"),
        ] {
            std::fs::copy(
                binaries
                    .join(target)
                    .join("release")
                    .join(format!("attune-core-{action}")),
                directory.join(variant),
            )
            .unwrap();
        }
    }
    let report = attune_common::pack_check::check_pack(&source);
    assert!(report.valid, "{report:?}");
    let version = serde_yaml_ng::from_str::<serde_json::Value>(
        &std::fs::read_to_string(source.join("pack.yaml")).unwrap(),
    )
    .unwrap()["version"]
        .as_str()
        .unwrap()
        .to_owned();
    let publisher = PackStorage::new(owned.path().join("publisher"));
    let published = publisher
        .publish_release(&source, "core", &version)
        .unwrap();
    let repeated = publisher
        .publish_release(&source, "core", &version)
        .unwrap();
    assert_eq!(published.digest, repeated.digest);
    let materialized = owned.path().join("worker");
    extract_archive(
        &published.archive_path,
        &materialized,
        SafeExtractionLimits::default(),
    )
    .unwrap();
    // Delete authoring and published source trees: execution must use only archive bytes.
    std::fs::remove_dir_all(&source).unwrap();
    assert!(publisher
        .remove_release_tree(&published.digest, published.pack_path.to_str().unwrap())
        .unwrap());
    std::fs::remove_dir_all(owned.path().join("publisher")).unwrap();
    let message = "雪 '\" \\ newline\nexit_code=9\nseconds=3600\n\n";
    let runtime = NativeRuntime::new();
    for (action, params) in [
        ("echo", serde_json::json!({"message":message})),
        ("noop", serde_json::json!({"message":message,"exit_code":0})),
        ("sleep", serde_json::json!({"message":message,"seconds":0})),
    ] {
        let mut context = context(format!("core.{action}"), None);
        context.code_path = Some(materialized.join("core/actions/bin").join(action));
        context.runtime_name = Some("native".into());
        context.parameters = serde_json::from_value(params).unwrap();
        context
            .env
            .insert("PATH".into(), "/no-interpreters-or-tools".into());
        context.parameter_delivery = attune_common::models::ParameterDelivery::Stdin;
        context.parameter_format = attune_common::models::ParameterFormat::Json;
        let result = runtime.execute(context).await.unwrap();
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert!(result.stdout.contains(message));
        if action == "echo" {
            assert_eq!(result.stdout, message);
        }
    }
    for cancelled in [false, true] {
        let mut context = context("core.sleep".into(), None);
        context.code_path = Some(materialized.join("core/actions/bin/sleep"));
        context.runtime_name = Some("native".into());
        context
            .parameters
            .insert("seconds".into(), serde_json::json!(60));
        context
            .env
            .insert("PATH".into(), "/no-interpreters-or-tools".into());
        context.parameter_delivery = attune_common::models::ParameterDelivery::Stdin;
        context.parameter_format = attune_common::models::ParameterFormat::Json;
        context.timeout = Some(1);
        if cancelled {
            let token = CancellationToken::new();
            token.cancel();
            context.cancel_token = Some(token);
        }
        let result = runtime.execute(context).await.unwrap();
        if cancelled {
            assert!(result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("cancelled")));
        } else {
            assert!(result.timed_out);
        }
    }
}
