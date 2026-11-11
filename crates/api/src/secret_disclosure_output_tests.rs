//! Output policy classification never inherits sensitivity from another channel.
use super::*;
use attune_common::{
    models::OutputFormat,
    output_policy::{OutputVisibility, ResolvedOutputPolicy, StdoutPolicy},
    value_schema::{compile_inline, SchemaContext},
};
use serde_json::json;

fn binding(policy: ResolvedOutputPolicy) -> ActionSchemaBinding {
    ActionSchemaBinding {
        input: compile_inline(
            "test.action",
            SchemaContext::FlatDocument,
            &json!({"credential":{"type":"string","secret":true}}),
        )
        .unwrap(),
        output: None,
        output_policy: policy,
        output_format: OutputFormat::Text,
        static_actions: Default::default(),
    }
}

fn origin(entity: &str, path: &str, kind: &str) -> SecretProvenanceRow {
    SecretProvenanceRow {
        entity_type: entity.into(),
        entity_id: 42,
        json_path: path.into(),
        source_kind: "provenance".into(),
        source_ref: Some(
            serde_json::to_string(&SecretProvenance {
                origins: vec![SecretOrigin::Local {
                    source_kind: kind.into(),
                    source_ref: None,
                }],
                templates: vec![],
            })
            .unwrap(),
        ),
    }
}

#[test]
fn public_raw_channel_does_not_inherit_secret_inputs_or_other_channels() {
    let public = binding(ResolvedOutputPolicy::default());
    let origins = vec![origin(
        ENTITY_EXECUTION_CONFIG,
        "/credential",
        "parameter_schema",
    )];
    assert!(!runtime_log_requires_disclosure(&public, &origins).unwrap());
    let mixed = binding(ResolvedOutputPolicy {
        stdout: StdoutPolicy::Secret,
        stderr: OutputVisibility::Public,
        error: OutputVisibility::Secret,
    });
    assert!(runtime_log_channel_requires_disclosure(
        &mixed,
        OutputChannel::Stdout
    ));
    assert!(!runtime_log_channel_requires_disclosure(
        &mixed,
        OutputChannel::Stderr
    ));
    assert!(runtime_log_channel_requires_disclosure(
        &mixed,
        OutputChannel::Error
    ));
}

#[test]
fn completed_channel_uses_its_exact_sources_instead_of_unrelated_fields() {
    let stdout = origin(ENTITY_EXECUTION_OUTPUT_CHANNEL, "/stdout", "stdout-exact");
    let rows = vec![
        origin(ENTITY_EXECUTION_CONFIG, "/credential", "input"),
        origin(ENTITY_EXECUTION_RESULT, "/data/token", "result"),
        stdout.clone(),
        origin(
            ENTITY_EXECUTION_OUTPUT_CHANNEL,
            "/stderr",
            "stderr-unrelated",
        ),
    ];
    assert_eq!(
        runtime_log_channel_origins(&rows, OutputChannel::Stdout),
        vec![stdout]
    );
}

#[test]
fn pending_channel_inherits_delivered_inputs_and_actual_reads_only() {
    let input = origin(ENTITY_EXECUTION_CONFIG, "/credential", "input");
    let runtime = origin("execution_runtime_key_read", "/0", "runtime");
    let rows = vec![
        input.clone(),
        runtime.clone(),
        origin(ENTITY_EXECUTION_RESULT, "/data/token", "result"),
        origin("workflow_variables", "/published", "workflow"),
        origin(ENTITY_EXECUTION_OUTPUT_CHANNEL, "/stderr", "stderr"),
    ];
    assert_eq!(
        runtime_log_channel_origins(&rows, OutputChannel::Stdout),
        vec![input, runtime]
    );
}
