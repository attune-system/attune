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
    QueueItem {
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
            SecretSource::QueueItem {
                queue_ref,
                item_id,
                path,
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

#[derive(Debug, Clone, PartialEq)]
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
        .filter(|source| pointer_suffix(&source.path, path).is_some())
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

pub fn merge_schema_secret_redactions(
    value: JsonValue,
    existing_sources: &[SecretPathSource],
    schema: Option<&JsonValue>,
) -> (JsonValue, Vec<SecretValueInput>) {
    let mut path_sources = existing_sources.to_vec();
    let existing_paths = existing_sources
        .iter()
        .map(|source| source.path.as_str())
        .collect::<BTreeSet<_>>();

    for path in secret_paths_from_schema(schema) {
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
                return Err(Error::encryption(format!(
                    "Encryption key hash mismatch for secret path '{}'",
                    secret.json_path
                )));
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
