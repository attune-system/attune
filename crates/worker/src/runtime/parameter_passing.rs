//! Parameter Passing Module
//!
//! Provides utilities for formatting and delivering action parameters
//! in different formats (dotenv, JSON, YAML) via different methods
//! (environment variables, stdin, temporary files).

use attune_common::child_process_environment::ChildProcessEnvironment;
use attune_common::models::{ParameterDelivery, ParameterFormat};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::debug;

use super::RuntimeError;

pub const ATTUNE_API_TOKEN_ENV: &str = "ATTUNE_API_TOKEN";

/// Runtime definitions cannot replace worker-owned Attune execution context.
pub fn is_reserved_runtime_env_var(name: &str) -> bool {
    attune_common::execution_env::is_reserved_execution_env_var(name)
}

/// Explicit execution values override runtime defaults, but never internal context.
pub fn merge_execution_environment(
    env: &mut HashMap<String, String>,
    execution_env: &HashMap<String, String>,
) -> Result<(), RuntimeError> {
    for (key, value) in execution_env {
        attune_common::execution_env::validate_execution_env_var(key, value)
            .map_err(|error| RuntimeError::InvalidAction(error.to_string()))?;
    }
    env.extend(execution_env.clone());
    Ok(())
}

/// Apply only the selected baseline and the explicit execution environment.
pub fn apply_runtime_environment(
    cmd: &mut Command,
    env: &HashMap<String, String>,
    baseline: &ChildProcessEnvironment,
) {
    baseline.apply(cmd.as_std_mut());
    for (key, value) in env {
        cmd.env(key, value);
    }
}

/// Format parameters according to the specified format
pub fn format_parameters(
    parameters: &HashMap<String, JsonValue>,
    format: ParameterFormat,
) -> Result<String, RuntimeError> {
    match format {
        ParameterFormat::Dotenv => format_dotenv(parameters),
        ParameterFormat::Json => format_json(parameters),
        ParameterFormat::Yaml => format_yaml(parameters),
    }
}

/// Flatten nested JSON objects into dotted notation for dotenv format
/// Example: {"headers": {"Content-Type": "application/json"}} becomes:
///   headers.Content-Type=application/json
fn flatten_parameters(
    params: &HashMap<String, JsonValue>,
    prefix: &str,
) -> Result<HashMap<String, String>, RuntimeError> {
    let mut flattened = HashMap::new();

    for (key, value) in params {
        // Names are record syntax, not quoted values. Validate every segment,
        // including names whose empty object would otherwise emit no record.
        if key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        {
            return Err(RuntimeError::InvalidAction(
                "Dotenv parameter names must use only ASCII letters, digits, _, . or -".to_string(),
            ));
        }
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", prefix, key)
        };

        let rendered = match value {
            JsonValue::Object(map) => {
                let nested_params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                for (name, value) in flatten_parameters(&nested_params, &full_key)? {
                    insert_dotenv_record(&mut flattened, name, value)?;
                }
                continue;
            }
            // Compact JSON escapes control characters inside arrays. Validate
            // the final record bytes below, rather than rejecting typed data
            // that has a safe representation.
            JsonValue::Array(_) => serde_json::to_string(value).map_err(|_| {
                RuntimeError::InvalidAction("Cannot serialize dotenv parameter array".to_string())
            })?,
            JsonValue::String(s) => s.clone(),
            JsonValue::Number(n) => n.to_string(),
            JsonValue::Bool(b) => b.to_string(),
            JsonValue::Null => String::new(),
        };
        insert_dotenv_record(&mut flattened, full_key, rendered)?;
    }

    Ok(flattened)
}

fn insert_dotenv_record(
    records: &mut HashMap<String, String>,
    name: String,
    value: String,
) -> Result<(), RuntimeError> {
    // Custom consumers do not share an escape decoder. Never emit physical
    // record separators or controls, and never include names/values in errors.
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(RuntimeError::InvalidAction(
            "Dotenv parameter values cannot contain control characters or line separators; use JSON"
                .to_string(),
        ));
    }
    if records.insert(name, value).is_some() {
        return Err(RuntimeError::InvalidAction(
            "Dotenv parameter names collide after dotted flattening; use JSON".to_string(),
        ));
    }
    Ok(())
}

/// Format parameters as dotenv (key='value')
/// Note: Parameter names are preserved as-is (case-sensitive)
/// Nested objects are flattened with dot notation (e.g., headers.Content-Type)
fn format_dotenv(parameters: &HashMap<String, JsonValue>) -> Result<String, RuntimeError> {
    let flattened = flatten_parameters(parameters, "")?;
    let mut lines = Vec::new();

    for (key, value) in flattened {
        // Escape single quotes in value
        let escaped_value = value.replace('\'', "'\\''");

        lines.push(format!("{}='{}'", key, escaped_value));
    }

    // Sort lines for consistent output
    lines.sort();

    Ok(lines.join("\n"))
}

/// Format parameters as JSON (compact, single-line)
///
/// Uses compact format so that actions reading stdin line-by-line
/// (e.g., `json.loads(sys.stdin.readline())`) receive the entire
/// JSON object on a single line.
fn format_json(parameters: &HashMap<String, JsonValue>) -> Result<String, RuntimeError> {
    serde_json::to_string(parameters).map_err(|e| {
        RuntimeError::ExecutionFailed(format!("Failed to serialize parameters to JSON: {}", e))
    })
}

/// Format parameters as YAML
fn format_yaml(parameters: &HashMap<String, JsonValue>) -> Result<String, RuntimeError> {
    serde_yaml_ng::to_string(parameters).map_err(|e| {
        RuntimeError::ExecutionFailed(format!("Failed to serialize parameters to YAML: {}", e))
    })
}

/// Create a temporary file with parameters
pub fn create_parameter_file(
    parameters: &HashMap<String, JsonValue>,
    format: ParameterFormat,
) -> Result<NamedTempFile, RuntimeError> {
    let formatted = format_parameters(parameters, format)?;

    let mut temp_file = NamedTempFile::new().map_err(RuntimeError::IoError)?;

    // Set restrictive permissions (owner read-only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = temp_file
            .as_file()
            .metadata()
            .map_err(RuntimeError::IoError)?
            .permissions();
        perms.set_mode(0o400); // Read-only for owner
        temp_file
            .as_file()
            .set_permissions(perms)
            .map_err(RuntimeError::IoError)?;
    }

    temp_file
        .write_all(formatted.as_bytes())
        .map_err(RuntimeError::IoError)?;

    temp_file.flush().map_err(RuntimeError::IoError)?;

    debug!(
        "Created parameter file at {:?} with format {:?}",
        temp_file.path(),
        format
    );

    Ok(temp_file)
}

/// Parameter delivery configuration
#[derive(Debug, Clone)]
pub struct ParameterDeliveryConfig {
    pub delivery: ParameterDelivery,
    pub format: ParameterFormat,
}

/// Build the action input document from declared parameters and key-backed
/// secrets only. Runtime API credentials and cache responses stay outside this
/// channel.
pub fn merge_parameters_and_secrets(
    parameters: &HashMap<String, JsonValue>,
    secrets: &HashMap<String, JsonValue>,
) -> HashMap<String, JsonValue> {
    let mut merged = parameters.clone();
    for (key, value) in secrets {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

/// Prepared parameters ready for execution
#[derive(Debug)]
pub enum PreparedParameters {
    /// Parameters are in environment variables
    Environment,
    /// Parameters will be passed via stdin
    Stdin(String),
    /// Parameters are in a temporary file
    File {
        path: PathBuf,
        #[allow(dead_code)]
        temp_file: NamedTempFile,
    },
}

impl PreparedParameters {
    /// Get the file path if this is file-based delivery
    pub fn file_path(&self) -> Option<&PathBuf> {
        match self {
            PreparedParameters::File { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Get the stdin content if this is stdin-based delivery
    pub fn stdin_content(&self) -> Option<&str> {
        match self {
            PreparedParameters::Stdin(content) => Some(content),
            _ => None,
        }
    }
}

/// Prepare parameters for delivery according to the specified method and format
pub fn prepare_parameters(
    parameters: &HashMap<String, JsonValue>,
    env: &mut HashMap<String, String>,
    config: ParameterDeliveryConfig,
) -> Result<PreparedParameters, RuntimeError> {
    match config.delivery {
        ParameterDelivery::Stdin => {
            // Format parameters for stdin
            let formatted = format_parameters(parameters, config.format)?;

            // Add environment variables to indicate delivery method
            env.insert("ATTUNE_PARAMETER_DELIVERY".to_string(), "stdin".to_string());
            env.insert(
                "ATTUNE_PARAMETER_FORMAT".to_string(),
                config.format.to_string(),
            );

            Ok(PreparedParameters::Stdin(formatted))
        }
        ParameterDelivery::File => {
            // Create temporary file with parameters
            let temp_file = create_parameter_file(parameters, config.format)?;
            let path = temp_file.path().to_path_buf();

            // Add environment variables to indicate delivery method and file location
            env.insert("ATTUNE_PARAMETER_DELIVERY".to_string(), "file".to_string());
            env.insert(
                "ATTUNE_PARAMETER_FORMAT".to_string(),
                config.format.to_string(),
            );
            env.insert(
                "ATTUNE_PARAMETER_FILE".to_string(),
                path.to_string_lossy().to_string(),
            );

            Ok(PreparedParameters::File { path, temp_file })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_environment_does_not_inherit_service_secrets() {
        const TEST_NAME: &str = "runtime::parameter_passing::tests::runtime_environment_does_not_inherit_service_secrets";
        const MARKER: &str = "ATTUNE_ENVIRONMENT_REPRO_CHILD";
        if std::env::var_os(MARKER).is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=4"])
                .env(MARKER, "1")
                .env("ATTUNE__SECURITY__JWT_SECRET", "dummy-jwt-sentinel")
                .env(
                    "ATTUNE__SECURITY__ENCRYPTION_KEY",
                    "dummy-encryption-sentinel",
                )
                .env("AWS_SECRET_ACCESS_KEY", "dummy-storage-sentinel")
                .env("SERVICE_ONLY_SENTINEL", "dummy-service-sentinel")
                .output()
                .await
                .unwrap();
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
                "The isolated environment fixture must run its exact test"
            );
            assert!(
                output.status.success(),
                "Isolated child environment check failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#"
            test "$EXPLICIT_VALUE" = action || exit 10
            test "$ATTUNE_API_TOKEN" = scoped-token || exit 11
            if test "${ATTUNE__SECURITY__JWT_SECRET+set}" = set; then printf 'JWT secret inherited\n'; fi
            if test "${ATTUNE__SECURITY__ENCRYPTION_KEY+set}" = set; then printf 'Encryption key inherited\n'; fi
            if test "${AWS_SECRET_ACCESS_KEY+set}" = set; then printf 'Storage credential inherited\n'; fi
            if test "${SERVICE_ONLY_SENTINEL+set}" = set; then printf 'Service sentinel inherited\n'; fi
        "#]);
        apply_runtime_environment(
            &mut command,
            &HashMap::from([
                ("EXPLICIT_VALUE".to_string(), "action".to_string()),
                ("ATTUNE_API_TOKEN".to_string(), "scoped-token".to_string()),
            ]),
            &ChildProcessEnvironment::default(),
        );
        let output = command.output().await.unwrap();
        assert!(
            output.status.success(),
            "Explicit action context must remain available"
        );
        assert!(
            output.stdout.is_empty(),
            "Service environment leaked into action process:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn caller_startup_probes_cannot_replace_fixed_actions() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("taken-over");
        let hook = root.path().join("startup");
        std::fs::write(&hook, format!("touch '{}'\n", marker.display())).unwrap();
        std::fs::write(
            root.path().join("sitecustomize.py"),
            format!("open({:?}, 'w').close()", marker.to_str().unwrap()),
        )
        .unwrap();
        let node_hook = root.path().join("hook.js");
        std::fs::write(
            &node_hook,
            format!(
                "require('fs').writeFileSync({:?}, '')",
                marker.to_str().unwrap()
            ),
        )
        .unwrap();
        let probes: HashMap<String, String> = HashMap::from([
            ("BASH_ENV".into(), hook.display().to_string()),
            ("ENV".into(), hook.display().to_string()),
            ("PYTHONPATH".into(), root.path().display().to_string()),
            (
                "NODE_OPTIONS".into(),
                format!("--require={}", node_hook.display()),
            ),
            ("LD_PRELOAD".into(), hook.display().to_string()),
            ("PATH".into(), root.path().display().to_string()),
            ("ATTUNE_API_TOKEN".into(), "caller-token".into()),
            ("APP_REGION".into(), "west".into()),
        ]);
        for (key, value) in &probes {
            if key != "APP_REGION" {
                assert!(
                    merge_execution_environment(
                        &mut HashMap::new(),
                        &HashMap::from([(key.clone(), value.clone())])
                    )
                    .is_err(),
                    "{key}"
                );
            }
        }
        for (binary, args) in [
            ("/bin/bash", vec!["-c", r#"printf '%s' "$APP_REGION""#]),
            (
                "/usr/bin/python3",
                vec!["-c", "import os; print(os.environ['APP_REGION'], end='')"],
            ),
            (
                "/usr/bin/node",
                vec!["-e", "process.stdout.write(process.env.APP_REGION)"],
            ),
        ] {
            if binary == "/usr/bin/node" && !std::path::Path::new(binary).exists() {
                eprintln!("Node subprocess probe unavailable: {binary} is not installed; policy rejection assertions still ran");
                continue;
            }
            let mut env = HashMap::new();
            assert!(merge_execution_environment(&mut env, &probes).is_err());
            assert!(env.is_empty());
            merge_execution_environment(
                &mut env,
                &HashMap::from([("APP_REGION".into(), "west".into())]),
            )
            .unwrap();
            assert_eq!(env.len(), 1);
            let mut cmd = Command::new(binary);
            cmd.args(args);
            apply_runtime_environment(&mut cmd, &env, &ChildProcessEnvironment::default());
            let output = cmd.output().await.expect("probe runtime must be installed");
            assert!(output.status.success(), "{binary}: {:?}", output.stderr);
            assert_eq!(output.stdout, b"west");
            assert!(
                output.stderr.is_empty(),
                "linker/startup probe reached {binary}"
            );
            assert!(!marker.exists());
        }
    }

    #[test]
    fn test_format_dotenv() {
        let mut params = HashMap::new();
        params.insert("message".to_string(), json!("Hello, World!"));
        params.insert("count".to_string(), json!(42));
        params.insert("enabled".to_string(), json!(true));

        let result = format_dotenv(&params).unwrap();

        assert!(result.contains("message='Hello, World!'"));
        assert!(result.contains("count='42'"));
        assert!(result.contains("enabled='true'"));
    }

    #[test]
    fn test_format_dotenv_nested_objects() {
        let mut params = HashMap::new();
        params.insert("url".to_string(), json!("https://example.com"));
        params.insert(
            "headers".to_string(),
            json!({"Content-Type": "application/json", "Authorization": "Bearer token"}),
        );
        params.insert(
            "query_params".to_string(),
            json!({"page": "1", "size": "10"}),
        );

        let result = format_dotenv(&params).unwrap();

        // Check that nested objects are flattened with dot notation
        assert!(result.contains("headers.Content-Type='application/json'"));
        assert!(result.contains("headers.Authorization='Bearer token'"));
        assert!(result.contains("query_params.page='1'"));
        assert!(result.contains("query_params.size='10'"));
        assert!(result.contains("url='https://example.com'"));
    }

    #[test]
    fn test_format_dotenv_empty_objects() {
        let mut params = HashMap::new();
        params.insert("url".to_string(), json!("https://example.com"));
        params.insert("headers".to_string(), json!({}));
        params.insert("query_params".to_string(), json!({}));

        let result = format_dotenv(&params).unwrap();

        // Empty objects should not produce any flattened keys
        assert!(result.contains("url='https://example.com'"));
        assert!(!result.contains("headers="));
        assert!(!result.contains("query_params="));
    }

    #[test]
    fn test_format_dotenv_escaping() {
        let mut params = HashMap::new();
        params.insert("message".to_string(), json!("It's a test"));

        let result = format_dotenv(&params).unwrap();

        assert!(result.contains("message='It'\\''s a test'"));
    }

    fn unsafe_dotenv_parameters() -> Vec<HashMap<String, JsonValue>> {
        let mut cases = Vec::new();
        for control in [
            '\n', '\r', '\0', '\t', '\u{7f}', '\u{85}', '\u{2028}', '\u{2029}',
        ] {
            let value = format!("private-sentinel{control}injected='private-sentinel'");
            cases.push(HashMap::from([("message".to_string(), json!(value))]));
            cases.push(HashMap::from([(
                "outer".to_string(),
                json!({"leaf": value}),
            )]));
        }
        for name in [
            "",
            "private-sentinel\nextra",
            "private-sentinel\rextra",
            "x=y",
            "x y",
            "#x",
            "x'",
            "x\0",
            "x\t",
            "x;exit",
            "x\\",
        ] {
            cases.push(HashMap::from([(
                name.to_string(),
                json!("private-sentinel"),
            )]));
            cases.push(HashMap::from([(
                "outer".to_string(),
                json!({name: "private-sentinel"}),
            )]));
            // An empty object must not hide an invalid segment.
            cases.push(HashMap::from([(name.to_string(), json!({}))]));
            // Nor may a non-string value hide an invalid record name.
            for value in [
                json!(7),
                json!(true),
                json!(null),
                json!(["private-sentinel"]),
            ] {
                cases.push(HashMap::from([(name.to_string(), value)]));
            }
        }
        cases.push(HashMap::from([
            ("outer.leaf".to_string(), json!("private-sentinel")),
            ("outer".to_string(), json!({"leaf": "private-sentinel"})),
        ]));
        // JSON escapes ASCII controls in arrays, but retains these separators.
        cases.push(HashMap::from([(
            "items".to_string(),
            json!([{"leaf": "private-sentinel\u{2028}extra"}]),
        )]));
        cases
    }

    #[test]
    fn dotenv_denials_are_value_free_and_do_not_mutate_delivery_environment() {
        for parameters in unsafe_dotenv_parameters() {
            for delivery in [ParameterDelivery::Stdin, ParameterDelivery::File] {
                let original = HashMap::from([("EXPLICIT".to_string(), "unchanged".to_string())]);
                let mut env = original.clone();
                let error = prepare_parameters(
                    &parameters,
                    &mut env,
                    ParameterDeliveryConfig {
                        delivery,
                        format: ParameterFormat::Dotenv,
                    },
                )
                .unwrap_err();
                assert!(matches!(error, RuntimeError::InvalidAction(_)));
                assert!(!format!("{error:?} {error}").contains("private-sentinel"));
                assert_eq!(env, original);
            }
        }
    }

    // Exercise the actual configurable interpreter, preparation and spawn path.
    // The first instruction leaves an owned marker: a denial must prevent even
    // that instruction, not merely prevent parsing the injected record.
    #[cfg(unix)]
    #[tokio::test]
    async fn dotenv_custom_runtime_rejects_injection_before_action_spawn() {
        use crate::runtime::process::ProcessRuntime;
        use crate::runtime::{ExecutionContext, Runtime};
        use attune_common::models::runtime::{InterpreterConfig, RuntimeExecutionConfig};

        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("spawned");
        let script = root.path().join("consumer.py");
        std::fs::write(
            &script,
            r#"
import json, os, pathlib, sys
pathlib.Path(os.environ['SPAWN_MARKER']).write_text('spawned')
if os.environ['ATTUNE_PARAMETER_DELIVERY'] == 'file':
    payload = pathlib.Path(os.environ['ATTUNE_PARAMETER_FILE']).read_text()
else:
    payload = sys.stdin.read()
if os.environ['ATTUNE_PARAMETER_FORMAT'] == 'json':
    print(json.dumps(json.loads(payload)))
else:
    # A line-oriented data consumer; deliberately no shell evaluation.
    print(json.dumps(payload.splitlines()))
"#,
        )
        .unwrap();
        let config = RuntimeExecutionConfig {
            interpreter: InterpreterConfig {
                binary: "python3".to_string(),
                args: vec![],
                file_extension: Some(".py".to_string()),
            },
            inline_execution: Default::default(),
            environment: None,
            dependencies: None,
            env_vars: HashMap::new(),
        };
        let runtime = ProcessRuntime::new(
            "custom".to_string(),
            config,
            root.path().to_path_buf(),
            root.path().join("envs"),
        );
        for delivery in [ParameterDelivery::Stdin, ParameterDelivery::File] {
            let make_context = || {
                let mut context =
                    ExecutionContext::test_context("fixture.consume".to_string(), None);
                context.code_path = Some(script.clone());
                context.working_dir = Some(root.path().to_path_buf());
                context.parameter_delivery = delivery;
                context.parameter_format = ParameterFormat::Dotenv;
                context.env.insert(
                    "SPAWN_MARKER".to_string(),
                    marker.to_str().unwrap().to_string(),
                );
                context
            };
            for parameters in unsafe_dotenv_parameters() {
                // Secret merging must not create a bypass of the same check.
                for as_secret in [false, true] {
                    let mut context = make_context();
                    if as_secret {
                        context.secrets = parameters.clone();
                    } else {
                        context.parameters = parameters.clone();
                    }
                    let error = runtime.execute(context).await.unwrap_err();
                    assert!(matches!(error, RuntimeError::InvalidAction(_)));
                    assert!(!format!("{error:?} {error}").contains("private-sentinel"));
                    assert!(!marker.exists(), "rejected input spawned the action");
                }
            }
            let mut context = make_context();
            context.parameters = HashMap::from([
                ("message".to_string(), json!("It's $(data)=only")),
                (
                    "headers".to_string(),
                    json!({"Content-Type": "application/json"}),
                ),
                (
                    "items".to_string(),
                    json!(["line\nnext\r", {"x=y\n": true}, null, 3]),
                ),
                ("count".to_string(), json!(42)),
                ("enabled".to_string(), json!(true)),
                ("empty".to_string(), json!(null)),
            ]);
            let valid_parameters = context.parameters.clone();
            let output = runtime.execute(context).await.unwrap();
            assert_eq!(output.exit_code, 0, "{}", output.stderr);
            assert!(marker.exists());
            let records = output.result.unwrap();
            let records = records.as_array().unwrap();
            assert_eq!(records.len(), 6, "one record per flattened field");
            assert!(records.contains(&json!("message='It'\\''s $(data)=only'")));
            assert!(records.contains(&json!("headers.Content-Type='application/json'")));
            assert!(records.contains(&json!(
                "items='[\"line\\nnext\\r\",{\"x=y\\n\":true},null,3]'"
            )));
            std::fs::remove_file(&marker).unwrap();

            // Default typed JSON retains multiline/control-bearing text, keys,
            // arrays, objects and scalar types; dotenv restrictions stay local.
            assert_eq!(ParameterFormat::default(), ParameterFormat::Json);
            let mut context = make_context();
            context.parameters = valid_parameters;
            context.parameter_format = ParameterFormat::default();
            context
                .parameters
                .insert("x=y\n".to_string(), json!({"leaf": "a\n\r\0b"}));
            let expected = serde_json::to_value(&context.parameters).unwrap();
            let output = runtime.execute(context).await.unwrap();
            assert_eq!(output.exit_code, 0, "{}", output.stderr);
            assert_eq!(output.result.unwrap(), expected);
            std::fs::remove_file(&marker).unwrap();
        }
        root.close().unwrap();
    }

    #[test]
    fn test_format_json() {
        let mut params = HashMap::new();
        params.insert("message".to_string(), json!("Hello"));
        params.insert("count".to_string(), json!(42));

        let result = format_json(&params).unwrap();
        let parsed: HashMap<String, JsonValue> = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed.get("message"), Some(&json!("Hello")));
        assert_eq!(parsed.get("count"), Some(&json!(42)));
    }

    #[test]
    fn test_cache_api_environment_is_not_serialized_into_secret_stdin() {
        let parameters = HashMap::from([("requested_id".to_string(), json!("account-42"))]);
        let secrets = HashMap::from([("upstream_token".to_string(), json!("secret-value"))]);
        let merged = merge_parameters_and_secrets(&parameters, &secrets);
        let mut env = HashMap::from([
            ("ATTUNE_API_TOKEN".to_string(), "execution-jwt".to_string()),
            (
                "CACHE_HTTP_RESPONSE".to_string(),
                r#"{"external_id":"account-42","value":"cache-data"}"#.to_string(),
            ),
        ]);

        let prepared = prepare_parameters(
            &merged,
            &mut env,
            ParameterDeliveryConfig {
                delivery: ParameterDelivery::Stdin,
                format: ParameterFormat::Json,
            },
        )
        .unwrap();
        let stdin: JsonValue =
            serde_json::from_str(prepared.stdin_content().expect("stdin delivery")).unwrap();

        assert_eq!(stdin["requested_id"], "account-42");
        assert_eq!(stdin["upstream_token"], "secret-value");
        assert!(stdin.get("ATTUNE_API_TOKEN").is_none());
        assert!(stdin.get("CACHE_HTTP_RESPONSE").is_none());
        assert!(!prepared
            .stdin_content()
            .expect("stdin delivery")
            .contains("cache-data"));
    }

    #[test]
    fn test_runtime_environment_removes_ambient_token_unless_explicitly_provided() {
        let mut without_token = Command::new("echo");
        apply_runtime_environment(
            &mut without_token,
            &HashMap::new(),
            &ChildProcessEnvironment::default(),
        );
        let removed = without_token
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == ATTUNE_API_TOKEN_ENV)
            .map(|(_, value)| value);
        assert_eq!(removed, None);

        let mut with_token = Command::new("echo");
        apply_runtime_environment(
            &mut with_token,
            &HashMap::from([(
                ATTUNE_API_TOKEN_ENV.to_string(),
                "execution-token".to_string(),
            )]),
            &ChildProcessEnvironment::default(),
        );
        let value = with_token
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == ATTUNE_API_TOKEN_ENV)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(value.as_deref(), Some("execution-token"));
    }

    #[test]
    fn test_attune_environment_names_are_reserved_for_the_worker() {
        assert!(is_reserved_runtime_env_var("ATTUNE_API_URL"));
        assert!(is_reserved_runtime_env_var("ATTUNE_API_TOKEN"));
        assert!(is_reserved_runtime_env_var("ATTUNE_PACK_REF"));
        assert!(!is_reserved_runtime_env_var("PYTHONPATH"));
    }

    #[test]
    fn execution_environment_overrides_runtime_values_but_not_reserved_values() {
        let mut env = HashMap::from([
            ("LOG_LEVEL".to_string(), "info".to_string()),
            ("ATTUNE_API_URL".to_string(), "http://internal".to_string()),
        ]);
        let execution_env = HashMap::from([
            ("LOG_LEVEL".to_string(), "debug".to_string()),
            ("REGION".to_string(), "eu-west-1".to_string()),
            ("ATTUNE_API_URL".to_string(), "http://untrusted".to_string()),
        ]);

        assert!(merge_execution_environment(&mut env, &execution_env).is_err());
        assert_eq!(env["LOG_LEVEL"], "info");
        let mut allowed = execution_env.clone();
        allowed.remove("ATTUNE_API_URL");
        merge_execution_environment(&mut env, &allowed).unwrap();

        assert_eq!(env["LOG_LEVEL"], "debug");
        assert_eq!(env["REGION"], "eu-west-1");
        assert_eq!(env["ATTUNE_API_URL"], "http://internal");
    }

    #[test]
    fn test_format_yaml() {
        let mut params = HashMap::new();
        params.insert("message".to_string(), json!("Hello"));
        params.insert("count".to_string(), json!(42));

        let result = format_yaml(&params).unwrap();

        assert!(result.contains("message:"));
        assert!(result.contains("Hello"));
        assert!(result.contains("count:"));
        assert!(result.contains("42"));
    }

    #[test]
    fn test_create_parameter_file() {
        let mut params = HashMap::new();
        params.insert("key".to_string(), json!("value"));

        let temp_file = create_parameter_file(&params, ParameterFormat::Json).unwrap();
        let content = std::fs::read_to_string(temp_file.path()).unwrap();

        assert!(content.contains("key"));
        assert!(content.contains("value"));
    }

    #[test]
    fn test_prepare_parameters_stdin() {
        let mut params = HashMap::new();
        params.insert("test".to_string(), json!("value"));

        let mut env = HashMap::new();
        let config = ParameterDeliveryConfig {
            delivery: ParameterDelivery::Stdin,
            format: ParameterFormat::Json,
        };

        let result = prepare_parameters(&params, &mut env, config).unwrap();

        assert!(matches!(result, PreparedParameters::Stdin(_)));
        assert_eq!(
            env.get("ATTUNE_PARAMETER_DELIVERY"),
            Some(&"stdin".to_string())
        );
        assert_eq!(
            env.get("ATTUNE_PARAMETER_FORMAT"),
            Some(&"json".to_string())
        );
    }

    #[test]
    fn test_prepare_parameters_file() {
        let mut params = HashMap::new();
        params.insert("test".to_string(), json!("value"));

        let mut env = HashMap::new();
        let config = ParameterDeliveryConfig {
            delivery: ParameterDelivery::File,
            format: ParameterFormat::Yaml,
        };

        let result = prepare_parameters(&params, &mut env, config).unwrap();

        assert!(matches!(result, PreparedParameters::File { .. }));
        assert_eq!(
            env.get("ATTUNE_PARAMETER_DELIVERY"),
            Some(&"file".to_string())
        );
        assert_eq!(
            env.get("ATTUNE_PARAMETER_FORMAT"),
            Some(&"yaml".to_string())
        );
        assert!(env.contains_key("ATTUNE_PARAMETER_FILE"));
    }
}
