//! Parameter Passing Module
//!
//! Provides utilities for formatting and delivering action parameters
//! in different formats (dotenv, JSON, YAML) via different methods
//! (environment variables, stdin, temporary files).

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
    name.starts_with("ATTUNE_")
}

/// Apply the explicit execution environment while preventing an API token
/// inherited by the worker process from leaking into a no-permission action.
pub fn apply_runtime_environment(cmd: &mut Command, env: &HashMap<String, String>) {
    cmd.env_remove(ATTUNE_API_TOKEN_ENV);
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
) -> HashMap<String, String> {
    let mut flattened = HashMap::new();

    for (key, value) in params {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", prefix, key)
        };

        match value {
            JsonValue::Object(map) => {
                // Recursively flatten nested objects
                let nested_params: HashMap<String, JsonValue> =
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let nested_flattened = flatten_parameters(&nested_params, &full_key);
                flattened.extend(nested_flattened);
            }
            JsonValue::Array(_) => {
                // Arrays are serialized as JSON strings
                flattened.insert(full_key, serde_json::to_string(value).unwrap_or_default());
            }
            JsonValue::String(s) => {
                flattened.insert(full_key, s.clone());
            }
            JsonValue::Number(n) => {
                flattened.insert(full_key, n.to_string());
            }
            JsonValue::Bool(b) => {
                flattened.insert(full_key, b.to_string());
            }
            JsonValue::Null => {
                flattened.insert(full_key, String::new());
            }
        }
    }

    flattened
}

/// Format parameters as dotenv (key='value')
/// Note: Parameter names are preserved as-is (case-sensitive)
/// Nested objects are flattened with dot notation (e.g., headers.Content-Type)
fn format_dotenv(parameters: &HashMap<String, JsonValue>) -> Result<String, RuntimeError> {
    let flattened = flatten_parameters(parameters, "");
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
        apply_runtime_environment(&mut without_token, &HashMap::new());
        let removed = without_token
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == ATTUNE_API_TOKEN_ENV)
            .map(|(_, value)| value);
        assert_eq!(removed, Some(None));

        let mut with_token = Command::new("echo");
        apply_runtime_environment(
            &mut with_token,
            &HashMap::from([(
                ATTUNE_API_TOKEN_ENV.to_string(),
                "execution-token".to_string(),
            )]),
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
