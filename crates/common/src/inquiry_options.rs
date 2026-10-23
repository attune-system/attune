use std::collections::HashSet;

use crate::models::{inquiry::InquiryResponseOption, JsonSchema};
use crate::value_schema::{compile_inline, CompiledContract, SchemaContext};
use crate::{Error, Result};

pub const MAX_RESPONSE_OPTIONS: usize = 25;
pub const MAX_RESPONSE_OPTION_REF_LENGTH: usize = 64;
pub const MAX_RESPONSE_OPTION_LABEL_LENGTH: usize = 255;
pub const MAX_RESPONSE_OPTIONS_SERIALIZED_SIZE: usize = 64 * 1024;

pub fn validate_response_options(
    response_schema: Option<&JsonSchema>,
    options: &[InquiryResponseOption],
) -> Result<()> {
    let contract = response_schema
        .map(|schema| {
            compile_inline(
                "inquiry.response_schema",
                SchemaContext::FlatDocument,
                schema,
            )
            .map_err(|error| Error::Validation(format!("invalid inquiry response schema: {error}")))
        })
        .transpose()?;
    let validator = contract
        .as_ref()
        .map(CompiledContract::new)
        .transpose()
        .map_err(|error| Error::Validation(format!("invalid inquiry response schema: {error}")))?;
    validate_bound_response_options(validator.as_ref(), options)
}

pub fn validate_bound_response_options(
    validator: Option<&CompiledContract<'_>>,
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
        if let Some(validator) = validator {
            validator.validate(&option.response).map_err(|error| {
                Error::Validation(format!(
                    "invalid response for inquiry option '{}': {}",
                    option.r#ref, error
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::inquiry::{InquiryResponseOption, InquiryResponseOptionStyle};
    use serde_json::json;
    use serde_json::Value;

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
            Some(&json!({"approved": {"required": "invalid"}})),
            &options
        )
        .is_err());
    }

    #[test]
    fn options_use_nested_requiredness_and_strict_secret_safe_errors() {
        let schema = json!({"value": {"type": "object", "properties": {"token": {"type": "string", "minLength": 100, "secret": true}}, "required": ["token"]}});
        assert!(
            validate_response_options(Some(&schema), &[option("one", json!({"value": {}}))])
                .is_err()
        );
        let error = validate_response_options(
            Some(&schema),
            &[option("one", json!({"value": {"token": "PRIVATE_VALUE"}}))],
        )
        .unwrap_err();
        assert!(!error.to_string().contains("PRIVATE_VALUE"));
        assert!(validate_response_options(
            Some(&schema),
            &[option("one", json!({"value": {"token": "{{ value }}"}}))]
        )
        .is_err());
    }
}
