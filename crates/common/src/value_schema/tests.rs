use super::*;

#[test]
fn assigned_sources_are_authoritative_without_turning_previews_into_bindings() {
    let schema = serde_json::json!({"type": "string", "default": "public-default"});
    let preview = compile(
        SourceIdentity::Authoring {
            component_ref: "preview".into(),
        },
        SchemaContext::SingleValue,
        &schema,
        &RegistryView::new(),
        Limits::default(),
    )
    .unwrap();
    let assigned = compile(
        SourceIdentity::Assigned {
            component_ref: "system.value".into(),
            assignment_id: uuid::Uuid::new_v4(),
        },
        SchemaContext::SingleValue,
        &schema,
        &RegistryView::new(),
        Limits::default(),
    )
    .unwrap();
    assert_ne!(preview.binding_digest, assigned.binding_digest);
    assert_eq!(
        preview.semantic_digest().unwrap(),
        assigned.semantic_digest().unwrap()
    );
    assert!(CompiledContract::new(&assigned)
        .unwrap()
        .validate(&serde_json::json!("updated value"))
        .is_ok());
}

#[test]
fn validation_diagnostics_never_echo_dynamic_object_keys_or_secret_descendants() {
    let schema = serde_json::json!({"payload": {"type": "object", "secret": true, "additionalProperties": {"type": "string", "minLength": 100}}});
    let contract = compile_inline("test.action", SchemaContext::FlatDocument, &schema).unwrap();
    let failure = CompiledContract::new(&contract)
        .unwrap()
        .validate(&serde_json::json!({"payload": {"private-key-fixture": "private-value-fixture"}}))
        .unwrap_err();
    let diagnostic = serde_json::to_string(&failure).unwrap();
    assert!(!diagnostic.contains("private-key-fixture"));
    assert!(!diagnostic.contains("private-value-fixture"));
    assert_eq!(failure.path, "/payload");
    let schema = serde_json::json!({"additionalProperties": {"type": "integer"}, "items": {"type": "integer"}});
    let contract = compile_inline("test.map", SchemaContext::SingleValue, &schema).unwrap();
    let failure = CompiledContract::new(&contract)
        .unwrap()
        .validate(&serde_json::json!({"987654321001": "invalid"}))
        .unwrap_err();
    assert_eq!(failure.path, "");
}

#[test]
fn secret_ancestors_forbid_nested_default_and_example_literals() {
    for keyword in ["default", "examples"] {
        let literal = if keyword == "examples" {
            serde_json::json!(["fixture-private"])
        } else {
            serde_json::json!("fixture-private")
        };
        let mut child = serde_json::json!({"type": "string"});
        child[keyword] = literal;
        let schema =
            serde_json::json!({"type": "object", "secret": true, "properties": {"value": child}});
        assert_eq!(
            compile_inline("test.private", SchemaContext::SingleValue, &schema)
                .unwrap_err()
                .code,
            ErrorCode::SecretLiteral
        );
    }
}
use serde_json::json;

#[test]
fn secret_schema_domain_rejects_scalar_types_and_public_literals() {
    for kind in ["number", "integer", "boolean", "null"] {
        for schema in [
            json!({"type": kind, "secret": true}),
            json!({"type": "object", "secret": true, "properties": {"value": {"type": kind}}}),
            json!({"type": "array", "secret": true, "items": {"type": kind}}),
            json!({"type": ["string", kind], "secret": true}),
        ] {
            assert_eq!(
                compile_inline("test.private", SchemaContext::SingleValue, &schema)
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidDefinition
            );
        }
    }
    for (keyword, literal) in [
        ("const", json!("public-domain")),
        ("enum", json!(["public-domain"])),
        ("default", json!("public-domain")),
        ("examples", json!([])),
    ] {
        let mut child = json!({"type": "string"});
        child[keyword] = literal;
        let enclosing_literal = if matches!(keyword, "enum" | "examples") {
            json!([{"value": "private"}])
        } else {
            json!({"value": "private"})
        };
        for schema in [
            json!({"type": "object", "secret": true, "properties": {"value": child}}),
            json!({"type": "object", "properties": {"value": {"type": "string", "secret": true}}, keyword: enclosing_literal}),
        ] {
            assert_eq!(
                compile_inline("test.private", SchemaContext::SingleValue, &schema)
                    .unwrap_err()
                    .code,
                ErrorCode::SecretLiteral
            );
        }
    }
}

#[test]
fn secret_references_cannot_hide_forbidden_types_or_literals() {
    for target in [
        json!({"type": "integer"}),
        json!({"type": "string", "enum": ["public-domain"]}),
        json!({"type": "object", "properties": {"value": {"const": "public-domain"}}}),
    ] {
        let view = registry(&[("example.value", target.clone())]);
        assert!(compile(
            source("test.private"),
            SchemaContext::SingleValue,
            &json!({"type": "example.value", "secret": true}),
            &view,
            Limits::default()
        )
        .is_err());
        assert!(compile_inline(
            "test.private",
            SchemaContext::SingleValue,
            &json!({"$defs": {"value": target}, "$ref": "#/$defs/value", "secret": true})
        )
        .is_err());
    }
}

#[test]
fn secret_values_are_text_trees_even_with_unconstrained_children() {
    for schema in [
        json!({"secret": true}),
        json!({"type": ["string", "object", "array"], "secret": true}),
    ] {
        let contract = build(schema, SchemaContext::SingleValue, &RegistryView::new());
        let compiled = CompiledContract::new(&contract).unwrap();
        for value in [
            json!(""),
            json!({}),
            json!([]),
            json!({"nested": ["text", {"token": "private"}]}),
        ] {
            compiled.validate(&value).unwrap();
            compiled.validate_authoring(&value).unwrap();
        }
        for value in [
            json!(1),
            json!(false),
            json!(null),
            json!({"private-key": ["text", 1]}),
        ] {
            assert_eq!(
                compiled.validate(&value).unwrap_err().code,
                ErrorCode::InvalidValue
            );
            assert!(compiled.validate_authoring(&value).is_err());
            // Invalid output must still be classified and protected.
            assert_eq!(compiled.secret_paths(&value).unwrap(), [""]);
        }
    }
}

fn source(name: &str) -> SourceIdentity {
    SourceIdentity::Released {
        component_ref: name.into(),
        release_id: 1,
        release_digest: "release-a".into(),
    }
}

fn registry(definitions: &[(&str, Value)]) -> RegistryView {
    definitions
        .iter()
        .enumerate()
        .map(|(index, (name, schema))| {
            (
                name.to_string(),
                RegistryEntry::Available(TypeRevision {
                    source: source(name),
                    revision_id: index as i64 + 1,
                    schema: schema.clone(),
                }),
            )
        })
        .collect()
}

fn build(schema: Value, context: SchemaContext, registry: &RegistryView) -> Contract {
    compile(
        source("example.action"),
        context,
        &schema,
        registry,
        Limits::default(),
    )
    .unwrap()
}

fn reject(schema: Value, context: SchemaContext, registry: &RegistryView) -> SchemaError {
    compile(
        source("example.action"),
        context,
        &schema,
        registry,
        Limits::default(),
    )
    .unwrap_err()
}

#[test]
fn inline_and_named_validation_defaults_and_secrets_are_equivalent() {
    let address = json!({
        "type": "object", "properties": {
            "city": {"type": "string", "minLength": 1},
            "country": {"type": "string", "enum": ["US", "GB"], "default": "US"},
            "token": {"type": "string", "secret": true}
        }, "required": ["city"], "additionalProperties": false
    });
    let types = registry(&[
        ("example.address", address.clone()),
        (
            "example.severity",
            json!({"type": "string", "enum": ["info", "critical"], "default": "info"}),
        ),
    ]);
    let named = build(
        json!({
            "destination": {"type": "example.address", "required": true},
            "severity": {"type": "example.severity"},
            "alternates": {"type": "array", "items": {"type": "example.address"}}
        }),
        SchemaContext::FlatDocument,
        &types,
    );
    let inline = build(
        json!({
            "destination": {"required": true, "$ref": "#/destination/$defs/body", "$defs": {"body": address}},
            "severity": {"type": "string", "enum": ["info", "critical"], "default": "info"},
            "alternates": {"type": "array", "items": types.get("example.address").and_then(|entry| match entry {
                RegistryEntry::Available(revision) => Some(&revision.schema), _ => None
            }).unwrap()}
        }),
        SchemaContext::FlatDocument,
        &RegistryView::new(),
    );
    let named = CompiledContract::new(&named).unwrap();
    let inline = CompiledContract::new(&inline).unwrap();
    for value in [
        json!({}),
        json!({"destination": {"city": "London", "token": "private"}, "alternates": [{"city": "Boston", "token": "other"}]}),
        json!({"destination": {"city": ""}}),
        json!({"destination": {"country": "US"}}),
        json!({"destination": {"city": "London", "extra": true}}),
        json!({"destination": {"city": "London"}, "severity": "wrong"}),
    ] {
        assert_eq!(
            named.validate(&value).is_ok(),
            inline.validate(&value).is_ok()
        );
        assert_eq!(
            named.apply_defaults(&value).unwrap(),
            inline.apply_defaults(&value).unwrap()
        );
        assert_eq!(
            named.secret_paths(&value).unwrap(),
            inline.secret_paths(&value).unwrap()
        );
    }
    assert_eq!(
        named
            .secret_paths(
                &json!({"destination": {"token": "private"}, "alternates": [{"token": "other"}]})
            )
            .unwrap(),
        vec!["/alternates/0/token", "/destination/token"]
    );
}

#[test]
fn flat_occurrence_and_object_requiredness_do_not_collide() {
    let types = registry(&[(
        "example.object",
        json!({"type": "object", "properties": {"child": {"type": "string"}}, "required": ["child"]}),
    )]);
    let contract = build(
        json!({"value": {"type": "example.object", "required": true}}),
        SchemaContext::FlatDocument,
        &types,
    );
    let compiled = CompiledContract::new(&contract).unwrap();
    assert!(compiled.validate(&json!({})).is_err());
    assert!(compiled.validate(&json!({"value": {}})).is_err());
    assert!(compiled
        .validate(&json!({"value": {"child": "present"}}))
        .is_ok());
    let inline = build(
        json!({"value": {"type": "object", "properties": {"child": {"type": "string"}}, "required": ["child"]}}),
        SchemaContext::FlatDocument,
        &RegistryView::new(),
    );
    let compiled = CompiledContract::new(&inline).unwrap();
    assert!(compiled.validate(&json!({})).is_ok());
    assert!(compiled.validate(&json!({"value": {}})).is_err());
    assert_eq!(
        reject(
            json!({"type": "string", "required": true}),
            SchemaContext::SingleValue,
            &RegistryView::new()
        )
        .code,
        ErrorCode::InvalidDefinition
    );
}

#[test]
fn contexts_are_explicit_and_malformed_flat_documents_fail_closed() {
    for schema in [
        json!(null),
        json!([]),
        json!({"value": "string"}),
        json!({"type": "object", "properties": {}}),
    ] {
        assert_eq!(
            reject(schema, SchemaContext::FlatDocument, &RegistryView::new()).code,
            ErrorCode::InvalidDefinition
        );
    }
    let contract = build(json!({}), SchemaContext::FlatDocument, &RegistryView::new());
    assert!(CompiledContract::new(&contract)
        .unwrap()
        .validate(&json!([]))
        .is_err());
    for schema in [json!(true), json!(false)] {
        let expected = schema.as_bool().unwrap();
        let contract = build(schema, SchemaContext::SingleValue, &RegistryView::new());
        assert_eq!(
            CompiledContract::new(&contract)
                .unwrap()
                .validate(&json!(42))
                .is_ok(),
            expected
        );
    }
}

#[test]
fn annotations_override_without_replacing_the_named_contract() {
    let types = registry(&[(
        "example.level",
        json!({"type": "integer", "minimum": 1, "default": 2, "description": "inherited"}),
    )]);
    let schema = json!({"value": {"type": "example.level", "default": 3, "description": "local", "position": 1}});
    let contract = build(schema.clone(), SchemaContext::FlatDocument, &types);
    assert_eq!(schema["value"]["type"], "example.level");
    let compiled = CompiledContract::new(&contract).unwrap();
    assert_eq!(
        compiled.apply_defaults(&json!({})).unwrap(),
        json!({"value": 3})
    );
    assert!(compiled.validate(&json!({"value": 0})).is_err());
    assert_eq!(
        reject(
            json!({"value": {"type": "example.level", "minimum": 0}}),
            SchemaContext::FlatDocument,
            &types
        )
        .code,
        ErrorCode::StructuralOverride
    );
    assert_eq!(
        reject(
            json!({"value": {"type": "example.level", "default": 0}}),
            SchemaContext::FlatDocument,
            &types
        )
        .code,
        ErrorCode::InvalidDefault
    );
}

#[test]
fn named_reference_failures_are_distinct_and_closed() {
    let types = BTreeMap::from([
        ("example.private".into(), RegistryEntry::Inaccessible),
        ("example.old".into(), RegistryEntry::Retired),
    ]);
    for (name, code) in [
        ("example.missing", ErrorCode::MissingReference),
        ("example.private", ErrorCode::InaccessibleReference),
        ("example.old", ErrorCode::RetiredReference),
        ("short", ErrorCode::InvalidReference),
        ("too.many.parts", ErrorCode::InvalidReference),
    ] {
        let failure = reject(
            json!({"value": {"type": name}}),
            SchemaContext::FlatDocument,
            &types,
        );
        assert_eq!(failure.code, code);
        assert!(failure.path.starts_with("/value"));
    }
    let invalid = registry(&[(
        "example.bad",
        json!({"type": "string", "minLength": "not a length"}),
    )]);
    assert_eq!(
        reject(
            json!({"type": "example.bad"}),
            SchemaContext::SingleValue,
            &invalid
        )
        .code,
        ErrorCode::InvalidDefinition
    );
}

#[test]
fn direct_indirect_optional_array_and_composition_cycles_are_rejected() {
    for body in [
        json!({"type": "example.loop"}),
        json!({"type": "object", "properties": {"optional": {"type": "example.loop"}}}),
        json!({"type": "array", "items": {"type": "example.loop"}}),
        json!({"anyOf": [{"type": "null"}, {"type": "example.loop"}]}),
        json!({"allOf": [{"type": "example.loop"}]}),
    ] {
        let types = registry(&[("example.loop", body)]);
        let failure = reject(
            json!({"type": "example.loop"}),
            SchemaContext::SingleValue,
            &types,
        );
        assert_eq!(failure.code, ErrorCode::Cycle);
        assert!(failure
            .reference_chain
            .first()
            .unwrap()
            .starts_with("example.loop"));
        assert_eq!(
            failure.reference_chain.first(),
            failure.reference_chain.last()
        );
    }
    let types = registry(&[
        ("example.person", json!({"type": "example.organization"})),
        ("example.organization", json!({"type": "example.person"})),
    ]);
    let failure = reject(
        json!({"type": "example.person"}),
        SchemaContext::SingleValue,
        &types,
    );
    assert_eq!(failure.code, ErrorCode::Cycle);
    assert_eq!(
        failure.reference_chain,
        ["example.person", "example.organization", "example.person"]
    );
}

#[test]
fn local_refs_are_scoped_and_only_schema_positions_are_targets() {
    let body = json!({"type": "object", "$defs": {"leaf": {"type": "integer"}}, "properties": {"value": {"$ref": "#/$defs/leaf"}}});
    let types = registry(&[("example.body", body)]);
    let contract = build(
        json!({"$defs": {"leaf": {"type": "string"}}, "type": "object", "properties": {
            "local": {"$ref": "#/$defs/leaf"}, "external": {"type": "example.body"}
        }}),
        SchemaContext::SingleValue,
        &types,
    );
    let compiled = CompiledContract::new(&contract).unwrap();
    assert!(compiled
        .validate(&json!({"local": "string", "external": {"value": 3}}))
        .is_ok());
    assert!(compiled
        .validate(&json!({"external": {"value": "wrong"}}))
        .is_err());
    let failure = reject(
        json!({"default": {"type": "string"}, "$defs": {"bad": {"$ref": "#/default"}}}),
        SchemaContext::SingleValue,
        &RegistryView::new(),
    );
    assert_eq!(failure.code, ErrorCode::InvalidReference);
    let contract = build(
        json!({"type": "object", "$defs": {"root": {"$ref": "#"}}}),
        SchemaContext::SingleValue,
        &RegistryView::new(),
    );
    assert!(CompiledContract::new(&contract)
        .unwrap()
        .validate(&json!({}))
        .is_ok());
}

#[test]
fn local_cycle_checks_include_unused_definitions_and_named_edges() {
    for schema in [
        json!({"$defs": {"loop": {"$ref": "#/$defs/loop"}}}),
        json!({"type": "object", "properties": {"child": {"$ref": "#"}}}),
        json!({"$defs": {"a": {"items": {"$ref": "#/$defs/b"}}, "b": {"oneOf": [{"$ref": "#/$defs/a"}]}}}),
    ] {
        assert_eq!(
            reject(schema, SchemaContext::SingleValue, &RegistryView::new()).code,
            ErrorCode::Cycle
        );
    }
    let types = registry(&[(
        "example.local",
        json!({"$defs": {"loop": {"type": "example.local"}}, "$ref": "#/$defs/loop"}),
    )]);
    assert_eq!(
        reject(
            json!({"type": "example.local"}),
            SchemaContext::SingleValue,
            &types
        )
        .code,
        ErrorCode::Cycle
    );
}

#[test]
fn local_json_pointer_escaping_and_flat_document_scope_work() {
    let contract = build(
        json!({"type": "object", "$defs": {"a/b~c": {"type": "string"}}, "properties": {"a/b~c": {"$ref": "#/$defs/a~1b~0c", "secret": true}}}),
        SchemaContext::SingleValue,
        &RegistryView::new(),
    );
    let compiled = CompiledContract::new(&contract).unwrap();
    assert_eq!(
        compiled.secret_paths(&json!({"a/b~c": "private"})).unwrap(),
        ["/a~1b~0c"]
    );
    let contract = build(
        json!({"one": {"type": "integer"}, "two": {"$ref": "#/one"}}),
        SchemaContext::FlatDocument,
        &RegistryView::new(),
    );
    assert!(CompiledContract::new(&contract)
        .unwrap()
        .validate(&json!({"two": "bad"}))
        .is_err());
}

#[test]
fn external_references_anchors_and_unsupported_vocabulary_are_rejected() {
    for reference in [
        "https://example.com/schema",
        "file:///etc/passwd",
        "../schema.json",
        "#anchor",
        "#/$defs/%61",
        "#/~2bad",
    ] {
        assert_eq!(
            reject(
                json!({"$ref": reference}),
                SchemaContext::SingleValue,
                &RegistryView::new()
            )
            .code,
            ErrorCode::InvalidReference
        );
    }
    for keyword in [
        "$id",
        "$anchor",
        "$dynamicRef",
        "$recursiveRef",
        "if",
        "dependentSchemas",
        "unevaluatedProperties",
        "patternProperties",
    ] {
        let schema = json!({keyword: {}});
        assert_eq!(
            reject(schema, SchemaContext::SingleValue, &RegistryView::new()).code,
            ErrorCode::UnsupportedKeyword
        );
    }
    assert_eq!(
        reject(
            json!({"$schema": "http://json-schema.org/draft-07/schema#"}),
            SchemaContext::SingleValue,
            &RegistryView::new()
        )
        .code,
        ErrorCode::UnsupportedKeyword
    );
}

#[test]
fn defaults_and_examples_are_data_not_schema_nodes() {
    let contract = build(
        json!({"type": "object", "default": {"type": "missing.type", "$ref": "http://unreachable"}, "examples": [{"items": {"type": "also.missing"}}]}),
        SchemaContext::SingleValue,
        &RegistryView::new(),
    );
    assert!(contract.dependencies.is_empty());
    assert!(CompiledContract::new(&contract)
        .unwrap()
        .validate(&json!({"type": "anything"}))
        .is_ok());
}

#[test]
fn defaults_preserve_present_values_and_do_not_create_optional_trees() {
    let contract = build(
        json!({
            "null_value": {"type": ["null", "integer"], "default": 2},
            "false_value": {"type": "boolean", "default": true},
            "zero_value": {"type": "integer", "default": 1},
            "empty_value": {"type": "string", "default": "fallback"},
            "optional": {"type": "object", "properties": {"child": {"type": "integer", "default": 1}}},
            "explicit_object": {"type": "object", "default": {}, "properties": {"child": {"type": "integer", "default": 1}}},
            "array": {"type": "array", "items": {"type": "object", "properties": {"child": {"type": "integer", "default": 1}}}}
        }),
        SchemaContext::FlatDocument,
        &RegistryView::new(),
    );
    let compiled = CompiledContract::new(&contract).unwrap();
    let input = json!({"null_value": null, "false_value": false, "zero_value": 0, "empty_value": "", "array": [{}, {"child": 0}]});
    let defaulted = compiled.apply_defaults(&input).unwrap();
    assert_eq!(
        defaulted,
        json!({"null_value": null, "false_value": false, "zero_value": 0, "empty_value": "", "explicit_object": {"child": 1}, "array": [{"child": 1}, {"child": 0}]})
    );
    assert_eq!(compiled.apply_defaults(&defaulted).unwrap(), defaulted);
    assert!(compiled.validate(&defaulted).is_ok());
    assert!(!input.as_object().unwrap().contains_key("explicit_object"));
}

#[test]
fn composition_defaults_are_rejected_but_explicit_node_defaults_are_validated() {
    for keyword in ["allOf", "anyOf", "oneOf"] {
        assert_eq!(reject(json!({keyword: [{"type": "object", "properties": {"child": {"type": "integer", "default": 1}}}]}), SchemaContext::SingleValue, &RegistryView::new()).code, ErrorCode::InvalidDefault);
    }
    let types = registry(&[(
        "example.defaulted",
        json!({"type": "integer", "default": 1}),
    )]);
    assert_eq!(
        reject(
            json!({"anyOf": [{"type": "example.defaulted"}]}),
            SchemaContext::SingleValue,
            &types
        )
        .code,
        ErrorCode::InvalidDefault
    );
    build(
        json!({"anyOf": [{"type": "integer"}, {"type": "string"}], "default": 1}),
        SchemaContext::SingleValue,
        &RegistryView::new(),
    );
}

#[test]
fn inherited_secret_marking_cannot_be_removed_and_literal_secrets_are_rejected() {
    let types = registry(&[
        ("example.secret", json!({"type": "string", "secret": true})),
        ("example.alias", json!({"type": "example.secret"})),
    ]);
    for name in ["example.secret", "example.alias"] {
        assert_eq!(
            reject(
                json!({"type": name, "secret": false}),
                SchemaContext::SingleValue,
                &types
            )
            .code,
            ErrorCode::SecretDowngrade
        );
        let contract = build(json!({"type": name}), SchemaContext::SingleValue, &types);
        assert_eq!(
            CompiledContract::new(&contract)
                .unwrap()
                .secret_paths(&json!("private"))
                .unwrap(),
            [""]
        );
    }
    for schema in [
        json!({"type": "string", "secret": true, "default": "private"}),
        json!({"type": "string", "secret": true, "examples": ["private"]}),
        json!({"type": "object", "default": {"token": "private"}, "properties": {"token": {"type": "string", "secret": true}}}),
    ] {
        let failure = reject(schema, SchemaContext::SingleValue, &RegistryView::new());
        assert_eq!(failure.code, ErrorCode::SecretLiteral);
        assert!(!failure.to_string().contains("private"));
    }
    let types = registry(&[(
        "example.public",
        json!({"type": "string", "default": "private"}),
    )]);
    assert_eq!(
        reject(
            json!({"type": "example.public", "secret": true}),
            SchemaContext::SingleValue,
            &types
        )
        .code,
        ErrorCode::SecretLiteral
    );
}

#[test]
fn composition_secrets_are_conservative_and_arrays_have_concrete_paths() {
    let contract = build(
        json!({"type": "object", "properties": {
            "value": {"anyOf": [
                {"type": "object", "properties": {"mode": {"const": "a"}, "token": {"secret": true}}},
                {"type": "object", "properties": {"mode": {"const": "b"}}}
            ]},
            "items": {"type": "array", "prefixItems": [{"secret": true}], "items": {"type": "object", "properties": {"password": {"secret": true}}}},
            "contains": {"type": "array", "contains": {"secret": true}}
        }}),
        SchemaContext::SingleValue,
        &RegistryView::new(),
    );
    let compiled = CompiledContract::new(&contract).unwrap();
    assert_eq!(compiled.secret_paths(&json!({"value": {"mode": "b", "token": "private"}, "items": ["first", {"password": "second"}], "contains": [1, 2]})).unwrap(), ["/contains/0", "/contains/1", "/items/0", "/items/1/password", "/value/token"]);
    assert!(compiled.secret_paths(&json!({})).unwrap().is_empty());
}

#[test]
fn validator_errors_never_contain_submitted_values() {
    let contract = build(
        json!({"token": {"type": "string", "minLength": 100, "secret": true}}),
        SchemaContext::FlatDocument,
        &RegistryView::new(),
    );
    let failure = CompiledContract::new(&contract)
        .unwrap()
        .validate(&json!({"token": "VERY_PRIVATE_VALUE"}))
        .unwrap_err();
    assert_eq!(failure.path, "/token");
    for text in [
        failure.to_string(),
        format!("{failure:?}"),
        serde_json::to_string(&failure).unwrap(),
    ] {
        assert!(!text.contains("VERY_PRIVATE_VALUE"));
    }
}

#[test]
fn runtime_templates_are_not_authoring_placeholders() {
    let contract = build(
        json!({"value": {"type": "object", "properties": {"child": {"type": "integer"}}, "required": ["child"]}}),
        SchemaContext::FlatDocument,
        &RegistryView::new(),
    );
    let compiled = CompiledContract::new(&contract).unwrap();
    assert!(compiled
        .validate(&json!({"value": "{{ event.payload.value }}"}))
        .is_err());
    assert!(compiled
        .validate(&json!({"value": {"child": "{{ event.payload.child }}"}}))
        .is_err());
}

#[test]
fn contracts_are_deterministic_self_contained_and_sensitive_to_revisions() {
    let types = registry(&[(
        "example.value",
        json!({"type": "string", "enum": ["a", "b"]}),
    )]);
    let schema = json!({"first": {"type": "example.value"}, "second": {"type": "example.value"}});
    let first = build(schema.clone(), SchemaContext::FlatDocument, &types);
    let second = build(
        serde_json::from_str(
            r#"{"second":{"type":"example.value"},"first":{"type":"example.value"}}"#,
        )
        .unwrap(),
        SchemaContext::FlatDocument,
        &types,
    );
    assert_eq!(first, second);
    assert_eq!(first.dependencies.len(), 1);
    let stored: Contract = serde_json::from_value(serde_json::to_value(&first).unwrap()).unwrap();
    assert!(CompiledContract::new(&stored)
        .unwrap()
        .validate(&json!({"first": "a"}))
        .is_ok());
    let mut updated = types.clone();
    if let RegistryEntry::Available(revision) = updated.get_mut("example.value").unwrap() {
        revision.revision_id += 1;
        if let SourceIdentity::Released { release_id, .. } = &mut revision.source {
            *release_id += 1;
        }
    }
    assert_ne!(
        first.binding_digest,
        build(schema, SchemaContext::FlatDocument, &updated).binding_digest
    );
    let mut tampered = stored;
    tampered.validator_schema = json!(true);
    assert!(CompiledContract::new(&tampered).is_err());
}

#[test]
fn shared_graphs_do_not_expand_exponentially() {
    let mut definitions = vec![("example.n0".to_owned(), json!({"type": "string"}))];
    for depth in 1..10 {
        definitions.push((format!("example.n{depth}"), json!({"type": "object", "properties": {
            "left": {"type": format!("example.n{}", depth - 1)}, "right": {"type": format!("example.n{}", depth - 1)}
        }})));
    }
    let types = registry(
        &definitions
            .iter()
            .map(|(name, schema)| (name.as_str(), schema.clone()))
            .collect::<Vec<_>>(),
    );
    let contract = build(
        json!({"type": "example.n9"}),
        SchemaContext::SingleValue,
        &types,
    );
    assert_eq!(contract.dependencies.len(), 10);
    assert_eq!(
        contract.annotated_schema["$defs"]
            .as_object()
            .unwrap()
            .len(),
        29
    );
    assert!(
        serde_json::to_vec(&contract.annotated_schema)
            .unwrap()
            .len()
            < 20_000
    );
}

#[test]
fn limits_cover_sources_nodes_dependencies_compiled_size_and_literals() {
    let schema = json!({"type": "object", "properties": {"one": {"type": "string"}, "two": {"type": "integer"}}});
    for limits in [
        Limits {
            max_schema_nodes: 2,
            ..Limits::default()
        },
        Limits {
            max_source_bytes: 20,
            ..Limits::default()
        },
        Limits {
            max_compiled_bytes: 20,
            ..Limits::default()
        },
        Limits {
            max_depth: 1,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            compile(
                source("example.action"),
                SchemaContext::SingleValue,
                &schema,
                &RegistryView::new(),
                limits
            )
            .unwrap_err()
            .code,
            ErrorCode::ResourceLimit
        );
    }
    let types = registry(&[
        ("example.a", json!({"type": "example.b"})),
        ("example.b", json!({"type": "example.c"})),
        ("example.c", json!({"type": "string"})),
    ]);
    assert_eq!(
        compile(
            source("example.action"),
            SchemaContext::SingleValue,
            &json!({"type": "example.a"}),
            &types,
            Limits {
                max_depth: 2,
                ..Limits::default()
            }
        )
        .unwrap_err()
        .code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        compile(
            source("example.action"),
            SchemaContext::SingleValue,
            &json!({"type": "string", "default": "a"}),
            &RegistryView::new(),
            Limits {
                max_literal_checks: 0,
                ..Limits::default()
            }
        )
        .unwrap_err()
        .code,
        ErrorCode::ResourceLimit
    );
}

#[test]
fn runtime_limits_cover_default_amplification_and_secret_traversal() {
    let schema = json!({"a": {"type": "string", "default": "abcdefgh"}, "b": {"type": "string", "default": "abcdefgh"}});
    let contract = compile(
        source("example.action"),
        SchemaContext::FlatDocument,
        &schema,
        &RegistryView::new(),
        Limits {
            max_value_bytes: 20,
            ..Limits::default()
        },
    )
    .unwrap();
    let compiled = CompiledContract::new(&contract).unwrap();
    assert_eq!(
        compiled.apply_defaults(&json!({})).unwrap_err().code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        compiled
            .validate(&json!({"long": "abcdefghijklmnopqrstuvwxyz"}))
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    let contract = compile(
        source("example.action"),
        SchemaContext::SingleValue,
        &json!({"type": "array", "items": {"secret": true}}),
        &RegistryView::new(),
        Limits {
            max_value_nodes: 10,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        CompiledContract::new(&contract)
            .unwrap()
            .secret_paths(&json!([1, 2, 3, 4, 5, 6, 7, 8, 9]))
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
}

#[test]
fn declaration_errors_report_original_source_locations() {
    let types = registry(&[(
        "example.bad",
        json!({"type": "object", "properties": {
            "child": {"type": "integer", "default": "not an integer"}
        }}),
    )]);
    let failure = reject(
        json!({"type": "example.bad"}),
        SchemaContext::SingleValue,
        &types,
    );
    assert_eq!(failure.code, ErrorCode::InvalidDefault);
    assert_eq!(failure.source_ref.as_deref(), Some("example.bad"));
    assert_eq!(failure.path, "/properties/child/default");
    let types = registry(&[(
        "example.bad",
        json!({"type": "string", "minLength": "not a length"}),
    )]);
    let failure = reject(
        json!({"type": "example.bad"}),
        SchemaContext::SingleValue,
        &types,
    );
    assert_eq!(failure.source_ref.as_deref(), Some("example.bad"));
    assert_eq!(failure.path, "/minLength");
}

#[test]
fn validator_annotation_stripping_does_not_modify_const_or_enum_data() {
    let literal = json!({"secret": true, "position": 7, "description": "literal"});
    for schema in [json!({"const": literal}), json!({"enum": [literal]})] {
        let contract = build(schema, SchemaContext::SingleValue, &RegistryView::new());
        let compiled = CompiledContract::new(&contract).unwrap();
        assert!(compiled.validate(&literal).is_ok());
        assert!(compiled.validate(&json!({})).is_err());
        assert!(compiled.secret_paths(&literal).unwrap().is_empty());
    }
}

#[test]
fn memoized_subgraphs_still_enforce_dependency_depth() {
    let types = registry(&[
        ("example.a", json!({"type": "example.b"})),
        ("example.b", json!({"type": "example.c"})),
        ("example.c", json!({"type": "example.d"})),
        ("example.d", json!({"type": "string"})),
    ]);
    let failure = compile(
        source("example.action"),
        SchemaContext::FlatDocument,
        &json!({
            "first": {"type": "example.b"},
            "second": {"type": "example.a"}
        }),
        &types,
        Limits {
            max_depth: 4,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, ErrorCode::ResourceLimit);
    let contract = compile(
        source("example.action"),
        SchemaContext::SingleValue,
        &json!({"type": "example.a"}),
        &types,
        Limits {
            max_depth: 4,
            ..Limits::default()
        },
    )
    .unwrap();
    let compiled = CompiledContract::new(&contract).unwrap();
    assert!(compiled.secret_paths(&json!("value")).unwrap().is_empty());
}

#[test]
fn old_contracts_do_not_consult_updated_registry_bodies() {
    let mut types = registry(&[("example.value", json!({"type": "string", "enum": ["old"]}))]);
    let authored = json!({"type": "example.value"});
    let old = build(authored.clone(), SchemaContext::SingleValue, &types);
    if let RegistryEntry::Available(revision) = types.get_mut("example.value").unwrap() {
        revision.schema = json!({"type": "string", "enum": ["new"]});
        revision.revision_id += 1;
    }
    let new = build(authored, SchemaContext::SingleValue, &types);
    assert_ne!(
        old.dependencies[0].content_digest,
        new.dependencies[0].content_digest
    );
    let old = CompiledContract::new(&old).unwrap();
    let new = CompiledContract::new(&new).unwrap();
    assert!(old.validate(&json!("old")).is_ok());
    assert!(new.validate(&json!("old")).is_err());
    assert!(old.validate(&json!("new")).is_err());
    assert!(new.validate(&json!("new")).is_ok());
}

#[test]
fn validation_work_is_bounded_for_shared_composition_graphs() {
    let mut definitions = vec![("example.n0".to_owned(), json!({"type": "string"}))];
    for depth in 1..12 {
        definitions.push((
            format!("example.n{depth}"),
            json!({"allOf": [
                {"type": format!("example.n{}", depth - 1)},
                {"type": format!("example.n{}", depth - 1)}
            ]}),
        ));
    }
    let types = registry(
        &definitions
            .iter()
            .map(|(name, schema)| (name.as_str(), schema.clone()))
            .collect::<Vec<_>>(),
    );
    let contract = compile(
        source("example.action"),
        SchemaContext::SingleValue,
        &json!({"type": "example.n11"}),
        &types,
        Limits {
            max_value_nodes: 1_000,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        CompiledContract::new(&contract)
            .unwrap()
            .validate(&json!("tiny"))
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
}

#[test]
fn formats_are_retained_annotations_without_new_enforcement() {
    let previous = jsonschema::Validator::new(&json!({"type": "object", "properties": {
        "time": {"type": "string", "format": "date-time"}
    }}))
    .unwrap();
    assert!(previous.is_valid(&json!({"time": "not a date"})));
    let contract = compile_inline(
        "test.format",
        SchemaContext::FlatDocument,
        &json!({"time": {"type": "string", "format": "date-time"}}),
    )
    .unwrap();
    assert!(contract.annotated_schema.to_string().contains("date-time"));
    assert!(!contract.validator_schema.to_string().contains("date-time"));
    assert!(CompiledContract::new(&contract)
        .unwrap()
        .validate(&json!({"time": "not a date"}))
        .is_ok());
    assert!(compile_inline(
        "test.format",
        SchemaContext::SingleValue,
        &json!({"format": 3})
    )
    .is_err());
    assert!(matches!(contract.source, SourceIdentity::Authoring { .. }));
}

#[test]
fn registry_revisions_must_not_be_authoring_previews() {
    let types = RegistryView::from([(
        "example.value".into(),
        RegistryEntry::Available(TypeRevision {
            source: SourceIdentity::Authoring {
                component_ref: "example.value".into(),
            },
            revision_id: 1,
            schema: json!({"type": "string"}),
        }),
    )]);
    assert_eq!(
        reject(
            json!({"type": "example.value"}),
            SchemaContext::SingleValue,
            &types
        )
        .code,
        ErrorCode::InvalidReference
    );
}

#[test]
fn repository_core_schema_definitions_use_the_supported_contract() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/core");
    let mut checked = 0;
    for entry in walkdir::WalkDir::new(&root) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("yaml")
        {
            continue;
        }
        let authored: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
        for field in [
            "parameters",
            "param_schema",
            "output",
            "output_schema",
            "out_schema",
            "conf_schema",
            "item_schema",
        ] {
            if let Some(schema) = authored.get(field) {
                let schema = serde_json::to_value(schema).unwrap();
                compile_inline(
                    format!("{}#{field}", entry.path().display()),
                    SchemaContext::FlatDocument,
                    &schema,
                )
                .unwrap_or_else(|error| panic!("{}#{field}: {error}", entry.path().display()));
                checked += 1;
            }
        }
    }
    assert!(checked > 20);
}

#[test]
fn candidate_types_are_authoring_only_and_have_no_fabricated_revision_ids() {
    let registry = RegistryView::from([(
        "example.value".into(),
        RegistryEntry::Candidate(json!({"type": "integer"})),
    )]);
    let contract = compile(
        SourceIdentity::Authoring {
            component_ref: "example.action".into(),
        },
        SchemaContext::SingleValue,
        &json!({"type": "example.value"}),
        &registry,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(contract.dependencies[0].revision_id, None);
    assert!(matches!(
        contract.dependencies[0].source,
        SourceIdentity::Authoring { .. }
    ));
    assert_eq!(
        compile(
            source("example.action"),
            SchemaContext::SingleValue,
            &json!({"type": "example.value"}),
            &registry,
            Limits::default()
        )
        .unwrap_err()
        .code,
        ErrorCode::InvalidReference
    );
}

#[test]
fn reference_inspection_checks_schema_positions_without_claiming_resolution() {
    let schema = json!({"value": {"type": "array", "items": {"type": "other.value"}, "default": [{"type": "not.a.schema"}]}});
    assert_eq!(
        inspect_definition(SchemaContext::FlatDocument, &schema, Limits::default()).unwrap(),
        [ReferenceUse {
            type_ref: "other.value".into(),
            path: "/value/items".into()
        }]
    );
    assert!(inspect_definition(
        SchemaContext::SingleValue,
        &json!({"type": "object", "properties": {"child": {"minLength": "invalid"}}}),
        Limits::default()
    )
    .is_err());
    assert!(inspect_definition(SchemaContext::SingleValue, &json!({"$schema": DIALECT, "$defs": {"value": {"type": "integer"}}, "$ref": "#/$defs/value"}), Limits::default()).is_ok());
    assert!(inspect_definition(
        SchemaContext::SingleValue,
        &json!({"$ref": "https://unreachable.invalid/schema"}),
        Limits::default()
    )
    .is_err());
    assert!(inspect_definition(
        SchemaContext::SingleValue,
        &json!({"allOf": []}),
        Limits::default()
    )
    .is_err());
}
