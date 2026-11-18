//! Infrastructure-free tests of the public action argument planning interface.

use attune_common::action_cli::{
    ArgumentLimits, ArgumentPlan, Error, ErrorCode, InputDelivery, LiteralArgument,
};
use attune_common::value_schema::{
    self, CompiledContract, Limits, RegistryEntry, RegistryView, SchemaContext, SourceIdentity,
    TypeRevision,
};
use serde_json::{json, Value};

fn source() -> SourceIdentity {
    SourceIdentity::Released {
        component_ref: "test.command".into(),
        release_id: 42,
        release_digest: "a".repeat(64),
    }
}

fn compile(schema: Value, literals: &[LiteralArgument]) -> Result<ArgumentPlan, Error> {
    ArgumentPlan::compile(
        source(),
        &schema,
        literals,
        &RegistryView::new(),
        Limits::default(),
    )
}

fn named_registry(name: &str, schema: Value) -> RegistryView {
    RegistryView::from([(
        name.into(),
        RegistryEntry::Available(TypeRevision {
            source: SourceIdentity::Released {
                component_ref: name.into(),
                release_id: 43,
                release_digest: "b".repeat(64),
            },
            revision_id: 44,
            schema,
        }),
    )])
}

fn materialize(plan: &ArgumentPlan, input: Value) -> attune_common::action_cli::PreparedArguments {
    let admitted = CompiledContract::new(plan.contract())
        .unwrap()
        .apply_defaults(&input)
        .unwrap();
    plan.materialize(
        "/usr/bin/test-tool",
        &admitted,
        &[],
        InputDelivery::None,
        ArgumentLimits::default(),
    )
    .unwrap()
}

fn code(error: Error) -> ErrorCode {
    match error {
        Error::Argument { code, .. } => code,
        other => panic!("expected argument error, got {other}"),
    }
}

#[test]
fn defaults_overrides_literals_flags_and_operands_share_one_order() {
    let plan = compile(
        json!({
            "format": {"type":"string", "enum":["json","csv"], "default":"json",
                "cli":{"kind":"option","name":"--format","order":10}},
            "verbose": {"type":"boolean", "default":false,
                "cli":{"kind":"flag","name":"--verbose","order":20}},
            "destination": {"type":"string", "required":true,
                "cli":{"kind":"positional","order":100}}
        }),
        &[
            LiteralArgument {
                literal: "--".into(),
                order: 90,
            },
            LiteralArgument {
                literal: "generate".into(),
                order: 0,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        materialize(&plan, json!({"destination":"report folder/output.txt"})).argv(),
        &[
            "generate",
            "--format",
            "json",
            "--",
            "report folder/output.txt"
        ]
    );
    assert_eq!(
        materialize(
            &plan,
            json!({"destination":"report.txt","format":"csv","verbose":true})
        )
        .argv(),
        &[
            "generate",
            "--format",
            "csv",
            "--verbose",
            "--",
            "report.txt"
        ]
    );
}

#[test]
fn option_styles_expansion_and_json_encoding_preserve_groups() {
    let plan = compile(json!({
        "include": {"type":"array", "items":{"type":"string"},
            "cli":{"kind":"option","name":"-I","style":"attached","order":10,"expand":true}},
        "mode": {"type":"integer", "cli":{"kind":"option","name":"--mode","style":"equals","order":20}},
        "objects": {"type":"array", "items":{"type":"object"},
            "cli":{"kind":"positional","order":30,"expand":true,"encoding":"json"}},
        "list": {"type":"array", "cli":{"kind":"positional","order":40,"encoding":"json"}}
    }), &[]).unwrap();
    let prepared = materialize(
        &plan,
        json!({"include":["a b","c"],"mode":0,
        "objects":[{"z":1,"a":{"y":false,"b":2}}],"list":[1,null,{"x":2}]}),
    );
    assert_eq!(
        prepared.argv(),
        &[
            "-Ia b",
            "-Ic",
            "--mode=0",
            r#"{"a":{"b":2,"y":false},"z":1}"#,
            r#"[1,null,{"x":2}]"#
        ]
    );
}

#[test]
fn literal_dots_slashes_brackets_and_tildes_are_property_names() {
    let plan = compile(
        json!({
            "root.key": {"type":"object", "properties": {
                "x/y~[0]": {"type":"string", "cli":{"kind":"positional","order":10}}
            }}
        }),
        &[],
    )
    .unwrap();
    let value = "$(touch /tmp/not-executed); ' \" * \n 世界";
    assert_eq!(
        materialize(&plan, json!({"root.key":{"x/y~[0]":value}})).argv(),
        &[value]
    );
    let error = plan
        .materialize(
            "tool",
            &json!({}),
            &[],
            InputDelivery::None,
            ArgumentLimits::default(),
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "action.cli.missing_value at [\"root.key\"][\"x/y~[0]\"]"
    );
}

#[test]
fn absent_false_empty_and_null_are_distinct() {
    let plan = compile(json!({
        "flag": {"type":"boolean", "cli":{"kind":"flag","name":"--yes","false_name":"--no","order":0}},
        "value": {"type":["string","null"], "cli":{"kind":"option","name":"--value","order":1}}
    }), &[]).unwrap();
    assert!(materialize(&plan, json!({})).argv().is_empty());
    assert_eq!(
        materialize(&plan, json!({"flag":false,"value":""})).argv(),
        &["--no", "--value", ""]
    );
    let error = plan
        .materialize(
            "tool",
            &json!({"value":null}),
            &[],
            InputDelivery::None,
            ArgumentLimits::default(),
        )
        .unwrap_err();
    assert_eq!(code(error), ErrorCode::InvalidValue);
}

#[test]
fn required_binding_checks_do_not_shift_missing_positionals() {
    for cli in [
        json!({"kind":"positional","order":0}),
        json!({"kind":"option","name":"--value","order":0,"missing":"error"}),
        json!({"kind":"flag","name":"--flag","order":0,"missing":"error"}),
    ] {
        let kind = if cli["kind"] == "flag" {
            "boolean"
        } else {
            "string"
        };
        let plan = compile(json!({"value":{"type":kind,"cli":cli}}), &[]).unwrap();
        let error = plan
            .materialize(
                "tool",
                &json!({}),
                &[],
                InputDelivery::None,
                ArgumentLimits::default(),
            )
            .unwrap_err();
        assert_eq!(code(error), ErrorCode::MissingValue);
    }
}

#[test]
fn omitted_flags_still_reserve_their_order() {
    let error = compile(
        json!({"flag":{"type":"boolean","cli":{"kind":"flag","name":"--flag","order":0}}}),
        &[LiteralArgument {
            literal: "literal".into(),
            order: 0,
        }],
    )
    .err()
    .unwrap();
    assert_eq!(code(error), ErrorCode::DuplicateOrder);
}

#[test]
fn malformed_annotations_and_unsupported_domains_fail_closed() {
    for cli in [
        Value::Null,
        json!({"kind":"positional","order":-1}),
        json!({"kind":"positional","order":2147483648_i64}),
        json!({"kind":"positional","order":0,"parameter":"other"}),
        json!({"kind":"positional","order":0,"missing":"omit"}),
        json!({"kind":"option","order":0,"name":"--"}),
        json!({"kind":"option","order":0,"name":""}),
        json!({"kind":"option","order":0,"name":"--x\0"}),
        json!({"kind":"option","order":0,"name":"--x","style":"shell"}),
        json!({"kind":"flag","order":0,"name":"--x","expand":true}),
        json!({"kind":"flag","order":0,"name":"--x","false_name":null}),
    ] {
        let error = compile(json!({"value":{"type":"string","cli":cli}}), &[])
            .err()
            .unwrap();
        assert_eq!(code(error), ErrorCode::InvalidBinding);
    }
    for field in [
        json!({"type":"object","cli":{"kind":"positional","order":0}}),
        json!({"cli":{"kind":"positional","order":0}}),
        json!({"type":"string","cli":{"kind":"flag","order":0,"name":"--x"}}),
        json!({"type":"string","cli":{"kind":"positional","order":0,"expand":true}}),
        json!({"type":"array","cli":{"kind":"positional","order":0,"expand":true}}),
    ] {
        assert_eq!(
            code(compile(json!({"value":field}), &[]).err().unwrap()),
            ErrorCode::InvalidBinding
        );
    }
}

#[test]
fn ordinary_schema_compilation_still_rejects_cli() {
    let field = json!({"type":"string","cli":{"kind":"positional","order":0}});
    assert_eq!(
        value_schema::compile_inline(
            "test.queue",
            SchemaContext::FlatDocument,
            &json!({"value":field})
        )
        .unwrap_err()
        .code,
        value_schema::ErrorCode::UnsupportedKeyword
    );
    assert_eq!(
        value_schema::compile_inline("test.type", SchemaContext::SingleValue, &field)
            .unwrap_err()
            .code,
        value_schema::ErrorCode::UnsupportedKeyword
    );
}

#[test]
fn annotations_in_repeated_or_conditional_schema_positions_are_rejected() {
    let field = json!({"type":"string","cli":{"kind":"positional","order":0}});
    for schema in [
        json!({"value":{"type":"array","items":field}}),
        json!({"value":{"type":"array","prefixItems":[field]}}),
        json!({"value":{"type":"object","additionalProperties":field}}),
        json!({"value":{"anyOf":[field]}}),
        json!({"value":{"type":"string","$defs":{"unused":field}}}),
    ] {
        assert!(matches!(compile(schema, &[]), Err(Error::Schema(_))));
    }
}

#[test]
fn named_types_bind_at_the_action_occurrence_and_do_not_publish_cli() {
    let registry = named_registry("types.path", json!({"type":"string"}));
    let schema = json!({"path":{"type":"types.path","cli":{"kind":"positional","order":0}}});
    let plan = ArgumentPlan::compile(source(), &schema, &[], &registry, Limits::default()).unwrap();
    assert_eq!(materialize(&plan, json!({"path":"a b"})).argv(), &["a b"]);
    let invalid_registry = named_registry(
        "types.path",
        json!({"type":"string","cli":{"kind":"positional","order":0}}),
    );
    assert!(
        ArgumentPlan::compile(source(), &schema, &[], &invalid_registry, Limits::default())
            .is_err()
    );
}

#[test]
fn local_aliases_do_not_inherit_bindings_and_binding_edits_change_contract_identity() {
    let schema = json!({
        "first":{"type":"string","cli":{"kind":"positional","order":0}},
        "second":{"$ref":"#/first"}
    });
    let plan = compile(schema.clone(), &[]).unwrap();
    assert_eq!(
        materialize(&plan, json!({"first":"one","second":"two"})).argv(),
        &["one"]
    );
    let mut edited = schema;
    edited["first"]["cli"]["order"] = json!(5);
    let changed = compile(edited, &[]).unwrap();
    assert_ne!(
        plan.contract().authored_digest,
        changed.contract().authored_digest
    );
    assert_ne!(
        plan.contract().binding_digest,
        changed.contract().binding_digest
    );
    assert_eq!(
        plan.contract().validator_schema,
        changed.contract().validator_schema
    );
    assert!(plan
        .contract()
        .annotated_schema
        .to_string()
        .contains("\"cli\""));
    assert!(!plan
        .contract()
        .validator_schema
        .to_string()
        .contains("\"cli\""));
}

#[test]
fn schema_secrecy_in_ancestors_descendants_and_nonmatching_branches_is_rejected() {
    let leaf = json!({"type":"string","cli":{"kind":"positional","order":0}});
    for schema in [
        json!({"secret":{"type":"string","secret":true,"cli":{"kind":"positional","order":0}}}),
        json!({"parent":{"type":"object","secret":true,"properties":{"child":leaf}}}),
        json!({"parent":{"type":"object","properties":{"child":{"type":"string","secret":true}},
            "cli":{"kind":"positional","encoding":"json","order":0}}}),
        json!({"parent":{"type":"object","properties":{"child":leaf},
            "anyOf":[{"properties":{"child":{"type":"string","secret":true}}}, {"properties":{"child":{"type":"string"}}}]}}),
    ] {
        assert_eq!(
            code(compile(schema, &[]).err().unwrap()),
            ErrorCode::SecretValue
        );
    }
    let registry = named_registry("types.secret", json!({"type":"string","secret":true}));
    let schema = json!({"secret":{"type":"types.secret","cli":{"kind":"positional","order":0}}});
    let error = ArgumentPlan::compile(source(), &schema, &[], &registry, Limits::default())
        .err()
        .unwrap();
    assert_eq!(code(error), ErrorCode::SecretValue);
}

#[test]
fn a_secret_sibling_does_not_classify_a_public_argument() {
    let plan = compile(
        json!({"parent":{"type":"object","properties":{
            "public":{"type":"string","cli":{"kind":"positional","order":0}},
            "private":{"type":"string","secret":true}
        }}}),
        &[],
    )
    .unwrap();
    let prepared = plan
        .materialize(
            "tool",
            &json!({"parent":{"public":"ok","private":"private-fixture"}}),
            &[],
            InputDelivery::Stdin,
            ArgumentLimits::default(),
        )
        .unwrap();
    assert_eq!(prepared.argv(), &["ok"]);
    assert!(prepared.stdin_json().unwrap().contains("private-fixture"));
    assert!(!format!("{prepared:?}").contains("private-fixture"));
    assert!(!format!("{prepared:?}").contains("ok"));
}

#[test]
fn recorded_protection_overlaps_ancestors_descendants_and_expanded_items() {
    for (schema, input, paths) in [
        (
            json!({"parent":{"type":"object","properties":{"value":{"type":"string","cli":{"kind":"positional","order":0}}}}}),
            json!({"parent":{"value":"private-fixture"}}),
            vec!["/parent".into()],
        ),
        (
            json!({"parent":{"type":"object","cli":{"kind":"positional","order":0,"encoding":"json"}}}),
            json!({"parent":{"value":"private-fixture"}}),
            vec!["/parent/value".into()],
        ),
        (
            json!({"items":{"type":"array","items":{"type":"string"},"cli":{"kind":"positional","order":0,"expand":true}}}),
            json!({"items":["public","private-fixture"]}),
            vec!["/items/1".into()],
        ),
    ] {
        let plan = compile(schema, &[]).unwrap();
        let error = plan
            .materialize(
                "tool",
                &input,
                &paths,
                InputDelivery::Stdin,
                ArgumentLimits::default(),
            )
            .unwrap_err();
        assert!(!format!("{error:?}").contains("private-fixture"));
        assert_eq!(code(error), ErrorCode::SecretValue);
    }
}

#[test]
fn pointer_prefixes_and_escaped_keys_do_not_confuse_provenance() {
    let plan = compile(
        json!({"x":{"type":"string","cli":{"kind":"positional","order":0}},
        "x2":{"type":"string"},"x/y":{"type":"string"}}),
        &[],
    )
    .unwrap();
    let input = json!({"x":"public","x2":"private-fixture","x/y":"private-fixture"});
    let prepared = plan
        .materialize(
            "tool",
            &input,
            &["/x2".into(), "/x~1y".into()],
            InputDelivery::Stdin,
            ArgumentLimits::default(),
        )
        .unwrap();
    assert_eq!(prepared.argv(), &["public"]);
    for invalid in ["x", "/x~", "/x~2y"] {
        let error = plan
            .materialize(
                "tool",
                &input,
                &[invalid.into()],
                InputDelivery::Stdin,
                ArgumentLimits::default(),
            )
            .unwrap_err();
        assert_eq!(code(error), ErrorCode::InvalidValue);
    }
}

#[test]
fn no_delivery_rejects_protected_inputs_even_when_unbound() {
    let plan = compile(json!({"private":{"type":"string","secret":true}}), &[]).unwrap();
    assert!(materialize(&plan, json!({})).stdin_json().is_none());
    let error = plan
        .materialize(
            "tool",
            &json!({"private":"private-fixture"}),
            &[],
            InputDelivery::None,
            ArgumentLimits::default(),
        )
        .unwrap_err();
    assert_eq!(code(error), ErrorCode::NoSecureDelivery);
}

#[test]
fn concrete_values_must_satisfy_the_captured_schema_without_coercion() {
    let plan = compile(json!({"value":{"type":"integer","minimum":0,"maximum":3,"cli":{"kind":"positional","order":0}}}), &[]).unwrap();
    for value in [json!("2"), json!(4), json!(-1)] {
        assert!(matches!(
            plan.materialize(
                "tool",
                &json!({"value":value}),
                &[],
                InputDelivery::None,
                ArgumentLimits::default()
            ),
            Err(Error::Schema(_))
        ));
    }
    assert_eq!(materialize(&plan, json!({"value":0})).argv(), &["0"]);
}

#[test]
fn argument_count_and_byte_limits_are_enforced_during_expansion() {
    let plan = compile(json!({"items":{"type":"array","items":{"type":"string"},"cli":{"kind":"positional","order":0,"expand":true}}}), &[]).unwrap();
    let limits = ArgumentLimits {
        max_arguments: 2,
        max_argument_bytes: 4,
        max_total_bytes: 10,
    };
    assert_eq!(
        plan.materialize(
            "p",
            &json!({"items":["a","b"]}),
            &[],
            InputDelivery::None,
            limits
        )
        .unwrap()
        .argv(),
        &["a", "b"]
    );
    for input in [
        json!({"items":["a","b","c"]}),
        json!({"items":["abcde"]}),
        json!({"items":["abcd","abcd"]}),
    ] {
        let error = plan
            .materialize("p", &input, &[], InputDelivery::None, limits)
            .unwrap_err();
        assert_eq!(code(error), ErrorCode::LimitExceeded);
    }
    let error = plan
        .materialize(
            "p",
            &json!({"items":[]}),
            &[],
            InputDelivery::None,
            ArgumentLimits {
                max_arguments: 1025,
                ..ArgumentLimits::default()
            },
        )
        .unwrap_err();
    assert_eq!(code(error), ErrorCode::InvalidBinding);
}

#[test]
fn json_encoding_limits_and_nul_rejection_do_not_echo_values() {
    for encoding in ["scalar", "json"] {
        let plan = compile(json!({"value":{"type":"string","cli":{"kind":"positional","encoding":encoding,"order":0}}}), &[]).unwrap();
        let error = plan
            .materialize(
                "p",
                &json!({"value":"private-fixture"}),
                &[],
                InputDelivery::None,
                ArgumentLimits {
                    max_argument_bytes: 4,
                    ..ArgumentLimits::default()
                },
            )
            .unwrap_err();
        assert!(!format!("{error:?}").contains("private-fixture"));
        assert_eq!(code(error), ErrorCode::LimitExceeded);
    }
    let plan = compile(
        json!({"value":{"type":"string","cli":{"kind":"positional","order":0}}}),
        &[],
    )
    .unwrap();
    let error = plan
        .materialize(
            "p",
            &json!({"value":"private\0fixture"}),
            &[],
            InputDelivery::None,
            ArgumentLimits::default(),
        )
        .unwrap_err();
    assert_eq!(code(error), ErrorCode::InvalidValue);
}

#[test]
fn data_named_cli_is_preserved_and_json_strings_are_explicitly_quoted() {
    let plan = compile(
        json!({
            "payload":{"type":"object","default":{"cli":"literal data"},
                "cli":{"kind":"positional","encoding":"json","order":0}},
            "text":{"type":"string","cli":{"kind":"positional","encoding":"json","order":1}}
        }),
        &[],
    )
    .unwrap();
    assert_eq!(
        materialize(&plan, json!({"text":"a b"})).argv(),
        &[r#"{"cli":"literal data"}"#, r#""a b""#]
    );
}

#[test]
fn materialization_does_not_reapply_admission_defaults() {
    let plan = compile(
        json!({"value":{"type":"string","default":"admitted default",
        "cli":{"kind":"option","name":"--value","order":0}}}),
        &[],
    )
    .unwrap();
    let raw = plan
        .materialize(
            "tool",
            &json!({}),
            &[],
            InputDelivery::None,
            ArgumentLimits::default(),
        )
        .unwrap();
    assert!(raw.argv().is_empty());
    assert_eq!(
        materialize(&plan, json!({})).argv(),
        &["--value", "admitted default"]
    );
}

#[test]
fn source_depth_definition_count_and_expansion_have_bounded_work() {
    let mut schema = json!({"type":"string","cli":{"kind":"positional","order":0}});
    for _ in 0..80 {
        schema = json!({"type":"object","properties":{"nested":schema}});
    }
    assert!(
        matches!(compile(json!({"root":schema}), &[]), Err(Error::Schema(error))
        if error.code == value_schema::ErrorCode::ResourceLimit)
    );

    let literals: Vec<_> = (0..1025)
        .map(|order| LiteralArgument {
            literal: String::new(),
            order,
        })
        .collect();
    assert_eq!(
        code(compile(json!({}), &literals).err().unwrap()),
        ErrorCode::LimitExceeded
    );

    let plan = compile(
        json!({"items":{"type":"array","items":{"type":"string"},
        "cli":{"kind":"positional","order":0,"expand":true}}}),
        &[],
    )
    .unwrap();
    let error = plan
        .materialize(
            "p",
            &json!({"items":vec!["";1025]}),
            &[],
            InputDelivery::None,
            ArgumentLimits::default(),
        )
        .unwrap_err();
    assert_eq!(code(error), ErrorCode::LimitExceeded);
}
