//! Parsing contracts that must survive replacement of the YAML implementation.
use attune_common::{pack_check::check_pack, workflow::parser::parse_workflow_yaml};
use serde_json::json;

#[test]
fn yaml_scalar_resolution_preserves_templates_and_large_integers() {
    let value: serde_json::Value = serde_yaml_ng::from_str(
        "boolean: true\nquoted: 'true'\nleading: 0123\nhex: 0x10\nmax: 18446744073709551615\nword: on\ntemplate: '{{ parameters.items }}'\n",
    )
    .unwrap();
    assert_eq!(
        value,
        json!({
            "boolean": true, "quoted": "true", "leading": "0123", "hex": 16,
            "max": u64::MAX, "word": "on", "template": "{{ parameters.items }}",
        })
    );
}

#[test]
fn yaml_workflow_roundtrip_preserves_iteration_and_transition_expressions() {
    let workflow = parse_workflow_yaml(
        "version: '1.0.0'\ntasks:\n  - name: each\n    action: demo.echo\n    with_items: '{{ parameters.items }}'\n    input: {items: '{{ parameters.items }}'}\n    next:\n      - when: '{{ succeeded() }}'\n        do: [after]\n  - name: after\n    action: demo.echo\n    input: {items: '{{ parameters.items }}'}\noutput_map: {items: '{{ parameters.items }}'}\n",
    )
    .unwrap();
    let serialized = serde_yaml_ng::to_string(&workflow).unwrap();
    let restored = parse_workflow_yaml(&serialized).unwrap();
    assert_eq!(
        serde_json::to_value(workflow).unwrap(),
        serde_json::to_value(restored).unwrap()
    );
}

fn schema_report(schema: &str) -> attune_common::pack_check::PackCheckReport {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("pack.yaml"),
        "ref: demo\nlabel: Demo\nversion: 1.0.0\n",
    )
    .unwrap();
    std::fs::create_dir(root.path().join("actions")).unwrap();
    std::fs::write(root.path().join("actions/echo.sh"), "#!/bin/sh\ncat\n").unwrap();
    std::fs::write(
        root.path().join("actions/echo.yaml"),
        format!(
            "ref: demo.echo\nrunner_type: shell\nentry_point: echo.sh\nparameters:\n{schema}\n"
        ),
    )
    .unwrap();
    check_pack(root.path())
}

#[test]
fn yaml_schema_keys_remain_strings_without_scalar_coercion() {
    let valid = schema_report("  'true': {type: string}");
    assert!(valid.valid, "{:?}", valid.diagnostics);
    for invalid in [
        "  true: {type: string}",
        "  42: {type: string}",
        "  [key]: {type: string}",
    ] {
        let report = schema_report(invalid);
        assert!(
            !report.valid,
            "non-string schema key was silently accepted: {invalid}"
        );
    }
}

#[test]
fn yaml_duplicate_keys_and_multiple_documents_fail_closed() {
    for source in [
        "value: first\nvalue: second\n",
        "first: true\n---\nsecond: true\n",
    ] {
        assert!(serde_yaml_ng::from_str::<serde_yaml_ng::Value>(source).is_err());
    }
}

#[test]
fn yaml_recursive_aliases_and_invalid_utf8_fail_closed() {
    assert!(serde_yaml_ng::from_str::<serde_yaml_ng::Value>("value: &loop [*loop]").is_err());
    assert!(serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(b"value: \xff").is_err());
}

#[test]
fn yaml_metadata_rejects_custom_tags_without_exposing_source_values() {
    for schema in [
        "  token: !private {type: string, default: SECRET_CANARY}",
        "  token: [SECRET_CANARY",
    ] {
        let report = schema_report(schema);
        assert!(!report.valid);
        assert!(!serde_json::to_string(&report.diagnostics)
            .unwrap()
            .contains("SECRET_CANARY"));
    }
}

#[test]
fn yaml_runtime_graph_has_no_unmaintained_backend() {
    let lock = include_str!("../../../Cargo.lock");
    assert!(!lock.contains("name = \"unsafe-libyaml\""));
    assert!(!lock.contains("name = \"serde_yaml_ng\""));
}
