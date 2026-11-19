//! The existing declared action parameter encodings, shared by trusted
//! preparation and the isolated guest gate. Errors never include input values.
use crate::models::ParameterFormat;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum FormattingError {
    #[error("{0}")]
    InvalidAction(String),
    #[error("Failed to serialize parameters to {0}")]
    Serialization(&'static str),
}

/// Format parameters according to the specified format
pub fn format_parameters(
    parameters: &HashMap<String, JsonValue>,
    format: ParameterFormat,
) -> Result<String, FormattingError> {
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
) -> Result<HashMap<String, String>, FormattingError> {
    let mut flattened = HashMap::new();

    for (key, value) in params {
        // Names are record syntax, not quoted values. Validate every segment,
        // including names whose empty object would otherwise emit no record.
        if key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        {
            return Err(FormattingError::InvalidAction(
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
                FormattingError::InvalidAction(
                    "Cannot serialize dotenv parameter array".to_string(),
                )
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
) -> Result<(), FormattingError> {
    // Custom consumers do not share an escape decoder. Never emit physical
    // record separators or controls, and never include names/values in errors.
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(FormattingError::InvalidAction(
            "Dotenv parameter values cannot contain control characters or line separators; use JSON"
                .to_string(),
        ));
    }
    if records.insert(name, value).is_some() {
        return Err(FormattingError::InvalidAction(
            "Dotenv parameter names collide after dotted flattening; use JSON".to_string(),
        ));
    }
    Ok(())
}

/// Format parameters as dotenv (key='value')
/// Note: Parameter names are preserved as-is (case-sensitive)
/// Nested objects are flattened with dot notation (e.g., headers.Content-Type)
fn format_dotenv(parameters: &HashMap<String, JsonValue>) -> Result<String, FormattingError> {
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
fn format_json(parameters: &HashMap<String, JsonValue>) -> Result<String, FormattingError> {
    serde_json::to_string(parameters).map_err(|_| FormattingError::Serialization("JSON"))
}

/// Format parameters as YAML
fn format_yaml(parameters: &HashMap<String, JsonValue>) -> Result<String, FormattingError> {
    serde_yaml_ng::to_string(parameters).map_err(|_| FormattingError::Serialization("YAML"))
}
