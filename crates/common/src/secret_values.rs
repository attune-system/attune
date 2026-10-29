//! Helpers for tracking, redacting, encrypting, and restoring execution secrets.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value as JsonValue};

use crate::{crypto, Error, Result};

pub const ENTITY_EXECUTION_CONFIG: &str = "execution_config";
pub const ENTITY_EXECUTION_RESULT: &str = "execution_result";
pub const ENTITY_WORKFLOW_VARIABLES: &str = "workflow_variables";
pub const ENTITY_ENFORCEMENT_CONFIG: &str = "enforcement_config";
pub const ENTITY_EVENT_PAYLOAD: &str = "event_payload";
pub const ENTITY_EVENT_CONFIG: &str = "event_config";
pub const ENTITY_QUEUE_ITEM_PAYLOAD: &str = "queue_item_payload";
pub const ENTITY_INQUIRY_OPTIONS: &str = "inquiry_options";
pub const ENTITY_INQUIRY_RESPONSE: &str = "inquiry_response";

pub type JsonPointer = String;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SecretSource {
    Bound(crate::secret_provenance::SecretProvenance),
    PackConfig {
        pack_ref: Option<String>,
        path: JsonPointer,
    },
    Keystore {
        key_ref: Option<String>,
        path: JsonPointer,
    },
    ExecutionResult {
        execution_id: i64,
        path: JsonPointer,
    },
    WorkflowParameter {
        execution_id: i64,
        path: JsonPointer,
    },
    InquiryResponse {
        inquiry_id: i64,
        path: JsonPointer,
    },
    EventPayload {
        event_id: i64,
        path: JsonPointer,
    },
    QueueItem {
        queue_id: i64,
        pack_ref: Option<String>,
        queue_ref: Option<String>,
        item_id: Option<i64>,
        path: JsonPointer,
    },
    ParameterSchema {
        path: JsonPointer,
    },
    TriggerSchema {
        trigger_ref: Option<String>,
        section: &'static str,
        path: JsonPointer,
    },
}

impl SecretSource {
    pub fn provenance(&self) -> crate::secret_provenance::SecretProvenance {
        use crate::secret_provenance::{SecretOrigin, SecretProvenance};
        match self {
            SecretSource::Bound(provenance) => provenance.clone(),
            SecretSource::ExecutionResult { execution_id, path } => SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_RESULT.into(),
                    entity_id: *execution_id,
                    path: path.clone(),
                }],
                templates: Vec::new(),
            },
            SecretSource::WorkflowParameter { execution_id, path } => SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: *execution_id,
                    path: path.clone(),
                }],
                templates: Vec::new(),
            },
            SecretSource::InquiryResponse { inquiry_id, path } => SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_INQUIRY_RESPONSE.into(),
                    entity_id: *inquiry_id,
                    path: path.clone(),
                }],
                templates: Vec::new(),
            },
            SecretSource::EventPayload { event_id, path } => SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_EVENT_PAYLOAD.into(),
                    entity_id: *event_id,
                    path: path.clone(),
                }],
                templates: Vec::new(),
            },
            SecretSource::QueueItem {
                queue_id,
                queue_ref: Some(queue_ref),
                pack_ref,
                item_id: Some(item_id),
                path,
            } => SecretProvenance {
                origins: vec![SecretOrigin::QueueItem {
                    queue_id: *queue_id,
                    queue_ref: queue_ref.clone(),
                    pack_ref: pack_ref.clone(),
                    item_id: *item_id,
                    path: path.clone(),
                }],
                templates: Vec::new(),
            },
            _ => SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: self.source_kind().into(),
                    source_ref: self.source_ref(),
                }],
                templates: Vec::new(),
            },
        }
    }

    pub fn source_kind(&self) -> &'static str {
        match self {
            SecretSource::Bound(_) => "provenance",
            SecretSource::PackConfig { .. } => "pack_config",
            SecretSource::Keystore { .. } => "keystore",
            SecretSource::ExecutionResult { .. } => "execution_result",
            SecretSource::WorkflowParameter { .. } => "workflow_parameter",
            SecretSource::InquiryResponse { .. } => "inquiry_response",
            SecretSource::EventPayload { .. } => "event_payload",
            SecretSource::QueueItem { .. } => "queue_item",
            SecretSource::ParameterSchema { .. } => "parameter_schema",
            SecretSource::TriggerSchema { .. } => "trigger_schema",
        }
    }

    pub fn source_ref(&self) -> Option<String> {
        match self {
            SecretSource::Bound(provenance) => serde_json::to_string(provenance).ok(),
            SecretSource::PackConfig { pack_ref, path } => pack_ref
                .as_ref()
                .map(|pack_ref| format!("{pack_ref}:{path}"))
                .or_else(|| Some(path.clone())),
            SecretSource::Keystore { key_ref, path } => key_ref
                .as_ref()
                .map(|key_ref| format!("{key_ref}:{path}"))
                .or_else(|| Some(path.clone())),
            SecretSource::ExecutionResult { execution_id, path } => {
                Some(format!("{execution_id}:{path}"))
            }
            SecretSource::WorkflowParameter { execution_id, path } => {
                Some(format!("{execution_id}:{path}"))
            }
            SecretSource::InquiryResponse { inquiry_id, path } => {
                Some(format!("{inquiry_id}:{path}"))
            }
            SecretSource::EventPayload { event_id, path } => Some(format!("{event_id}:{path}")),
            SecretSource::QueueItem {
                queue_ref,
                item_id,
                path,
                ..
            } => Some(format!(
                "{}:{}:{path}",
                queue_ref.as_deref().unwrap_or(""),
                item_id.map(|id| id.to_string()).unwrap_or_default()
            )),
            SecretSource::ParameterSchema { path } => Some(path.clone()),
            SecretSource::TriggerSchema {
                trigger_ref,
                section,
                path,
            } => trigger_ref
                .as_ref()
                .map(|trigger_ref| format!("{trigger_ref}:{section}:{path}"))
                .or_else(|| Some(format!("{section}:{path}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecretPathSource {
    pub path: JsonPointer,
    pub source: SecretSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedJson {
    pub value: JsonValue,
    pub secret_paths: Vec<JsonPointer>,
    pub sources: Vec<SecretSource>,
    pub secret_path_sources: Vec<SecretPathSource>,
}

impl RenderedJson {
    pub fn plain(value: JsonValue) -> Self {
        Self {
            value,
            secret_paths: Vec::new(),
            sources: Vec::new(),
            secret_path_sources: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecretValueInput {
    pub json_path: String,
    pub value: JsonValue,
    pub source_kind: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedSecretValue {
    pub json_path: String,
    pub encrypted_value: JsonValue,
    pub encryption_key_hash: String,
    pub source_kind: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredSecretValue {
    pub json_path: String,
    pub encrypted_value: JsonValue,
    pub encryption_key_hash: Option<String>,
    pub source_kind: String,
    pub source_ref: Option<String>,
}

pub fn redaction_marker() -> JsonValue {
    json!({
        "$attune_secret": true,
        "redacted": true
    })
}

pub fn is_redaction_marker(value: &JsonValue) -> bool {
    value
        .as_object()
        .is_some_and(|obj| obj.get("$attune_secret").and_then(JsonValue::as_bool) == Some(true))
}

/// Metadata-only responses must never expose an unbound author's configuration.
/// A retained revision can replace this conservative projection at the API boundary.
pub fn configuration_metadata_projection(
    reference: &str,
    schema: &JsonValue,
    value: JsonValue,
) -> JsonValue {
    let contract = crate::value_schema::compile_inline(
        reference,
        crate::value_schema::SchemaContext::FlatDocument,
        schema,
    );
    let Ok(contract) = contract else {
        return redaction_marker();
    };
    let Ok(compiled) = crate::value_schema::CompiledContract::new(&contract) else {
        return redaction_marker();
    };
    merge_contract_secret_redactions(value, &[], &compiled)
        .map(|(public, _)| public)
        .unwrap_or_else(|_| redaction_marker())
}

pub fn redact_secret_parameters(
    value: JsonValue,
    schema: Option<&JsonValue>,
) -> (JsonValue, Vec<SecretValueInput>) {
    let path_sources = secret_paths_from_schema(schema)
        .into_iter()
        .map(|path| SecretPathSource {
            source: SecretSource::ParameterSchema { path: path.clone() },
            path,
        })
        .collect::<Vec<_>>();
    redact_secret_path_sources(value, &path_sources)
}

pub fn redact_secret_path_sources(
    mut value: JsonValue,
    path_sources: &[SecretPathSource],
) -> (JsonValue, Vec<SecretValueInput>) {
    let mut secrets = Vec::new();
    let paths = path_sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<BTreeSet<_>>();
    let roots = paths.iter().filter(|path| {
        !path
            .match_indices('/')
            .any(|(offset, _)| paths.contains(&path[..offset]))
    });
    for path in roots {
        if let Some(secret_value) = take_at_pointer(&mut value, path) {
            secrets.push(SecretValueInput {
                json_path: path.clone(),
                value: secret_value,
                source_kind: "provenance".to_string(),
                source_ref: Some(
                    serde_json::to_string(&provenance_for_path(path_sources, path))
                        .expect("secret provenance is serializable"),
                ),
            });
            set_at_pointer(&mut value, path, redaction_marker());
        }
    }

    (value, secrets)
}

pub fn redact_action_result(
    mut result: JsonValue,
    schema: Option<&JsonValue>,
    execution_id: i64,
    parameter_secret_paths: &[String],
) -> (JsonValue, Vec<SecretValueInput>) {
    use crate::secret_provenance::{SecretOrigin, SecretProvenance};
    let inherited = SecretProvenance {
        origins: parameter_secret_paths
            .iter()
            .map(|path| SecretOrigin::Entity {
                entity_type: ENTITY_EXECUTION_CONFIG.into(),
                entity_id: execution_id,
                path: path.clone(),
            })
            .collect(),
        templates: Vec::new(),
    };
    let mut secrets = Vec::new();
    if let Some(data) = result.get("data").cloned() {
        let (data, inputs) = redact_secret_parameters(data, schema);
        result["data"] = data;
        secrets = inputs
            .into_iter()
            .map(|mut input| {
                input.json_path = format!("/data{}", input.json_path);
                if !inherited.origins.is_empty() {
                    input.source_ref = Some(
                        serde_json::to_string(&inherited)
                            .expect("secret provenance is serializable"),
                    );
                }
                input
            })
            .collect();
    }
    if !inherited.origins.is_empty() || !secret_paths_from_schema(schema).is_empty() {
        for path in ["/stdout", "/error"] {
            if let Some(value) = result
                .pointer(path)
                .filter(|value| !value.is_null() && !is_redaction_marker(value))
                .cloned()
            {
                let provenance = if inherited.origins.is_empty() {
                    SecretSource::ParameterSchema { path: path.into() }.provenance()
                } else {
                    inherited.clone()
                };
                secrets.push(SecretValueInput {
                    json_path: path.into(),
                    value,
                    source_kind: "provenance".into(),
                    source_ref: Some(
                        serde_json::to_string(&provenance)
                            .expect("secret provenance is serializable"),
                    ),
                });
                set_at_pointer(&mut result, path, redaction_marker());
            }
        }
    }
    (result, secrets)
}

fn provenance_for_path(
    sources: &[SecretPathSource],
    path: &str,
) -> crate::secret_provenance::SecretProvenance {
    use crate::secret_provenance::{SecretOrigin, SecretProvenance};
    let mut provenance = SecretProvenance::default();
    for source in sources
        .iter()
        .filter(|source| {
            pointer_suffix(&source.path, path).is_some()
                || pointer_suffix(path, &source.path).is_some()
        })
        .map(|source| &source.source)
    {
        let next = match source {
            SecretSource::Bound(provenance) => provenance.clone(),
            SecretSource::ExecutionResult { execution_id, path } => SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_RESULT.into(),
                    entity_id: *execution_id,
                    path: path.clone(),
                }],
                templates: Vec::new(),
            },
            SecretSource::WorkflowParameter { execution_id, path } => SecretProvenance {
                origins: vec![SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: *execution_id,
                    path: path.clone(),
                }],
                templates: Vec::new(),
            },
            _ => SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: source.source_kind().into(),
                    source_ref: source.source_ref(),
                }],
                templates: Vec::new(),
            },
        };
        provenance.merge(next);
    }
    provenance
}

/// Output conformance is advisory. Secrets are not: malformed sensitive output
/// is encrypted as a whole rather than trusting traversal of an invalid shape.
pub fn redact_bound_action_result(
    mut result: JsonValue,
    output: Option<&crate::value_schema::Contract>,
    format: crate::models::OutputFormat,
    execution_id: i64,
    parameter_secret_paths: &[String],
    runtime_key_origins: &[crate::secret_provenance::KeyOrigin],
) -> Result<(JsonValue, Vec<SecretValueInput>)> {
    use crate::secret_provenance::{SecretOrigin, SecretProvenance};
    use crate::value_schema::{CompiledContract, ErrorCode, SchemaError};
    let contract = output
        .map(CompiledContract::new)
        .transpose()
        .map_err(|error| Error::validation(format!("Invalid bound output contract: {error}")))?;
    let declared_sensitive = contract
        .as_ref()
        .is_some_and(CompiledContract::declares_secrets);
    let inherited = SecretProvenance {
        origins: parameter_secret_paths
            .iter()
            .map(|path| SecretOrigin::Entity {
                entity_type: ENTITY_EXECUTION_CONFIG.into(),
                entity_id: execution_id,
                path: path.clone(),
            })
            .chain(runtime_key_origins.iter().cloned().map(SecretOrigin::Key))
            .collect(),
        templates: Vec::new(),
    };
    let local_origin = |path: &str| {
        if inherited.origins.is_empty() {
            SecretSource::ParameterSchema { path: path.into() }.provenance()
        } else {
            inherited.clone()
        }
    };
    let mut secrets = Vec::new();
    if let Some(contract) = &contract {
        let absent = JsonValue::Null;
        let data = result.get("data").unwrap_or(&absent);
        let validation = if format == crate::models::OutputFormat::Jsonl {
            match data.as_array() {
                Some(records) if records.len() <= output.unwrap().limits.max_literal_checks => {
                    records
                        .iter()
                        .try_for_each(|record| contract.validate(record))
                }
                Some(_) => Err(SchemaError {
                    code: ErrorCode::ResourceLimit,
                    path: "/data".into(),
                    source_ref: None,
                    reference_chain: Vec::new(),
                }),
                None => Err(SchemaError {
                    code: ErrorCode::InvalidValue,
                    path: "/data".into(),
                    source_ref: None,
                    reference_chain: Vec::new(),
                }),
            }
        } else {
            contract.validate(data)
        };
        let invalid = validation.is_err();
        if result.get("succeeded") == Some(&JsonValue::Bool(true)) {
            if let Err(error) = validation {
                result["output_validation_warning"] =
                    serde_json::json!({"code": error.code, "path": error.path});
            }
        }
        if let Some(data) = result.get("data").filter(|_| declared_sensitive).cloned() {
            if declared_sensitive && invalid {
                secrets.push(SecretValueInput {
                    json_path: "/data".into(),
                    value: data,
                    source_kind: "provenance".into(),
                    source_ref: Some(
                        serde_json::to_string(&local_origin("/data"))
                            .expect("serializable provenance"),
                    ),
                });
                result["data"] = redaction_marker();
            } else {
                let redact_record = |value: JsonValue,
                                     prefix: &str|
                 -> Result<(JsonValue, Vec<SecretValueInput>)> {
                    let (value, mut inputs) =
                        merge_contract_secret_redactions(value, &[], contract)?;
                    for input in &mut inputs {
                        input.json_path = format!("{prefix}{}", input.json_path);
                        if !inherited.origins.is_empty() {
                            input.source_ref = Some(
                                serde_json::to_string(&inherited).expect("serializable provenance"),
                            );
                        }
                    }
                    Ok((value, inputs))
                };
                if format == crate::models::OutputFormat::Jsonl {
                    if let Some(records) = data.as_array() {
                        let mut redacted = Vec::with_capacity(records.len());
                        for (index, record) in records.iter().enumerate() {
                            let (value, inputs) =
                                redact_record(record.clone(), &format!("/data/{index}"))?;
                            redacted.push(value);
                            secrets.extend(inputs);
                        }
                        result["data"] = JsonValue::Array(redacted);
                    }
                } else {
                    let (value, inputs) = redact_record(data, "/data")?;
                    result["data"] = value;
                    secrets.extend(inputs);
                }
            }
        }
    }
    // The queue acknowledgement is a duplicate of structured action output.
    // It cannot bypass encryption when the original path was sensitive.
    let acknowledgement_paths = secrets
        .iter()
        .filter_map(|input| {
            if input.json_path == "/data" {
                Some(("/queue_ack".to_string(), input.source_ref.clone()))
            } else {
                pointer_suffix("/data/queue_ack", &input.json_path)
                    .map(|suffix| (format!("/queue_ack{suffix}"), input.source_ref.clone()))
            }
        })
        .collect::<Vec<_>>();
    for (path, source_ref) in acknowledgement_paths {
        if let Some(value) = result.pointer(&path).cloned() {
            secrets.push(SecretValueInput {
                json_path: path.clone(),
                value,
                source_kind: "provenance".into(),
                source_ref,
            });
            set_at_pointer(&mut result, &path, redaction_marker());
        }
    }
    if declared_sensitive || !inherited.origins.is_empty() {
        for path in ["/stdout", "/error"] {
            if let Some(value) = result
                .pointer(path)
                .filter(|value| !value.is_null() && !is_redaction_marker(value))
                .cloned()
            {
                secrets.push(SecretValueInput {
                    json_path: path.into(),
                    value,
                    source_kind: "provenance".into(),
                    source_ref: Some(
                        serde_json::to_string(&local_origin(path))
                            .expect("serializable provenance"),
                    ),
                });
                set_at_pointer(&mut result, path, redaction_marker());
            }
        }
    }
    Ok((result, secrets))
}

/// A runtime read can flow to any output field, regardless of authored schema.
/// Encrypt complete payload fields, including acknowledgement duplicates, rather
/// than attempting value matching or trusting a pack's declared secret paths.
pub fn protect_runtime_read_result(
    original: &JsonValue,
    mut redacted: JsonValue,
    mut inputs: Vec<SecretValueInput>,
    reads: &[crate::secret_provenance::SecretOrigin],
) -> Result<(JsonValue, Vec<SecretValueInput>)> {
    if reads.is_empty() {
        return Ok((redacted, inputs));
    }
    let mut provenance = crate::secret_provenance::SecretProvenance {
        origins: reads.to_vec(),
        templates: Vec::new(),
    };
    for input in &inputs {
        if input.source_kind != "provenance" {
            return Err(Error::validation("Unknown output provenance"));
        }
        let parent = input
            .source_ref
            .as_deref()
            .ok_or_else(|| Error::validation("Missing output provenance"))?;
        provenance.merge(serde_json::from_str(parent)?);
    }
    let reference = serde_json::to_string(&provenance)?;
    if provenance.origins.len() > 256 || reference.len() > 1024 * 1024 {
        return Err(Error::validation(
            "Output read provenance exceeds its bound",
        ));
    }
    for field in ["data", "stdout", "stderr", "error", "queue_ack"] {
        if let Some(value) = original
            .get(field)
            .filter(|value| !value.is_null())
            .cloned()
        {
            let path = format!("/{field}");
            inputs.retain(|input| {
                input.json_path != path && !input.json_path.starts_with(&format!("{path}/"))
            });
            inputs.push(SecretValueInput {
                json_path: path,
                value,
                source_kind: "provenance".into(),
                source_ref: Some(reference.clone()),
            });
            redacted[field] = redaction_marker();
        }
    }
    Ok((redacted, inputs))
}

#[cfg(test)]
mod runtime_read_result_tests {
    use super::*;
    #[test]
    fn undeclared_data_errors_and_acknowledgements_inherit_reads_and_encrypt() {
        let original = json!({"succeeded":true,"data":{"public":"sdk-secret"},"stdout":"sdk-secret","stderr":"sdk-secret","error":"sdk-secret","queue_ack":{"value":"sdk-secret"}});
        let read = crate::repositories::runtime_read::RuntimeReadRepository::reference(42);
        let (redacted, inputs) = protect_runtime_read_result(
            &original,
            original.clone(),
            vec![],
            std::slice::from_ref(&read),
        )
        .unwrap();
        assert!(!redacted.to_string().contains("sdk-secret"));
        assert_eq!(inputs.len(), 5);
        for input in &inputs {
            let provenance: crate::secret_provenance::SecretProvenance =
                serde_json::from_str(input.source_ref.as_deref().unwrap()).unwrap();
            assert_eq!(provenance.origins, vec![read.clone()]);
        }
        let key = "runtime-read-result-test-key-32-characters";
        let prepared = prepare_secret_values(inputs, key).unwrap();
        assert!(!format!("{prepared:?}").contains("sdk-secret"));
    }
}

#[cfg(test)]
mod fingerprint_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const KEY: &str = "synthetic-recorded-secret-encryption-key";

    #[test]
    fn recorded_secret_fingerprint_survives_serialization_and_restoration() {
        let origin = serde_json::to_string(&crate::secret_provenance::SecretProvenance {
            origins: vec![crate::repositories::runtime_read::RuntimeReadRepository::reference(42)],
            templates: Vec::new(),
        })
        .unwrap();
        let prepared = prepare_secret_values(
            vec![SecretValueInput {
                json_path: "/credential".into(),
                value: json!({"token": "synthetic-only"}),
                source_kind: "provenance".into(),
                source_ref: Some(origin.clone()),
            }],
            KEY,
        )
        .unwrap();
        let serialized = serde_json::to_value(&prepared).unwrap();
        let decoded: Vec<PreparedSecretValue> = serde_json::from_value(serialized).unwrap();
        let mut stored: Vec<StoredSecretValue> = decoded
            .into_iter()
            .map(|field| StoredSecretValue {
                json_path: field.json_path,
                encrypted_value: field.encrypted_value,
                encryption_key_hash: Some(field.encryption_key_hash),
                source_kind: field.source_kind,
                source_ref: field.source_ref,
            })
            .collect();
        assert_eq!(stored[0].source_ref.as_deref(), Some(origin.as_str()));
        assert_eq!(
            stored[0].encryption_key_hash.as_deref(),
            Some(crypto::hash_encryption_key(KEY).as_str())
        );
        let redacted = json!({"credential": redaction_marker(), "public": 7});
        assert_eq!(
            restore_secret_values(redacted.clone(), &stored, KEY).unwrap(),
            json!({"credential": {"token": "synthetic-only"}, "public": 7})
        );
        assert!(restore_secret_values(
            redacted.clone(),
            &stored,
            "different-synthetic-server-secret-32"
        )
        .is_err());
        stored[0].encryption_key_hash = Some(crypto::hash_encryption_key(
            "different-synthetic-server-secret-32",
        ));
        assert!(restore_secret_values(redacted, &stored, KEY).is_err());
    }

    #[test]
    fn old_fingerprint_requires_explicit_conversion_but_ciphertext_is_unchanged() {
        let value = json!("historical-synthetic-secret");
        let encrypted = crypto::encrypt_json(&value, KEY).unwrap();
        let old_hash: String = Sha256::digest(KEY.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut stored = vec![StoredSecretValue {
            json_path: String::new(),
            encrypted_value: encrypted.clone(),
            encryption_key_hash: Some(old_hash),
            source_kind: "local".into(),
            source_ref: None,
        }];
        assert!(restore_secret_values(redaction_marker(), &stored, KEY).is_err());
        assert_eq!(crypto::decrypt_json(&encrypted, KEY).unwrap(), value);
        // Only the verifier changes. This is not a populated-store migration.
        stored[0].encryption_key_hash = Some(crypto::hash_encryption_key(KEY));
        assert_eq!(
            restore_secret_values(redaction_marker(), &stored, KEY).unwrap(),
            value
        );
        assert_eq!(stored[0].encrypted_value, encrypted);
    }
}

pub fn merge_schema_secret_redactions(
    value: JsonValue,
    existing_sources: &[SecretPathSource],
    schema: Option<&JsonValue>,
) -> (JsonValue, Vec<SecretValueInput>) {
    merge_secret_redactions(value, existing_sources, secret_paths_from_schema(schema))
}

/// Contract traversal includes inherited annotations, composition, and concrete
/// array paths. Provenance merging is the same as for existing explicit sources.
pub fn merge_contract_secret_redactions(
    value: JsonValue,
    existing_sources: &[SecretPathSource],
    contract: &crate::value_schema::CompiledContract,
) -> Result<(JsonValue, Vec<SecretValueInput>)> {
    let paths = contract
        .secret_paths(&value)
        .map_err(|error| Error::validation(format!("Invalid bound secret traversal: {error}")))?;
    Ok(merge_secret_redactions(value, existing_sources, paths))
}

pub fn validate_contract_secret_destination_paths(
    value: &JsonValue,
    contract: &crate::value_schema::CompiledContract,
    secret_paths: &[JsonPointer],
) -> Result<()> {
    let allowed = contract
        .secret_paths(value)
        .map_err(|error| Error::validation(format!("Invalid bound secret traversal: {error}")))?;
    if secret_paths
        .iter()
        .any(|path| !path_allowed_by_secret_schema(path, &allowed))
    {
        return Err(Error::validation(
            "Secret value cannot be assigned to a non-secret parameter path",
        ));
    }
    Ok(())
}

fn merge_secret_redactions(
    value: JsonValue,
    existing_sources: &[SecretPathSource],
    schema_paths: Vec<String>,
) -> (JsonValue, Vec<SecretValueInput>) {
    let mut path_sources = existing_sources.to_vec();
    let existing_paths = existing_sources
        .iter()
        .map(|source| source.path.as_str())
        .collect::<BTreeSet<_>>();

    for path in schema_paths {
        if !existing_paths.contains(path.as_str()) {
            path_sources.push(SecretPathSource {
                source: SecretSource::ParameterSchema { path: path.clone() },
                path,
            });
        }
    }

    redact_secret_path_sources(value, &path_sources)
}

pub fn secret_paths_from_schema(schema: Option<&JsonValue>) -> Vec<String> {
    let Some(schema) = schema else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    if let Some(map) = schema.as_object() {
        if map.contains_key("properties") {
            collect_json_schema_secret_paths(schema, "", &mut paths);
        } else {
            for (key, definition) in map {
                collect_flat_schema_secret_paths(
                    definition,
                    &format!("/{}", escape_pointer_segment(key)),
                    &mut paths,
                );
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

pub fn validate_secret_destination_paths(
    schema: Option<&JsonValue>,
    secret_paths: &[JsonPointer],
) -> Result<()> {
    if secret_paths.is_empty() {
        return Ok(());
    }

    let allowed = secret_paths_from_schema(schema);
    let rejected = secret_paths
        .iter()
        .filter(|path| !path_allowed_by_secret_schema(path, &allowed))
        .cloned()
        .collect::<Vec<_>>();

    if rejected.is_empty() {
        Ok(())
    } else {
        Err(Error::validation(format!(
            "Secret value cannot be assigned to non-secret parameter path(s): {}",
            rejected.join(", ")
        )))
    }
}

pub fn pointer_join(base: &str, suffix: &str) -> String {
    if base.is_empty() {
        suffix.to_string()
    } else if suffix.is_empty() {
        base.to_string()
    } else {
        format!("{base}{suffix}")
    }
}

pub fn pointer_from_dot_path(path: &str) -> String {
    let segments = path
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(escape_pointer_segment)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    }
}

pub fn pointer_suffix(full: &str, prefix: &str) -> Option<String> {
    if full == prefix {
        Some(String::new())
    } else {
        full.strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_prefix('/'))
            .map(|suffix| format!("/{suffix}"))
    }
}

pub fn prepare_secret_values(
    secrets: Vec<SecretValueInput>,
    encryption_key: &str,
) -> Result<Vec<PreparedSecretValue>> {
    let encryption_key_hash = crypto::hash_encryption_key(encryption_key);
    secrets
        .into_iter()
        .map(|secret| {
            let encrypted_value = crypto::encrypt_json(&secret.value, encryption_key)?;
            Ok(PreparedSecretValue {
                json_path: secret.json_path,
                encrypted_value,
                encryption_key_hash: encryption_key_hash.clone(),
                source_kind: secret.source_kind,
                source_ref: secret.source_ref,
            })
        })
        .collect()
}

pub fn restore_secret_values(
    mut redacted: JsonValue,
    secrets: &[StoredSecretValue],
    encryption_key: &str,
) -> Result<JsonValue> {
    let actual_hash = crypto::hash_encryption_key(encryption_key);
    for secret in secrets {
        if let Some(expected_hash) = &secret.encryption_key_hash {
            if expected_hash != &actual_hash {
                return Err(Error::encryption(
                    "Encryption key does not match stored secret values",
                ));
            }
        }
        let value = crypto::decrypt_json(&secret.encrypted_value, encryption_key)?;
        set_at_pointer(&mut redacted, &secret.json_path, value);
    }
    Ok(redacted)
}

pub fn splice_plain_secret_values(
    mut redacted: JsonValue,
    secrets: &[(String, JsonValue)],
) -> JsonValue {
    for (path, value) in secrets {
        set_at_pointer(&mut redacted, path, value.clone());
    }
    redacted
}

pub fn redacted_paths(value: &JsonValue) -> Vec<String> {
    let mut paths = Vec::new();
    collect_redacted_paths(value, "", &mut paths);
    paths
}

fn collect_redacted_paths(value: &JsonValue, path: &str, paths: &mut Vec<String>) {
    if is_redaction_marker(value) {
        paths.push(path.to_string());
        return;
    }

    match value {
        JsonValue::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{}/{}", path, escape_pointer_segment(key));
                collect_redacted_paths(child, &child_path, paths);
            }
        }
        JsonValue::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                let child_path = format!("{}/{}", path, idx);
                collect_redacted_paths(child, &child_path, paths);
            }
        }
        _ => {}
    }
}

fn collect_flat_schema_secret_paths(schema: &JsonValue, path: &str, paths: &mut Vec<String>) {
    let Some(map) = schema.as_object() else {
        return;
    };

    if map.get("secret").and_then(JsonValue::as_bool) == Some(true) {
        paths.push(path.to_string());
        return;
    }

    if let Some(properties) = map.get("properties").and_then(JsonValue::as_object) {
        for (key, child) in properties {
            collect_flat_schema_secret_paths(
                child,
                &format!("{}/{}", path, escape_pointer_segment(key)),
                paths,
            );
        }
    }
}

fn collect_json_schema_secret_paths(schema: &JsonValue, path: &str, paths: &mut Vec<String>) {
    let Some(map) = schema.as_object() else {
        return;
    };

    if map.get("secret").and_then(JsonValue::as_bool) == Some(true) && !path.is_empty() {
        paths.push(path.to_string());
        return;
    }

    if let Some(properties) = map.get("properties").and_then(JsonValue::as_object) {
        for (key, child) in properties {
            collect_json_schema_secret_paths(
                child,
                &format!("{}/{}", path, escape_pointer_segment(key)),
                paths,
            );
        }
    }
}

fn take_at_pointer(value: &mut JsonValue, pointer: &str) -> Option<JsonValue> {
    value.pointer(pointer).cloned()
}

fn set_at_pointer(value: &mut JsonValue, pointer: &str, replacement: JsonValue) {
    let segments = pointer_segments(pointer);
    set_at_segments(value, &segments, replacement);
}

fn set_at_segments(value: &mut JsonValue, segments: &[String], replacement: JsonValue) {
    if segments.is_empty() {
        *value = replacement;
        return;
    }

    match value {
        JsonValue::Object(map) => {
            let key = &segments[0];
            if segments.len() == 1 {
                map.insert(key.clone(), replacement);
            } else {
                let child = map
                    .entry(key.clone())
                    .or_insert_with(|| JsonValue::Object(Map::new()));
                set_at_segments(child, &segments[1..], replacement);
            }
        }
        JsonValue::Array(items) => {
            if let Ok(index) = segments[0].parse::<usize>() {
                if index < items.len() {
                    if segments.len() == 1 {
                        items[index] = replacement;
                    } else {
                        set_at_segments(&mut items[index], &segments[1..], replacement);
                    }
                }
            }
        }
        _ => {}
    }
}

fn pointer_segments(pointer: &str) -> Vec<String> {
    if pointer.is_empty() {
        return Vec::new();
    }
    pointer
        .trim_start_matches('/')
        .split('/')
        .map(unescape_pointer_segment)
        .collect()
}

fn path_allowed_by_secret_schema(path: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|allowed_path| {
        path == allowed_path
            || path
                .strip_prefix(allowed_path)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn unescape_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

#[cfg(test)]
mod tests {
    #[test]
    fn configuration_metadata_never_discloses_unbound_or_unresolved_secrets() {
        let projection = super::configuration_metadata_projection(
            "test.pack",
            &serde_json::json!({
                "credentials":{"type":"array","items":{"type":"object","properties":{"token":{"type":"string","secret":true}}}},
                "count":{"type":"integer"}
            }),
            serde_json::json!({"credentials":[{"token":"metadata-fixture"}],"count":0}),
        );
        assert!(!projection.to_string().contains("metadata-fixture"));
        assert_eq!(projection["count"], 0);
        assert!(super::is_redaction_marker(
            &projection["credentials"][0]["token"]
        ));
        assert!(super::is_redaction_marker(
            &super::configuration_metadata_projection(
                "test.pack",
                &serde_json::json!({"value":{"type":"external.secret"}}),
                serde_json::json!({"value":"unresolved-fixture"})
            )
        ));
    }
    use super::*;

    #[test]
    fn enclosing_secret_keeps_all_nested_origins_regardless_of_source_order() {
        use crate::{
            models::OwnerType,
            secret_provenance::{KeyOrigin, SecretProvenance},
        };
        let source = |path: &str, id: i64, reference: &str| SecretPathSource {
            path: path.into(),
            source: SecretSource::Bound(SecretProvenance::key(KeyOrigin {
                key_id: id,
                key_ref: reference.into(),
                owner_type: OwnerType::System,
                owner_identity: None,
                owner_ref: None,
                encrypted: true,
            })),
        };
        let mut sources = vec![
            source("/combined", 1, "system.a"),
            source("/combined/password", 2, "system.b"),
        ];
        for _ in 0..2 {
            let (redacted, inputs) = redact_secret_path_sources(
                json!({"combined":{"password":"fixture-b", "other":"fixture-a"}}),
                &sources,
            );
            assert!(is_redaction_marker(&redacted["combined"]));
            assert_eq!(inputs.len(), 1);
            let provenance: SecretProvenance =
                serde_json::from_str(inputs[0].source_ref.as_ref().unwrap()).unwrap();
            assert_eq!(provenance.origins.len(), 2);
            assert!(provenance.origins.iter().any(|origin| matches!(origin, crate::secret_provenance::SecretOrigin::Key(key) if key.key_ref == "system.b")));
            sources.reverse();
        }
    }

    #[test]
    fn secret_output_cannot_remain_visible_through_raw_stdout_or_errors() {
        let schema = json!({"token":{"type":"string", "secret":true}});
        let (redacted, secrets) = redact_action_result(
            json!({"data":{"token":"fixture-private", "status":"ok"}, "stdout":"{\"token\":\"fixture-private\"}", "error":"fixture-private", "exit_code":0}),
            Some(&schema),
            42,
            &["/credential".into()],
        );
        assert!(is_redaction_marker(&redacted["data"]["token"]));
        assert!(is_redaction_marker(&redacted["stdout"]));
        assert!(is_redaction_marker(&redacted["error"]));
        assert_eq!(redacted["data"]["status"], "ok");
        assert!(!redacted.to_string().contains("fixture-private"));
        assert_eq!(secrets.len(), 3);
        for secret in secrets {
            let provenance: crate::secret_provenance::SecretProvenance =
                serde_json::from_str(secret.source_ref.as_ref().unwrap()).unwrap();
            assert_eq!(
                provenance.origins,
                vec![crate::secret_provenance::SecretOrigin::Entity {
                    entity_type: ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: 42,
                    path: "/credential".into(),
                }]
            );
        }
        let (redacted, _) =
            redact_action_result(json!({"stdout":"fixture-private"}), Some(&schema), 42, &[]);
        assert!(is_redaction_marker(&redacted["stdout"]));
        let (public, secrets) =
            redact_action_result(json!({"stdout":"public status"}), None, 42, &[]);
        assert_eq!(public["stdout"], "public status");
        assert!(secrets.is_empty());
    }

    #[test]
    fn bound_output_warnings_do_not_fail_execution_or_expose_malformed_secrets() {
        let schema = json!({"token": {"type": "string", "secret": true}, "count": {"type": "integer", "required": true}});
        let contract = crate::value_schema::compile_inline(
            "demo.action",
            crate::value_schema::SchemaContext::FlatDocument,
            &schema,
        )
        .unwrap();
        let (result, inputs) = redact_bound_action_result(
            json!({
                "succeeded": true, "data": {"token": "fixture-private", "count": "invalid"},
                "queue_ack": {"token": "fixture-private"}, "stdout": "fixture-private"
            }),
            Some(&contract),
            crate::models::OutputFormat::Json,
            42,
            &["/password".into()],
            &[],
        )
        .unwrap();
        assert_eq!(result["succeeded"], true);
        assert!(result.get("output_validation_warning").is_some());
        assert!(is_redaction_marker(&result["data"]));
        assert!(is_redaction_marker(&result["queue_ack"]));
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("fixture-private"));
        assert!(inputs.iter().any(|input| input.json_path == "/data"));
    }

    #[test]
    fn sdk_key_reads_protect_raw_output_and_bind_marked_output_to_every_key() {
        use crate::{
            models::{OutputFormat, OwnerType},
            secret_provenance::{KeyOrigin, SecretOrigin, SecretProvenance},
        };
        let origins = [
            KeyOrigin {
                key_id: 17,
                key_ref: "pack.demo.credential".into(),
                owner_type: OwnerType::Pack,
                owner_identity: None,
                owner_ref: Some("demo".into()),
                encrypted: true,
            },
            KeyOrigin {
                key_id: 18,
                key_ref: "system.second".into(),
                owner_type: OwnerType::System,
                owner_identity: None,
                owner_ref: None,
                encrypted: false,
            },
        ];
        let contract = crate::value_schema::compile_inline(
            "demo.run",
            crate::value_schema::SchemaContext::FlatDocument,
            &json!({"token": {"type": "string", "secret": true}}),
        )
        .unwrap();
        let (redacted, inputs) = redact_bound_action_result(
            json!({"succeeded":true, "data":{"token":"sdk-private"}, "stdout":"sdk-private", "error":"sdk-private"}),
            Some(&contract), OutputFormat::Json, 42, &[], &origins,
        ).unwrap();
        assert!(!redacted.to_string().contains("sdk-private"));
        assert_eq!(inputs.len(), 3);
        for input in inputs {
            let provenance: SecretProvenance =
                serde_json::from_str(input.source_ref.as_deref().unwrap()).unwrap();
            assert_eq!(
                provenance.origins,
                origins
                    .iter()
                    .cloned()
                    .map(SecretOrigin::Key)
                    .collect::<Vec<_>>()
            );
        }
        let (redacted, inputs) = redact_bound_action_result(
            json!({"data":"unmarked code output", "stdout":"sdk-private", "error":"sdk-private"}),
            None,
            OutputFormat::Text,
            42,
            &[],
            &origins,
        )
        .unwrap();
        assert!(is_redaction_marker(&redacted["stdout"]));
        assert!(is_redaction_marker(&redacted["error"]));
        assert_eq!(inputs.len(), 2);
        // Arbitrary unmarked application data remains the pack-code trust boundary.
        assert_eq!(redacted["data"], "unmarked code output");
    }

    #[test]
    fn bound_jsonl_validates_and_redacts_each_record_and_schema_less_outputs_stay_unrestricted() {
        let schema = json!({"token": {"type": "string", "secret": true}});
        let contract = crate::value_schema::compile_inline(
            "demo.action",
            crate::value_schema::SchemaContext::FlatDocument,
            &schema,
        )
        .unwrap();
        let (result, inputs) = redact_bound_action_result(
            json!({"succeeded": true, "data": [{"token": "a"}, {"token": "b"}]}),
            Some(&contract),
            crate::models::OutputFormat::Jsonl,
            42,
            &[],
            &[],
        )
        .unwrap();
        assert!(result.get("output_validation_warning").is_none());
        assert!(is_redaction_marker(&result["data"][0]["token"]));
        assert!(inputs
            .iter()
            .any(|input| input.json_path == "/data/1/token"));
        let (result, inputs) = redact_bound_action_result(
            json!({"succeeded": true, "data": "free-form text"}),
            None,
            crate::models::OutputFormat::Text,
            42,
            &[],
            &[],
        )
        .unwrap();
        assert!(result.get("output_validation_warning").is_none());
        assert!(inputs.is_empty());
        assert_eq!(result["data"], "free-form text");
    }

    #[test]
    fn contract_parent_redaction_preserves_nested_key_origins_and_array_paths() {
        use crate::{
            models::OwnerType,
            secret_provenance::{KeyOrigin, SecretOrigin, SecretProvenance},
        };
        let schema = json!({
            "credentials": {"type": "object", "secret": true},
            "tokens": {"type": "array", "items": {"type": "string", "secret": true}}
        });
        let binding = crate::value_schema::compile_inline(
            "demo.action",
            crate::value_schema::SchemaContext::FlatDocument,
            &schema,
        )
        .unwrap();
        let contract = crate::value_schema::CompiledContract::new(&binding).unwrap();
        let origin = KeyOrigin {
            key_id: 42,
            key_ref: "system.token".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_ref: None,
            encrypted: true,
        };
        let sources = vec![SecretPathSource {
            path: "/credentials/password".into(),
            source: SecretSource::Bound(SecretProvenance::key(origin.clone())),
        }];
        let value =
            json!({"credentials": {"password": "fixture-secret"}, "tokens": ["fixture-token"]});
        validate_contract_secret_destination_paths(
            &value,
            &contract,
            &["/credentials/password".into(), "/tokens/0".into()],
        )
        .unwrap();
        assert!(
            validate_contract_secret_destination_paths(&value, &contract, &["/public".into()])
                .is_err()
        );
        let (redacted, inputs) =
            merge_contract_secret_redactions(value, &sources, &contract).unwrap();
        assert!(is_redaction_marker(&redacted["credentials"]));
        assert!(is_redaction_marker(&redacted["tokens"][0]));
        let parent = inputs
            .iter()
            .find(|input| input.json_path == "/credentials")
            .unwrap();
        let provenance: SecretProvenance =
            serde_json::from_str(parent.source_ref.as_ref().unwrap()).unwrap();
        assert!(provenance.origins.contains(&SecretOrigin::Key(origin)));
    }

    #[test]
    fn one_value_combining_keys_retains_every_origin() {
        use crate::{
            models::OwnerType,
            secret_provenance::{KeyOrigin, SecretProvenance},
        };
        let sources = ["system.a", "system.b"]
            .into_iter()
            .enumerate()
            .map(|(index, reference)| SecretPathSource {
                path: "/combined".into(),
                source: SecretSource::Bound(SecretProvenance::key(KeyOrigin {
                    key_id: index as i64 + 1,
                    key_ref: reference.into(),
                    owner_type: OwnerType::System,
                    owner_identity: None,
                    owner_ref: None,
                    encrypted: true,
                })),
            })
            .collect::<Vec<_>>();
        let (_, inputs) =
            redact_secret_path_sources(serde_json::json!({"combined":"dummy-a:dummy-b"}), &sources);
        assert_eq!(inputs.len(), 1);
        let provenance: SecretProvenance =
            serde_json::from_str(inputs[0].source_ref.as_ref().unwrap()).unwrap();
        assert_eq!(provenance.origins.len(), 2);
    }
    use serde_json::json;

    #[test]
    fn flat_schema_secret_values_are_redacted_and_restored() {
        let schema = json!({
            "username": {"type": "string"},
            "password": {"type": "string", "secret": true}
        });
        let value = json!({"username": "alice", "password": "s3cr3t"});

        let (redacted, secrets) = redact_secret_parameters(value, Some(&schema));

        assert_eq!(redacted["username"], "alice");
        assert!(is_redaction_marker(&redacted["password"]));
        assert_eq!(secrets[0].json_path, "/password");
        assert_eq!(secrets[0].value, "s3cr3t");

        let restored =
            splice_plain_secret_values(redacted, &[("/password".to_string(), json!("s3cr3t"))]);
        assert_eq!(restored["password"], "s3cr3t");
    }

    #[test]
    fn secret_destination_validation_rejects_non_secret_paths() {
        let schema = json!({
            "username": {"type": "string"},
            "password": {"type": "string", "secret": true}
        });

        assert!(
            validate_secret_destination_paths(Some(&schema), &["/password".to_string()]).is_ok()
        );
        assert!(
            validate_secret_destination_paths(Some(&schema), &["/username".to_string()]).is_err()
        );
    }

    #[test]
    fn json_schema_secret_paths_are_collected_recursively() {
        let schema = json!({
            "type": "object",
            "properties": {
                "db": {
                    "type": "object",
                    "properties": {
                        "password": {"type": "string", "secret": true}
                    }
                }
            }
        });

        assert_eq!(
            secret_paths_from_schema(Some(&schema)),
            vec!["/db/password"]
        );
    }
}
