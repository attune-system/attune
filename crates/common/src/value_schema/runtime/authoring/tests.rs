use serde_json::{json, Value};

use crate::value_schema::{
    compile, compile_inline, CompiledContract, ErrorCode, Limits, RegistryEntry, RegistryView,
    SchemaContext, SourceIdentity, TypeRevision,
};

fn check(schema: Value, value: Value) -> crate::value_schema::Result<super::AuthoringReport> {
    let contract = compile_inline("test.authoring", SchemaContext::SingleValue, &schema).unwrap();
    CompiledContract::new(&contract)
        .unwrap()
        .validate_authoring(&value)
}

#[test]
fn rule_secrets_require_key_templates_including_nested_values_and_defaults() {
    let contract = compile_inline(
        "test.rule",
        SchemaContext::FlatDocument,
        &json!({
            "credential":{"type":"string","secret":true},
            "nested":{"type":"object","properties":{"token":{"type":"string","secret":true}}},
            "tokens":{"type":"array","secret":true,"items":{"type":"string"}}
        }),
    )
    .unwrap();
    let compiled = CompiledContract::new(&contract).unwrap();
    for value in [
        json!({"credential":"{{ keystore['system.token'] }}"}),
        json!({"credential":"{{ keystore.system.token }}"}),
        json!({"credential":"{{ keystore['identity.alice@example.com.api_token'] }}"}),
        json!({"nested":{"token":"{{ keystore[pack.config.token_ref] }}"}}),
        json!({"nested":"{{ keystore['system.structured'] }}"}),
        json!({"tokens":["{{ keystore['system.first'] }}", "{{ keystore.system.second }}"]}),
        json!({}),
    ] {
        compiled.validate_rule_secret_templates(&value).unwrap();
    }
    for value in [
        json!({"credential":"literal-secret"}),
        json!({"credential":"Bearer {{ keystore.system.token }}"}),
        json!({"credential":"{{ event.payload.token }}"}),
        json!({"credential":"{{ keystore.system.token }}literal-secret"}),
        json!({"credential":"{{ keystore.system.token }}{{ keystore.system.other }}"}),
        json!({"nested":{"token":"literal-secret"}}),
        json!({"nested":"{{ event.payload.object }}"}),
        json!({"tokens":["{{ keystore.system.first }}", "literal-secret"]}),
        json!({"credential":null}),
    ] {
        let error = compiled.validate_rule_secret_templates(&value).unwrap_err();
        assert!(!error.to_string().contains("literal-secret"));
    }
    let error = compile_inline(
        "test.rule",
        SchemaContext::FlatDocument,
        &json!({"credential":{"type":"string","secret":true,"default":"literal-secret"}}),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::SecretLiteral);
    assert!(!error.to_string().contains("literal-secret"));
}

#[test]
fn rule_secret_template_policy_resolves_named_types_and_composition() {
    let registry = RegistryView::from([(
        "test.credentials".into(),
        RegistryEntry::Candidate(json!({
            "type":"object","properties":{"token":{"type":"string","secret":true}}
        })),
    )]);
    let contract = compile(SourceIdentity::Authoring {component_ref:"test.rule".into()}, SchemaContext::FlatDocument,
        &json!({"input":{"type":"test.credentials"}, "combined":{"anyOf":[{"type":"string"},{"type":"string","secret":true}]}}),
        &registry, Limits::default()).unwrap();
    let compiled = CompiledContract::new(&contract).unwrap();
    compiled.validate_rule_secret_templates(&json!({"input":{"token":"{{ keystore.system.token }}"},"combined":"{{ keystore.system.token }}"})).unwrap();
    for value in [
        json!({"input":{"token":"literal"}}),
        json!({"input":"{{ event.payload.object }}"}),
        json!({"combined":"literal"}),
    ] {
        assert!(compiled.validate_rule_secret_templates(&value).is_err());
    }
}

#[test]
fn constrained_whole_values_are_deferred_without_fabricated_samples() {
    for schema in [
        json!({"type": "object", "properties": {"child": {"type": "integer"}}, "required": ["child"], "additionalProperties": false}),
        json!({"type": "string", "pattern": "^[A-Z]{3}$", "minLength": 3, "maxLength": 3}),
        json!({"type": "array", "minItems": 2, "items": {"type": "integer"}}),
        json!({"oneOf": [{"type": "string"}, {"type": "integer"}]}),
    ] {
        assert_eq!(
            check(schema, json!("{{ event.payload.value }}"))
                .unwrap()
                .deferred_paths,
            [""]
        );
    }
    assert!(check(json!(false), json!("{{ value }}")).is_err());
    assert!(check(json!(true), json!("prefix {{ value }} suffix")).is_ok());
}

#[test]
fn literal_siblings_and_presence_constraints_are_checked() {
    let schema = json!({"type": "object", "properties": {
        "known": {"type": "integer", "minimum": 1},
        "pending": {"type": "object", "required": ["nested"]}
    }, "required": ["known", "pending"], "additionalProperties": false});
    for value in [
        json!({"known": "wrong", "pending": "{{ object }}"}),
        json!({"known": 0, "pending": "{{ object }}"}),
        json!({"pending": "{{ object }}"}),
        json!({"known": 1, "pending": "{{ object }}", "extra": "{{ extra }}"}),
    ] {
        assert_eq!(
            check(schema.clone(), value).unwrap_err().code,
            ErrorCode::InvalidValue
        );
    }
    assert_eq!(
        check(schema, json!({"known": 1, "pending": "{{ object }}"}))
            .unwrap()
            .deferred_paths,
        ["/pending"]
    );
}

#[test]
fn nested_objects_arrays_and_escaped_paths_are_not_skipped() {
    let schema = json!({"type": "object", "properties": {
        "a": {"type": "object", "properties": {"x/y~z": {"type": "integer"}, "known": {"type": "boolean"}}},
        "items": {"type": "array", "items": {"type": "object", "properties": {"known": {"type": "integer"}, "pending": {"type": "string", "pattern": "^fixed$"}}}}
    }});
    let value = json!({"a": {"x/y~z": "{{ value }}", "known": true}, "a-": "{{ unrelated }}", "items": [{"known": 1, "pending": "{{ value }}"}]});
    assert_eq!(
        check(schema.clone(), value).unwrap().deferred_paths,
        ["/a-", "/a/x~1y~0z", "/items/0/pending"]
    );
    assert!(check(
        schema,
        json!({"a": {"x/y~z": "{{ value }}", "known": "wrong"}})
    )
    .is_err());
    assert!(check(
        json!({"type": "array", "items": {"type": "integer"}}),
        json!(["{{ value }}", "wrong"])
    )
    .is_err());
}

#[test]
fn array_lengths_tuple_items_and_known_duplicates_remain_enforced() {
    assert!(check(
        json!({"type": "array", "minItems": 2}),
        json!(["{{ value }}"])
    )
    .is_err());
    assert!(check(
        json!({"type": "array", "prefixItems": [{"type": "integer"}], "items": false}),
        json!(["{{ value }}", "{{ extra }}"])
    )
    .is_err());
    assert!(check(
        json!({"type": "array", "uniqueItems": true}),
        json!([1, 1.0, "{{ value }}"])
    )
    .is_err());
    assert!(check(
        json!({"type": "array", "uniqueItems": true}),
        json!([1, 2, "{{ value }}"])
    )
    .is_ok());
}

#[test]
fn enum_and_const_constraints_compare_known_fragments() {
    let schema = json!({"enum": [{"known": 1.0, "pending": "a"}, {"known": 2, "pending": "b"}]});
    assert!(check(
        schema.clone(),
        json!({"known": 1, "pending": "{{ value }}"})
    )
    .is_ok());
    assert!(check(schema, json!({"known": 3, "pending": "{{ value }}"})).is_err());
    assert!(check(json!({"const": [1, "a"]}), json!([2, "{{ value }}"])).is_err());
}

#[test]
fn composition_does_not_invent_branch_failures_or_hide_literal_failures() {
    let branch = json!({"type": "object", "properties": {"pending": {"type": "string"}}});
    let other = json!({"type": "object", "properties": {"pending": {"type": "integer"}}});
    assert!(check(
        json!({"oneOf": [branch, other]}),
        json!({"pending": "{{ value }}"})
    )
    .is_ok());
    assert!(check(
        json!({"oneOf": [{"type": "object"}, {"type": "object"}]}),
        json!({"pending": "{{ value }}"})
    )
    .is_err());
    assert!(check(json!({"anyOf": [{"properties": {"known": {"const": "a"}}}, {"properties": {"known": {"const": "b"}}}]}), json!({"known": "c", "pending": "{{ value }}"})).is_err());
    assert!(check(
        json!({"allOf": [{"properties": {"known": {"type": "integer"}}}]}),
        json!({"known": "wrong", "pending": "{{ value }}"})
    )
    .is_err());
    assert!(check(
        json!({"not": {"properties": {"known": {"const": "a"}}}}),
        json!({"known": "a", "pending": "{{ value }}"})
    )
    .is_err());
    assert!(check(
        json!({"not": {"properties": {"pending": {"const": "a"}}}}),
        json!({"pending": "{{ value }}"})
    )
    .is_ok());
}

#[test]
fn contains_preserves_known_minimum_and_maximum_failures() {
    let schema = json!({"type": "array", "contains": {"type": "integer"}, "minContains": 2, "maxContains": 3});
    assert!(check(schema.clone(), json!(["wrong", "{{ value }}"])).is_err());
    assert!(check(schema.clone(), json!([1, 2, 3, 4, "{{ value }}"])).is_err());
    assert!(check(schema.clone(), json!([1, "{{ value }}"])).is_ok());
    assert!(check(schema, json!([1, 2, "{{ value }}"])).is_ok());
    assert!(check(
        json!({"not": {"contains": {"type": "integer"}}}),
        json!([1, "{{ value }}"])
    )
    .is_err());
}

#[test]
fn authoring_without_templates_matches_strict_validation_and_never_adds_defaults() {
    let contract = compile_inline(
        "test.authoring",
        SchemaContext::SingleValue,
        &json!({"type": "object", "properties": {
        "required": {"type": "integer", "default": 1}
    }, "required": ["required"]}),
    )
    .unwrap();
    let compiled = CompiledContract::new(&contract).unwrap();
    for value in [
        json!({}),
        json!({"required": 1}),
        json!({"required": "wrong"}),
    ] {
        assert_eq!(
            compiled.validate_authoring(&value).is_ok(),
            compiled.validate(&value).is_ok()
        );
    }
}

#[test]
fn named_and_local_refs_share_authoring_semantics() {
    let contract = compile_inline("test.authoring", SchemaContext::FlatDocument, &json!({
        "value": {"$ref": "#/value/$defs/body", "$defs": {"body": {"type": "object", "properties": {"known": {"type": "integer"}, "pending": {"type": "string"}}, "required": ["known"]}}}
    })).unwrap();
    let compiled = CompiledContract::new(&contract).unwrap();
    assert!(compiled
        .validate_authoring(&json!({"value": "{{ object }}"}))
        .is_ok());
    assert!(compiled
        .validate_authoring(&json!({"value": {"known": "wrong", "pending": "{{ value }}"}}))
        .is_err());
    let registry = RegistryView::from([(
        "test.object".into(),
        RegistryEntry::Available(TypeRevision {
            source: SourceIdentity::Released {
                component_ref: "test.object".into(),
                release_id: 1,
                release_digest: "test-release".into(),
            },
            revision_id: 1,
            schema: json!({"type": "object", "properties": {"known": {"type": "integer"}, "pending": {"type": "string"}}, "required": ["known"]}),
        }),
    )]);
    let named = compile(
        SourceIdentity::Authoring {
            component_ref: "test.authoring".into(),
        },
        SchemaContext::FlatDocument,
        &json!({"value": {"type": "test.object"}}),
        &registry,
        Limits::default(),
    )
    .unwrap();
    let named = CompiledContract::new(&named).unwrap();
    for value in [
        json!({"value": "{{ object }}"}),
        json!({"value": {"known": "wrong", "pending": "{{ value }}"}}),
    ] {
        assert_eq!(
            named.validate_authoring(&value),
            compiled.validate_authoring(&value)
        );
    }
}

#[test]
fn bounded_projection_never_rejects_a_valid_rendering_in_a_small_domain() {
    let schemas = [
        json!({"oneOf": [{"properties": {"pending": {"type": "integer"}, "known": {"const": 1}}}, {"properties": {"pending": {"type": "string"}}}]}),
        json!({"not": {"anyOf": [{"properties": {"pending": {"type": "integer"}}}, {"properties": {"known": {"const": 0}}}]}}),
        json!({"oneOf": [{"type": "object"}, {"properties": {"pending": {"type": "integer"}}}]}),
    ];
    for schema in schemas {
        let contract =
            compile_inline("test.authoring", SchemaContext::SingleValue, &schema).unwrap();
        let compiled = CompiledContract::new(&contract).unwrap();
        for known in [0, 1, 2] {
            let partial = json!({"known": known, "pending": "{{ value }}"});
            let valid_rendering = [json!(0), json!("a"), json!(null), json!([]), json!({})]
                .into_iter()
                .any(|pending| {
                    compiled
                        .validate(&json!({"known": known, "pending": pending}))
                        .is_ok()
                });
            if valid_rendering {
                assert!(
                    compiled.validate_authoring(&partial).is_ok(),
                    "schema: {schema}"
                );
            }
        }
    }
}
