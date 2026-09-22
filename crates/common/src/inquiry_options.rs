use std::collections::HashSet;

use jsonschema::Validator;
use serde_json::Value;

use crate::models::{inquiry::InquiryResponseOption, JsonSchema};
use crate::{Error, Result};

pub const MAX_RESPONSE_OPTIONS: usize = 25;
pub const MAX_RESPONSE_OPTION_REF_LENGTH: usize = 64;
pub const MAX_RESPONSE_OPTION_LABEL_LENGTH: usize = 255;
pub const MAX_RESPONSE_OPTIONS_SERIALIZED_SIZE: usize = 64 * 1024;

pub fn validate_response_options(
    response_schema: Option<&JsonSchema>,
    options: &[InquiryResponseOption],
) -> Result<()> {
    if options.is_empty() {
        return Err(Error::Validation(
            "inquiries require at least one response option".to_string(),
        ));
    }
    if options.len() > MAX_RESPONSE_OPTIONS {
        return Err(Error::Validation(format!(
            "inquiries support at most {MAX_RESPONSE_OPTIONS} response options"
        )));
    }
    let serialized_size = serde_json::to_vec(options)
        .map_err(|error| Error::Validation(format!("invalid inquiry response options: {error}")))?
        .len();
    if serialized_size > MAX_RESPONSE_OPTIONS_SERIALIZED_SIZE {
        return Err(Error::Validation(format!(
            "inquiry response options exceed {MAX_RESPONSE_OPTIONS_SERIALIZED_SIZE} bytes"
        )));
    }

    let validator = response_schema
        .map(validate_flat_schema)
        .transpose()?
        .map(flat_to_json_schema)
        .map(|schema| {
            Validator::new(&schema).map_err(|error| {
                Error::Validation(format!("invalid inquiry response schema: {error}"))
            })
        })
        .transpose()?;
    let mut refs = HashSet::with_capacity(options.len());
    for option in options {
        if option.r#ref.is_empty()
            || option.r#ref.len() > MAX_RESPONSE_OPTION_REF_LENGTH
            || !option.r#ref.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
        {
            return Err(Error::Validation(format!(
                "inquiry response option ref '{}' must be a lowercase token of at most {MAX_RESPONSE_OPTION_REF_LENGTH} characters",
                option.r#ref
            )));
        }
        if !refs.insert(option.r#ref.as_str()) {
            return Err(Error::Validation(format!(
                "duplicate inquiry response option ref '{}'",
                option.r#ref
            )));
        }
        let label = option.label.trim();
        if label.is_empty() || label.len() > MAX_RESPONSE_OPTION_LABEL_LENGTH {
            return Err(Error::Validation(format!(
                "inquiry response option '{}' label must contain at most {MAX_RESPONSE_OPTION_LABEL_LENGTH} characters",
                option.r#ref
            )));
        }
        if !option.response.is_object() {
            return Err(Error::Validation(format!(
                "inquiry response option '{}' response must be an object",
                option.r#ref
            )));
        }
        if let Some(validator) = &validator {
            let errors: Vec<String> = validator
                .iter_errors(&option.response)
                .map(|error| error.to_string())
                .collect();
            if !errors.is_empty() {
                return Err(Error::Validation(format!(
                    "invalid response for inquiry option '{}': {}",
                    option.r#ref,
                    errors.join(", ")
                )));
            }
        }
    }
    Ok(())
}

fn validate_flat_schema(schema: &JsonSchema) -> Result<&JsonSchema> {
    let fields = schema.as_object().ok_or_else(|| {
        Error::Validation(
            "inquiry response_schema must use Attune's flat per-field format".to_string(),
        )
    })?;
    for (name, definition) in fields {
        let definition = definition.as_object().ok_or_else(|| {
            Error::Validation(format!(
                "inquiry response_schema field '{name}' must be an object"
            ))
        })?;
        if definition.get("type").and_then(Value::as_str).is_none() {
            return Err(Error::Validation(format!(
                "inquiry response_schema field '{name}' must declare a string type"
            )));
        }
    }
    Ok(schema)
}

fn flat_to_json_schema(flat: &Value) -> Value {
    let map = flat
        .as_object()
        .expect("flat inquiry schema was validated before conversion");

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (key, definition) in map {
        let Some(definition) = definition.as_object() else {
            continue;
        };
        let mut definition = definition.clone();
        let is_required = definition
            .remove("required")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        definition.remove("secret");
        definition.remove("position");
        if is_required {
            required.push(Value::String(key.clone()));
        }
        properties.insert(key.clone(), Value::Object(definition));
    }

    let mut schema = serde_json::Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), Value::Array(required));
    }
    Value::Object(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::inquiry::{InquiryResponseOption, InquiryResponseOptionStyle};
    use serde_json::json;

    fn option(r#ref: &str, response: Value) -> InquiryResponseOption {
        InquiryResponseOption {
            r#ref: r#ref.to_string(),
            label: r#ref.to_string(),
            style: InquiryResponseOptionStyle::Default,
            response,
        }
    }

    #[test]
    fn validates_unique_options_against_flat_schema() {
        let schema = json!({"approved": {"type": "boolean", "required": true}});
        assert!(validate_response_options(
            Some(&schema),
            &[option("approve", json!({"approved": true}))]
        )
        .is_ok());
        assert!(validate_response_options(
            Some(&schema),
            &[option("approve", json!({"approved": "yes"}))]
        )
        .is_err());
        assert!(validate_response_options(
            Some(&schema),
            &[
                option("approve", json!({"approved": true})),
                option("approve", json!({"approved": false})),
            ]
        )
        .is_err());
        assert!(validate_response_options(None, &[]).is_err());
    }

    #[test]
    fn rejects_raw_json_schema_and_malformed_flat_fields() {
        let options = [option("approve", json!({"approved": true}))];
        assert!(validate_response_options(
            Some(&json!({
                "type": "object",
                "properties": {"approved": {"type": "boolean"}}
            })),
            &options
        )
        .is_err());
        assert!(
            validate_response_options(Some(&json!({"approved": "boolean"})), &options).is_err()
        );
        assert!(validate_response_options(
            Some(&json!({"approved": {"required": true}})),
            &options
        )
        .is_err());
    }
}
