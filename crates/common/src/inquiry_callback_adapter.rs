use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{Error, Result};

const MAX_ADAPTERS: usize = 16;
const MAX_ADAPTER_REF_LEN: usize = 64;
const MAX_CONSTRAINTS: usize = 32;
const MAX_POINTER_LEN: usize = 255;
const MAX_ALLOWED_VALUES: usize = 32;
const MAX_DELIVERY_ID_LEN: usize = 255;
const MAX_PROVIDER_ID_LEN: usize = 255;
const MAX_RESPONSE_HANDLE_LEN: usize = 96;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SensorInquiryCallbackAdapter {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub provider: String,
    pub subject_kind: String,
    pub request: InquiryCallbackRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InquiryCallbackRequest {
    pub delivery_id_pointer: String,
    pub tenant_pointer: String,
    pub external_subject_pointer: String,
    pub response_handle_pointer: String,
    #[serde(default)]
    pub required_values: BTreeMap<String, String>,
    #[serde(default)]
    pub allowed_values: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub required_array_lengths: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedInquiryCallbackSelection {
    pub provider: String,
    pub subject_kind: String,
    pub response_handle: String,
    pub tenant: String,
    pub external_subject: String,
}

impl SensorInquiryCallbackAdapter {
    pub fn validate(&self, adapter_ref: &str) -> Result<()> {
        validate_token(
            "inquiry callback adapter ref",
            adapter_ref,
            MAX_ADAPTER_REF_LEN,
            true,
        )?;
        validate_token("inquiry callback provider", &self.provider, 64, true)?;
        validate_token(
            "inquiry callback subject kind",
            &self.subject_kind,
            64,
            true,
        )?;

        let request = &self.request;
        for (field, pointer) in [
            ("delivery_id_pointer", &request.delivery_id_pointer),
            ("tenant_pointer", &request.tenant_pointer),
            (
                "external_subject_pointer",
                &request.external_subject_pointer,
            ),
            ("response_handle_pointer", &request.response_handle_pointer),
        ] {
            validate_pointer(field, pointer)?;
        }
        if request.required_values.len()
            + request.allowed_values.len()
            + request.required_array_lengths.len()
            > MAX_CONSTRAINTS
        {
            return Err(Error::validation(format!(
                "Inquiry callback adapter '{adapter_ref}' has too many request constraints"
            )));
        }
        for (pointer, expected) in &request.required_values {
            validate_pointer("required_values", pointer)?;
            validate_constraint_value(adapter_ref, expected)?;
        }
        for (pointer, allowed) in &request.allowed_values {
            validate_pointer("allowed_values", pointer)?;
            if allowed.is_empty() || allowed.len() > MAX_ALLOWED_VALUES {
                return Err(Error::validation(format!(
                    "Inquiry callback adapter '{adapter_ref}' must allow between 1 and {MAX_ALLOWED_VALUES} values per pointer"
                )));
            }
            for value in allowed {
                validate_constraint_value(adapter_ref, value)?;
            }
        }
        for (pointer, length) in &request.required_array_lengths {
            validate_pointer("required_array_lengths", pointer)?;
            if *length > MAX_ALLOWED_VALUES {
                return Err(Error::validation(format!(
                    "Inquiry callback adapter '{adapter_ref}' array length constraints cannot exceed {MAX_ALLOWED_VALUES}"
                )));
            }
        }
        Ok(())
    }

    pub fn normalize(
        &self,
        raw: &JsonValue,
    ) -> std::result::Result<(String, NormalizedInquiryCallbackSelection), &'static str> {
        for (pointer, expected) in &self.request.required_values {
            if raw.pointer(pointer).and_then(JsonValue::as_str) != Some(expected.as_str()) {
                return Err("required value does not match");
            }
        }
        for (pointer, allowed) in &self.request.allowed_values {
            let Some(actual) = raw.pointer(pointer).and_then(JsonValue::as_str) else {
                return Err("allowed value is missing");
            };
            if !allowed.iter().any(|value| value == actual) {
                return Err("value is not allowed");
            }
        }
        for (pointer, expected) in &self.request.required_array_lengths {
            if raw
                .pointer(pointer)
                .and_then(JsonValue::as_array)
                .map(Vec::len)
                != Some(*expected)
            {
                return Err("array length does not match");
            }
        }

        let delivery_id =
            extract_bounded_string(raw, &self.request.delivery_id_pointer, MAX_DELIVERY_ID_LEN)?;
        let tenant =
            extract_bounded_string(raw, &self.request.tenant_pointer, MAX_PROVIDER_ID_LEN)?;
        let external_subject = extract_bounded_string(
            raw,
            &self.request.external_subject_pointer,
            MAX_PROVIDER_ID_LEN,
        )?;
        let response_handle = extract_bounded_string(
            raw,
            &self.request.response_handle_pointer,
            MAX_RESPONSE_HANDLE_LEN,
        )?;

        Ok((
            delivery_id,
            NormalizedInquiryCallbackSelection {
                provider: self.provider.clone(),
                subject_kind: self.subject_kind.clone(),
                response_handle,
                tenant,
                external_subject,
            },
        ))
    }
}

pub fn sensor_inquiry_callback_adapters(
    config: Option<&JsonValue>,
) -> Result<BTreeMap<String, SensorInquiryCallbackAdapter>> {
    let Some(value) = config.and_then(|config| config.get("inquiry_callback_adapters")) else {
        return Ok(BTreeMap::new());
    };
    let adapters: BTreeMap<String, SensorInquiryCallbackAdapter> =
        serde_json::from_value(value.clone()).map_err(|error| {
            Error::validation(format!(
                "Invalid sensor inquiry_callback_adapters configuration: {error}"
            ))
        })?;
    if adapters.len() > MAX_ADAPTERS {
        return Err(Error::validation(format!(
            "A sensor can define at most {MAX_ADAPTERS} inquiry callback adapters"
        )));
    }
    for (adapter_ref, adapter) in &adapters {
        adapter.validate(adapter_ref)?;
    }
    Ok(adapters)
}

pub fn sensor_has_callback_demand(config: Option<&JsonValue>) -> bool {
    sensor_inquiry_callback_adapters(config)
        .is_ok_and(|adapters| adapters.values().any(|adapter| adapter.enabled))
}

fn extract_bounded_string(
    raw: &JsonValue,
    pointer: &str,
    max_len: usize,
) -> std::result::Result<String, &'static str> {
    let Some(value) = raw.pointer(pointer).and_then(JsonValue::as_str) else {
        return Err("required string is missing");
    };
    if value.is_empty()
        || value.len() > max_len
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err("required string is invalid");
    }
    Ok(value.to_string())
}

fn validate_pointer(field: &str, pointer: &str) -> Result<()> {
    if !pointer.starts_with('/') || pointer.len() > MAX_POINTER_LEN {
        return Err(Error::validation(format!(
            "Inquiry callback {field} must be a JSON Pointer of at most {MAX_POINTER_LEN} bytes"
        )));
    }
    Ok(())
}

fn validate_constraint_value(adapter_ref: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_PROVIDER_ID_LEN || value.chars().any(char::is_control)
    {
        return Err(Error::validation(format!(
            "Inquiry callback adapter '{adapter_ref}' has an invalid constraint value"
        )));
    }
    Ok(())
}

fn validate_token(label: &str, value: &str, max_len: usize, allow_dot: bool) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= max_len
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '_'
                || character == '-'
                || (allow_dot && character == '.')
        });
    if !valid {
        return Err(Error::validation(format!(
            "{label} must contain only lowercase ASCII letters, digits, '_',{} or '-' and be at most {max_len} bytes",
            if allow_dot { " '.'," } else { "" }
        )));
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn config() -> JsonValue {
        json!({
            "inquiry_callback_adapters": {
                "slack.socket_mode": {
                    "enabled": true,
                    "provider": "slack",
                    "subject_kind": "user",
                    "request": {
                        "delivery_id_pointer": "/envelope_id",
                        "tenant_pointer": "/payload/team/id",
                        "external_subject_pointer": "/payload/user/id",
                        "response_handle_pointer": "/payload/actions/0/value",
                        "required_values": {
                            "/type": "interactive",
                            "/payload/type": "block_actions"
                        },
                        "allowed_values": {
                            "/payload/actions/0/action_id": [
                                "attune.inquiry.response.v1.approve",
                                "attune.inquiry.response.v1.reject"
                            ]
                        },
                        "required_array_lengths": {"/payload/actions": 1}
                    }
                }
            }
        })
    }

    #[test]
    fn parses_and_normalizes_a_declared_adapter() {
        let adapters = sensor_inquiry_callback_adapters(Some(&config())).unwrap();
        let adapter = &adapters["slack.socket_mode"];
        let (delivery_id, selection) = adapter
            .normalize(&json!({
                "envelope_id": "env-1",
                "type": "interactive",
                "payload": {
                    "type": "block_actions",
                    "team": {"id": "T123"},
                    "user": {"id": "U456"},
                    "actions": [{
                        "action_id": "attune.inquiry.response.v1.approve",
                        "value": "opaque-handle"
                    }]
                }
            }))
            .unwrap();

        assert_eq!(delivery_id, "env-1");
        assert_eq!(selection.provider, "slack");
        assert_eq!(selection.subject_kind, "user");
        assert_eq!(selection.response_handle, "opaque-handle");
        assert!(sensor_has_callback_demand(Some(&config())));
    }

    #[test]
    fn rejects_unknown_fields_and_untrusted_values() {
        let mut unknown = config();
        unknown["inquiry_callback_adapters"]["slack.socket_mode"]["script"] = json!("run me");
        assert!(sensor_inquiry_callback_adapters(Some(&unknown)).is_err());

        let adapters = sensor_inquiry_callback_adapters(Some(&config())).unwrap();
        let invalid = json!({
            "envelope_id": "env-1",
            "type": "interactive",
            "payload": {
                "type": "block_actions",
                "team": {"id": "T123"},
                "user": {"id": "U456"},
                "actions": [{"action_id": "untrusted", "value": "opaque-handle"}]
            }
        });
        assert!(adapters["slack.socket_mode"].normalize(&invalid).is_err());
    }
}
