//! Canonical public output projections and separately addressed protected originals.
//!
//! Only the bound output schema and channel policy classify values. Input and
//! runtime provenance describe disclosure rights; they never expand secrecy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::{
    models::OutputFormat,
    output_policy::{OutputVisibility, ResolvedOutputPolicy, StdoutPolicy},
    secret_provenance::SecretProvenance,
    secret_values::{
        is_redaction_marker, merge_contract_secret_redactions, pointer_suffix, redacted_paths,
        redaction_marker, SecretSource, SecretValueInput,
    },
    value_schema::{CompiledContract, Contract, Limits},
    Error, Result,
};

pub const ENTITY_EXECUTION_OUTPUT_CHANNEL: &str = "execution_output_channel";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputChannel {
    Stdout,
    Stderr,
    Error,
}

impl OutputChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Error => "error",
        }
    }

    pub fn json_path(self) -> &'static str {
        match self {
            Self::Stdout => "/stdout",
            Self::Stderr => "/stderr",
            Self::Error => "/error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OriginalOutputChannelReference {
    pub execution_id: i64,
    pub channel: OutputChannel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputChannelProjectionReason {
    Unparseable,
    Truncated,
    ResourceLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputChannelProjection {
    pub policy: StdoutPolicy,
    pub original: OriginalOutputChannelReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<OutputChannelProjectionReason>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedActionOutput {
    pub public_result: Value,
    /// Replacements in the public structured result, stored as execution_result.
    pub structured_secrets: Vec<SecretValueInput>,
    /// Raw originals, stored as execution_output_channel, never execution_result.
    pub channel_secrets: Vec<SecretValueInput>,
}

/// Validate public channel references without reading private originals.
/// Metadata never establishes disclosure authority by itself.
pub fn output_channel_projections(
    result: &Value,
    execution_id: i64,
) -> Result<BTreeMap<OutputChannel, OutputChannelProjection>> {
    let Some(metadata) = result.get("output_channels") else {
        return Ok(BTreeMap::new());
    };
    let projections: BTreeMap<OutputChannel, OutputChannelProjection> =
        serde_json::from_value(metadata.clone())
            .map_err(|_| Error::validation("Invalid output channel projection"))?;
    for (channel, projection) in &projections {
        if execution_id <= 0
            || projection.original.execution_id != execution_id
            || projection.original.channel != *channel
            || projection.policy == StdoutPolicy::Public
            || (*channel != OutputChannel::Stdout && projection.policy == StdoutPolicy::Sanitized)
        {
            return Err(Error::validation("Invalid output channel reference"));
        }
        let value = result
            .get(channel.as_str())
            .ok_or_else(|| Error::validation("Missing output channel projection"))?;
        let valid = match projection.policy {
            StdoutPolicy::Secret => is_redaction_marker(value) && projection.reason.is_none(),
            StdoutPolicy::Sanitized => match projection.reason {
                Some(_) => is_redaction_marker(value),
                None => value.is_string(),
            },
            StdoutPolicy::Public => false,
        };
        if !valid {
            return Err(Error::validation("Invalid output channel projection"));
        }
    }
    Ok(projections)
}

pub fn classify_action_output(
    result: Value,
    output: Option<&Contract>,
    format: OutputFormat,
    policy: ResolvedOutputPolicy,
    execution_id: i64,
    origins: &SecretProvenance,
) -> Result<ClassifiedActionOutput> {
    if !result.is_object() || execution_id <= 0 {
        return Err(Error::validation("Invalid action output envelope"));
    }
    let envelope_limits = Limits::default();
    crate::value_schema::check_json(
        &result,
        envelope_limits,
        envelope_limits.max_value_bytes.saturating_mul(4),
    )
    .map_err(|_| Error::validation("Action output envelope exceeds its bound"))?;
    if result.get("output_channels").is_some() || !redacted_paths(&result).is_empty() {
        return Err(Error::validation(
            "Action output uses reserved protection metadata",
        ));
    }
    let compiled = output
        .map(CompiledContract::new)
        .transpose()
        .map_err(|_| Error::validation("Invalid bound output contract"))?;
    let sensitive = compiled
        .as_ref()
        .is_some_and(CompiledContract::declares_secrets);
    if (format == OutputFormat::Text && policy.stdout == StdoutPolicy::Sanitized)
        || (sensitive && policy.stdout == StdoutPolicy::Public)
    {
        return Err(Error::validation(
            "Output channel policy conflicts with its contract",
        ));
    }

    let mut public_result = result.clone();
    let mut structured_secrets = Vec::new();
    if let (Some(contract), Some(data)) = (compiled.as_ref(), result.get("data")) {
        let (public, secrets, invalid) =
            classify_data(data.clone(), contract, output.unwrap(), format, origins)?;
        public_result["data"] = public;
        structured_secrets = secrets;
        if invalid {
            // Dynamic keys, received values, and validator prose are never diagnostics.
            public_result["output_validation_warning"] = json!({"code":"invalid_output"});
        }
    }
    protect_acknowledgement_duplicate(
        &result,
        &mut public_result,
        &mut structured_secrets,
        origins,
    )?;

    let mut channel_secrets = Vec::new();
    let mut projections = BTreeMap::new();
    for channel in [
        OutputChannel::Stdout,
        OutputChannel::Stderr,
        OutputChannel::Error,
    ] {
        let Some(original) = result
            .get(channel.as_str())
            .filter(|value| !value.is_null())
        else {
            continue;
        };
        let channel_policy = match channel {
            OutputChannel::Stdout => policy.stdout,
            OutputChannel::Stderr => visibility_policy(policy.stderr),
            OutputChannel::Error => visibility_policy(policy.error),
        };
        if channel_policy == StdoutPolicy::Public {
            continue;
        }
        channel_secrets.push(secret_value(
            channel.json_path(),
            original.clone(),
            origins,
        )?);
        let (projection, reason) = if channel_policy == StdoutPolicy::Secret {
            (redaction_marker(), None)
        } else {
            match sanitized_stdout(&result, compiled.as_ref(), output, format, origins) {
                Ok(value) => (Value::String(value), None),
                Err(reason) => (redaction_marker(), Some(reason)),
            }
        };
        public_result[channel.as_str()] = projection;
        projections.insert(
            channel,
            OutputChannelProjection {
                policy: channel_policy,
                original: OriginalOutputChannelReference {
                    execution_id,
                    channel,
                },
                reason,
            },
        );
    }
    if !projections.is_empty() {
        public_result["output_channels"] = serde_json::to_value(projections)?;
    }
    output_channel_projections(&public_result, execution_id)?;
    Ok(ClassifiedActionOutput {
        public_result,
        structured_secrets,
        channel_secrets,
    })
}

fn visibility_policy(value: OutputVisibility) -> StdoutPolicy {
    match value {
        OutputVisibility::Public => StdoutPolicy::Public,
        OutputVisibility::Secret => StdoutPolicy::Secret,
    }
}

fn secret_value(path: &str, value: Value, origins: &SecretProvenance) -> Result<SecretValueInput> {
    let local;
    let origins = if origins.origins.is_empty() {
        local = SecretSource::ParameterSchema { path: path.into() }.provenance();
        &local
    } else {
        origins
    };
    let reference = serde_json::to_string(origins)?;
    if origins.origins.len() > 256 || reference.len() > 1024 * 1024 {
        return Err(Error::validation("Output provenance exceeds its bound"));
    }
    Ok(SecretValueInput {
        json_path: path.into(),
        value,
        source_kind: "provenance".into(),
        source_ref: Some(reference),
    })
}

/// Reconstruct exactly the canonical structured slots, including bounded
/// record/traversal fallbacks, without interpreting raw-channel projections.
pub fn action_data_secret_inputs(
    data: &Value,
    output: Option<&Contract>,
    format: OutputFormat,
) -> Result<Vec<SecretValueInput>> {
    let Some(output) = output else {
        return Ok(Vec::new());
    };
    let compiled = CompiledContract::new(output)
        .map_err(|_| Error::validation("Invalid bound output contract"))?;
    let (_, fields, _) = classify_data(
        data.clone(),
        &compiled,
        output,
        format,
        &SecretProvenance::default(),
    )?;
    Ok(fields)
}

fn classify_data(
    data: Value,
    compiled: &CompiledContract<'_>,
    contract: &Contract,
    format: OutputFormat,
    origins: &SecretProvenance,
) -> Result<(Value, Vec<SecretValueInput>, bool)> {
    let records: Vec<_> = match format {
        OutputFormat::Jsonl => match data.as_array() {
            Some(records) if records.len() <= contract.limits.max_literal_checks => {
                records.iter().collect()
            }
            _ if compiled.declares_secrets() => {
                return Ok((
                    redaction_marker(),
                    vec![secret_value("/data", data, origins)?],
                    true,
                ))
            }
            _ => return Ok((data, Vec::new(), true)),
        },
        _ => vec![&data],
    };
    let mut public_records = Vec::with_capacity(records.len());
    let mut secrets = Vec::new();
    let mut invalid = false;
    for (index, record) in records.into_iter().enumerate() {
        invalid |= compiled.validate(record).is_err();
        let prefix = if format == OutputFormat::Jsonl {
            format!("/data/{index}")
        } else {
            "/data".into()
        };
        let (public, inputs) = if compiled.declares_secrets() {
            match merge_contract_secret_redactions(record.clone(), &[], compiled) {
                Ok(value) => value,
                Err(_) => {
                    // Traversal failure makes this record's subtree ambiguous, not
                    // independently classifiable sibling records or raw channels.
                    invalid = true;
                    (
                        redaction_marker(),
                        vec![secret_value("", record.clone(), origins)?],
                    )
                }
            }
        } else {
            (record.clone(), Vec::new())
        };
        for input in inputs {
            secrets.push(secret_value(
                &format!("{prefix}{}", input.json_path),
                input.value,
                origins,
            )?);
        }
        public_records.push(public);
    }
    let public = if format == OutputFormat::Jsonl {
        Value::Array(public_records)
    } else {
        public_records.pop().unwrap_or(Value::Null)
    };
    Ok((public, secrets, invalid))
}

fn protect_acknowledgement_duplicate(
    original: &Value,
    public: &mut Value,
    secrets: &mut Vec<SecretValueInput>,
    origins: &SecretProvenance,
) -> Result<()> {
    let duplicates: Vec<_> = secrets
        .iter()
        .filter_map(|input| {
            if input.json_path == "/data" {
                Some("/queue_ack".to_string())
            } else {
                pointer_suffix(&input.json_path, "/data/queue_ack")
                    .map(|suffix| format!("/queue_ack{suffix}"))
            }
        })
        .collect();
    for path in duplicates {
        if let Some(value) = original.pointer(&path) {
            secrets.push(secret_value(&path, value.clone(), origins)?);
            if let Some(value) = public.pointer_mut(&path) {
                *value = redaction_marker();
            }
        }
    }
    Ok(())
}

fn sanitized_stdout(
    result: &Value,
    compiled: Option<&CompiledContract<'_>>,
    contract: Option<&Contract>,
    format: OutputFormat,
    origins: &SecretProvenance,
) -> std::result::Result<String, OutputChannelProjectionReason> {
    use OutputChannelProjectionReason::{ResourceLimit, Truncated, Unparseable};
    if result.get("stdout_truncated") == Some(&Value::Bool(true))
        || result.get("logs_incomplete") == Some(&Value::Bool(true))
    {
        return Err(Truncated);
    }
    let stdout = result
        .get("stdout")
        .and_then(Value::as_str)
        .ok_or(Unparseable)?;
    let limit = contract.map(|contract| contract.limits).unwrap_or_default();
    if stdout.len() > limit.max_value_bytes {
        return Err(ResourceLimit);
    }
    let trimmed = stdout.trim();
    let parsed = match format {
        OutputFormat::Json => serde_json::from_str(trimmed)
            .or_else(|_| serde_json::from_str(trimmed.lines().last().unwrap_or_default()))
            .map_err(|_| Unparseable)?,
        OutputFormat::Yaml => serde_yaml_ng::from_str(trimmed).map_err(|_| Unparseable)?,
        OutputFormat::Jsonl => {
            let mut records = Vec::new();
            for line in trimmed.lines().filter(|line| !line.trim().is_empty()) {
                if records.len() >= limit.max_literal_checks {
                    return Err(ResourceLimit);
                }
                records.push(serde_json::from_str(line).map_err(|_| Unparseable)?);
            }
            if records.is_empty() {
                return Err(Unparseable);
            }
            Value::Array(records)
        }
        OutputFormat::Text => return Err(Unparseable),
    };
    crate::value_schema::check_json(&parsed, limit, limit.max_value_bytes)
        .map_err(|_| ResourceLimit)?;
    if !redacted_paths(&parsed).is_empty() {
        return Err(Unparseable);
    }
    let public = match (compiled, contract) {
        (Some(compiled), Some(contract)) => {
            classify_data(parsed, compiled, contract, format, origins)
                .map_err(|_| ResourceLimit)?
                .0
        }
        _ => parsed,
    };
    if is_redaction_marker(&public) {
        return Err(ResourceLimit);
    }
    match format {
        OutputFormat::Jsonl => public
            .as_array()
            .ok_or(Unparseable)?
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(|records| records.join("\n"))
            .map_err(|_| ResourceLimit),
        // YAML stdout is deliberately projected as JSON too: JSON is a valid
        // YAML representation and cannot reintroduce unclassified debug bytes.
        _ => serde_json::to_string(&public).map_err(|_| ResourceLimit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        output_policy::OutputPolicy,
        secret_provenance::SecretOrigin,
        value_schema::{compile_inline, SchemaContext},
    };

    fn contract(schema: Value) -> Contract {
        compile_inline("example.result", SchemaContext::FlatDocument, &schema).unwrap()
    }

    fn classify(
        result: Value,
        schema: Option<&Contract>,
        format: OutputFormat,
    ) -> ClassifiedActionOutput {
        classify_action_output(
            result,
            schema,
            format,
            OutputPolicy::default().resolve(schema, format).unwrap(),
            42,
            &SecretProvenance::default(),
        )
        .unwrap()
    }

    #[test]
    fn input_or_runtime_origins_do_not_classify_public_outputs() {
        let origins = SecretProvenance {
            origins: vec![SecretOrigin::Entity {
                entity_type: "execution_config".into(),
                entity_id: 42,
                path: "/password".into(),
            }],
            templates: Vec::new(),
        };
        let original = json!({"succeeded":true,"data":{"status":"ok"},"stdout":"ok","stderr":"diagnostic","error":"failure message"});
        let result = classify_action_output(
            original.clone(),
            None,
            OutputFormat::Text,
            OutputPolicy::default()
                .resolve(None, OutputFormat::Text)
                .unwrap(),
            42,
            &origins,
        )
        .unwrap();
        assert_eq!(result.public_result, original);
        assert!(result.structured_secrets.is_empty());
        assert!(result.channel_secrets.is_empty());
    }

    #[test]
    fn protects_only_declared_nested_fields_and_retains_separate_raw_originals() {
        let schema = contract(json!({
            "account":{"type":"object","properties":{"name":{"type":"string"},"token":{"type":"string","secret":true}}},
            "credentials":{"type":"array","items":{"type":"object","properties":{"service":{"type":"string"},"password":{"type":"string","secret":true}}}}
        }));
        let data = json!({"account":{"name":"public-account","token":"private-token"},"credentials":[{"service":"public-service","password":"private-password"}]});
        let stdout = format!(
            "debug line contains private-token\n{}",
            serde_json::to_string(&data).unwrap()
        );
        let result = classify(
            json!({"data":data,"stdout":stdout}),
            Some(&schema),
            OutputFormat::Json,
        );
        assert_eq!(
            result.public_result["data"]["account"]["name"],
            "public-account"
        );
        assert_eq!(
            result.public_result["data"]["credentials"][0]["service"],
            "public-service"
        );
        assert!(is_redaction_marker(
            &result.public_result["data"]["account"]["token"]
        ));
        assert!(is_redaction_marker(
            &result.public_result["data"]["credentials"][0]["password"]
        ));
        assert_eq!(result.structured_secrets.len(), 2);
        assert_eq!(result.channel_secrets.len(), 1);
        assert_eq!(result.channel_secrets[0].json_path, "/stdout");
        assert_eq!(result.channel_secrets[0].value, stdout);
        let sanitized = result.public_result["stdout"].as_str().unwrap();
        assert!(!sanitized.contains("private-token"));
        assert!(!sanitized.contains("private-password"));
        assert!(!sanitized.contains("debug"));
        assert!(sanitized.contains("public-account"));
        assert!(result
            .structured_secrets
            .iter()
            .all(|field| field.json_path.starts_with("/data/")));
        let projections = output_channel_projections(&result.public_result, 42).unwrap();
        assert_eq!(
            projections[&OutputChannel::Stdout].policy,
            StdoutPolicy::Sanitized
        );
        assert!(projections[&OutputChannel::Stdout].reason.is_none());
        assert!(output_channel_projections(&result.public_result, 43).is_err());
    }

    #[test]
    fn invalid_public_fields_do_not_expand_secrecy() {
        let schema =
            contract(json!({"count":{"type":"integer"},"token":{"type":"string","secret":true}}));
        let result = classify(
            json!({"data":{"count":"invalid-but-public","token":"private"}}),
            Some(&schema),
            OutputFormat::Json,
        );
        assert_eq!(result.public_result["data"]["count"], "invalid-but-public");
        assert!(is_redaction_marker(&result.public_result["data"]["token"]));
        assert_eq!(result.structured_secrets[0].json_path, "/data/token");
        assert!(result
            .public_result
            .get("output_validation_warning")
            .is_some());
    }

    #[test]
    fn malformed_secret_container_does_not_hide_public_siblings() {
        let schema = contract(
            json!({"status":{"type":"string"},"credentials":{"type":"object","properties":{"password":{"type":"string","secret":true}}}}),
        );
        let result = classify(
            json!({"data":{"status":"ok","credentials":"malformed-private-container"}}),
            Some(&schema),
            OutputFormat::Json,
        );
        assert_eq!(result.public_result["data"]["status"], "ok");
        assert!(is_redaction_marker(
            &result.public_result["data"]["credentials"]
        ));
        assert_eq!(result.structured_secrets.len(), 1);
        assert_eq!(result.structured_secrets[0].json_path, "/data/credentials");
    }

    #[test]
    fn protects_explicit_channels_without_tainting_structured_data() {
        let policy = ResolvedOutputPolicy {
            stdout: StdoutPolicy::Secret,
            stderr: OutputVisibility::Secret,
            error: OutputVisibility::Public,
        };
        let result = classify_action_output(json!({"data":{"public":1},"stdout":"private-stdout","stderr":"private-stderr","error":"public-error"}), None, OutputFormat::Text, policy, 42, &SecretProvenance::default()).unwrap();
        assert_eq!(result.public_result["data"], json!({"public":1}));
        assert_eq!(result.public_result["error"], "public-error");
        assert!(is_redaction_marker(&result.public_result["stdout"]));
        assert!(is_redaction_marker(&result.public_result["stderr"]));
        assert!(result.structured_secrets.is_empty());
        assert_eq!(result.channel_secrets.len(), 2);
        for field in &result.channel_secrets {
            let origins: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().unwrap()).unwrap();
            assert!(
                matches!(&origins.origins[0], SecretOrigin::Local { source_kind, .. } if source_kind == "parameter_schema")
            );
        }
    }

    #[test]
    fn jsonl_records_are_selectively_redacted_and_projected_separately() {
        let schema =
            contract(json!({"service":{"type":"string"},"token":{"type":"string","secret":true}}));
        let data = json!([{"service":"one","token":"private-one"},{"service":"two","token":"private-two"}]);
        let stdout = data
            .as_array()
            .unwrap()
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let result = classify(
            json!({"data":data,"stdout":stdout}),
            Some(&schema),
            OutputFormat::Jsonl,
        );
        assert_eq!(result.structured_secrets.len(), 2);
        assert_eq!(result.public_result["data"][1]["service"], "two");
        let projected = result.public_result["stdout"].as_str().unwrap();
        assert_eq!(projected.lines().count(), 2);
        assert!(!projected.contains("private-one"));
        assert!(!projected.contains("private-two"));
    }

    #[test]
    fn malformed_or_truncated_raw_channels_do_not_hide_independent_public_data() {
        let schema =
            contract(json!({"status":{"type":"string"},"token":{"type":"string","secret":true}}));
        for (stdout, truncated, reason) in [
            (
                "not-json",
                false,
                OutputChannelProjectionReason::Unparseable,
            ),
            (
                "{\"status\":\"ok\"}",
                true,
                OutputChannelProjectionReason::Truncated,
            ),
        ] {
            let result = classify(
                json!({"data":{"status":"ok","token":"private"},"stdout":stdout,"stdout_truncated":truncated}),
                Some(&schema),
                OutputFormat::Json,
            );
            assert_eq!(result.public_result["data"]["status"], "ok");
            assert!(is_redaction_marker(&result.public_result["stdout"]));
            assert_eq!(
                output_channel_projections(&result.public_result, 42).unwrap()
                    [&OutputChannel::Stdout]
                    .reason,
                Some(reason)
            );
        }
    }

    #[test]
    fn invalid_jsonl_middle_record_is_not_copied_into_sanitized_stdout() {
        let schema = contract(json!({"token":{"type":"string","secret":true}}));
        let result = classify(
            json!({"data":[{"token":"one"},{"token":"two"}],"stdout":"{\"token\":\"one\"}\nunparsed private debug\n{\"token\":\"two\"}"}),
            Some(&schema),
            OutputFormat::Jsonl,
        );
        assert!(is_redaction_marker(&result.public_result["stdout"]));
        assert_eq!(result.structured_secrets.len(), 2);
        assert!(!result
            .public_result
            .to_string()
            .contains("unparsed private debug"));
    }

    #[test]
    fn queue_acknowledgement_duplicates_share_classification() {
        let schema = contract(
            json!({"queue_ack":{"type":"object","properties":{"status":{"type":"string"},"summary":{"type":"string","secret":true}}}}),
        );
        let ack = json!({"status":"complete","summary":"private-summary"});
        let result = classify(
            json!({"data":{"queue_ack":ack},"queue_ack":ack}),
            Some(&schema),
            OutputFormat::Json,
        );
        assert_eq!(result.public_result["queue_ack"]["status"], "complete");
        assert!(is_redaction_marker(
            &result.public_result["queue_ack"]["summary"]
        ));
        assert!(result
            .structured_secrets
            .iter()
            .any(|field| field.json_path == "/queue_ack/summary"));
    }

    #[test]
    fn inherited_named_annotations_cannot_be_weakened_by_false() {
        let error = compile_inline(
            "example.result", SchemaContext::FlatDocument,
            &json!({"credential":{"type":"object","$defs":{"token":{"type":"string","secret":true}},"properties":{"token":{"$ref":"#/credential/$defs/token","secret":false}}}}),
        ).unwrap_err();
        assert_eq!(error.code, crate::value_schema::ErrorCode::SecretDowngrade);
    }

    #[test]
    fn named_type_secrecy_and_additional_property_paths_are_preserved() {
        use crate::value_schema::{compile, RegistryEntry, RegistryView, SourceIdentity};
        let registry = RegistryView::from([(
            "example.credential".into(),
            RegistryEntry::Candidate(json!({"type":"string","secret":true})),
        )]);
        let schema = compile(
            SourceIdentity::Authoring {
                component_ref: "example.result".into(),
            },
            SchemaContext::FlatDocument,
            &json!({
                "credential":{"type":"example.credential"},
                "secrets":{"type":"object","additionalProperties":{"type":"string","secret":true}},
                "public":{"type":"string"}
            }),
            &registry,
            Limits::default(),
        )
        .unwrap();
        let result = classify(
            json!({"data":{"credential":"private-credential","secrets":{"dynamic/key":"private-dynamic"},"public":"visible"}}),
            Some(&schema),
            OutputFormat::Json,
        );
        assert_eq!(result.public_result["data"]["public"], "visible");
        assert!(is_redaction_marker(
            &result.public_result["data"]["credential"]
        ));
        assert!(is_redaction_marker(
            &result.public_result["data"]["secrets"]["dynamic/key"]
        ));
        assert!(result
            .structured_secrets
            .iter()
            .any(|secret| secret.json_path == "/data/secrets/dynamic~1key"));
    }

    #[test]
    fn malformed_secret_array_is_protected_without_expanding_to_root() {
        let schema = contract(json!({
            "public":{"type":"string"},
            "credentials":{"type":"array","items":{"type":"object","properties":{"token":{"type":"string","secret":true}}}}
        }));
        let result = classify(
            json!({"data":{"public":"visible","credentials":{"wrong":"private"}}}),
            Some(&schema),
            OutputFormat::Json,
        );
        assert_eq!(result.public_result["data"]["public"], "visible");
        assert!(is_redaction_marker(
            &result.public_result["data"]["credentials"]
        ));
        assert_eq!(result.structured_secrets[0].json_path, "/data/credentials");
    }

    #[test]
    fn rejects_forged_markers_channel_metadata_and_original_references() {
        for result in [
            json!({"data":{"token":redaction_marker()}}),
            json!({"output_channels":{}}),
        ] {
            assert!(classify_action_output(
                result,
                None,
                OutputFormat::Text,
                OutputPolicy::default()
                    .resolve(None, OutputFormat::Text)
                    .unwrap(),
                42,
                &SecretProvenance::default()
            )
            .is_err());
        }
        let result = classify_action_output(
            json!({"stdout":"private"}),
            None,
            OutputFormat::Text,
            ResolvedOutputPolicy {
                stdout: StdoutPolicy::Secret,
                stderr: OutputVisibility::Public,
                error: OutputVisibility::Public,
            },
            42,
            &SecretProvenance::default(),
        )
        .unwrap();
        let mut altered = result.public_result;
        altered["output_channels"]["stdout"]["original"]["channel"] = json!("stderr");
        assert!(output_channel_projections(&altered, 42).is_err());
        altered["output_channels"]["stdout"]["original"]["channel"] = json!("stdout");
        altered["output_channels"]["stdout"]["unexpected"] = json!("private metadata");
        assert!(output_channel_projections(&altered, 42).is_err());
    }
}
