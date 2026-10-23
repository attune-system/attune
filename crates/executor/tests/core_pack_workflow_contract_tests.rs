//! Pure tests of shipped pack contracts through the real compiler and renderer.
//! No database, process, runtime action or network fixture is used.

use attune_common::{
    secret_values::SecretSource,
    value_schema::{compile_inline, CompiledContract, ErrorCode, SchemaContext},
    workflow::parser::parse_workflow_file,
};
use attune_executor::workflow::context::{TaskOutcome, WorkflowContext};
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf};

fn core() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packs/core")
}

fn metadata(name: &str) -> Value {
    serde_yaml_ng::from_str(
        &std::fs::read_to_string(core().join("actions").join(format!("{name}.yaml"))).unwrap(),
    )
    .unwrap()
}

fn graph() -> Value {
    let path = core().join("actions/workflows/install_packs.workflow.yaml");
    let parsed = parse_workflow_file(&path).expect("shipped canonical graph must parse");
    assert_eq!(parsed.tasks.len(), 12);
    serde_yaml_ng::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn task<'a>(graph: &'a Value, name: &str) -> &'a Value {
    graph["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["name"] == name)
        .unwrap()
}

fn parameters(input: &Value) -> Value {
    let action = metadata("install_packs");
    let contract = compile_inline(
        "core.install_packs",
        SchemaContext::FlatDocument,
        &action["parameters"],
    )
    .unwrap();
    CompiledContract::new(&contract)
        .unwrap()
        .apply_defaults(input)
        .unwrap()
}

fn context(input: Value, graph: &Value) -> WorkflowContext {
    let vars: HashMap<String, Value> = serde_json::from_value(graph["vars"].clone()).unwrap();
    let mut context = WorkflowContext::new(parameters(&input), vars);
    context.set_var("pack_paths", json!(["/owned/metadata-only/example"]));
    context.set_last_task_outcome(
        json!({"summary": {"failure_count": 0}}),
        TaskOutcome::Succeeded,
    );
    context.set_template_origin("workflow", "core.install_packs");
    context
}

#[test]
fn shipped_core_action_schemas_compile_with_real_flat_compiler() {
    let mut checked = 0;
    for entry in std::fs::read_dir(core().join("actions")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("yaml") {
            continue;
        }
        let action: Value =
            serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        checked += 1;
        for field in ["parameters", "output"] {
            if let Some(schema) = action.get(field) {
                compile_inline(
                    action["ref"].as_str().unwrap(),
                    SchemaContext::FlatDocument,
                    schema,
                )
                .unwrap_or_else(|failure| panic!("{} /{field}: {failure}", path.display()));
            }
        }
    }
    assert_eq!(checked, 11);
}

#[test]
fn empty_or_null_secret_defaults_are_not_a_workflow_compatibility_escape() {
    let action = metadata("install_packs");
    assert!(action["parameters"]["api_token"].get("default").is_none());
    let mut schema = action["parameters"].clone();
    schema["api_token"]["default"] = json!("");
    assert_eq!(
        compile_inline("core.install_packs", SchemaContext::FlatDocument, &schema)
            .unwrap_err()
            .code,
        ErrorCode::SecretLiteral
    );
    schema["api_token"]["default"] = Value::Null;
    assert!(compile_inline("core.install_packs", SchemaContext::FlatDocument, &schema).is_err());
}

#[test]
fn omitted_optional_token_renders_no_secret_field_or_endpoint_override() {
    let graph = graph();
    let input = json!({"packs": ["https://example.invalid/source.tar.gz"]});
    assert!(parameters(&input).get("api_token").is_none());
    let context = context(input, &graph);
    for name in [
        "download_packs",
        "get_dependencies",
        "build_environments",
        "register_packs",
    ] {
        let task = task(&graph, name);
        let rendered = context
            .render_json_with_sensitivity(&task["input"])
            .unwrap();
        assert!(rendered.value.get("api_token").is_none(), "{name}");
        assert!(rendered.value.get("api_url").is_none(), "{name}");
        assert!(rendered.secret_paths.is_empty(), "{name}");
        assert!(rendered.secret_path_sources.is_empty(), "{name}");
        let action_name = task["action"]
            .as_str()
            .unwrap()
            .strip_prefix("core.")
            .unwrap();
        let action = metadata(action_name);
        let contract = compile_inline(
            task["action"].as_str().unwrap(),
            SchemaContext::FlatDocument,
            &action["parameters"],
        )
        .unwrap();
        let compiled = CompiledContract::new(&contract).unwrap();
        let child = compiled.apply_defaults(&rendered.value).unwrap();
        compiled.validate(&child).unwrap();
        assert!(child.get("api_token").is_none(), "{name}");
        assert!(child.get("api_url").is_none(), "{name}");
    }
}

#[test]
fn provided_secret_retains_parameter_origin_for_recorded_child_inputs() {
    let graph = graph();
    let context = context(
        json!({"packs": ["https://example.invalid/source.tar.gz"], "api_token": "fixture-not-a-live-credential"}),
        &graph,
    );
    let origin = SecretSource::WorkflowParameter {
        execution_id: 42,
        path: "/api_token".into(),
    };
    context.mark_secret_pointer_paths("parameters", &["/api_token".into()], |_| origin.clone());
    for name in [
        "download_packs_with_token",
        "get_dependencies_with_token",
        "build_environments_with_token",
        "register_packs_with_token",
    ] {
        let rendered = context
            .render_json_with_sensitivity(&task(&graph, name)["input"])
            .unwrap();
        assert_eq!(rendered.value["api_token"], "fixture-not-a-live-credential");
        assert!(rendered.secret_paths.contains(&"/api_token".into()));
        let sources: Vec<_> = rendered
            .secret_path_sources
            .iter()
            .filter(|source| source.path == "/api_token")
            .collect();
        assert!(!sources.is_empty(), "{name}");
        assert!(
            sources.iter().any(|source| {
                let provenance = source.source.provenance();
                provenance.origins == origin.provenance().origins
                    && !provenance.templates.is_empty()
            }),
            "{name}"
        );
    }
}

#[test]
fn native_guards_select_only_the_present_or_absent_token_path() {
    let graph = graph();
    for token_present in [false, true] {
        for skip_dependencies in [false, true] {
            for skip_environment in [false, true] {
                let mut input = json!({
                    "packs": ["https://example.invalid/source.tar.gz"],
                    "skip_dependencies": skip_dependencies,
                    "skip_env_build": skip_environment,
                });
                if token_present {
                    input["api_token"] = json!("fixture-not-a-live-credential");
                }
                let context = context(input, &graph);
                let suffix = if token_present { "_with_token" } else { "" };
                for (name, expected) in [
                    ("initialize", format!("download_packs{suffix}")),
                    (
                        "check_dependencies",
                        if skip_dependencies {
                            "check_environment".into()
                        } else {
                            format!("get_dependencies{suffix}")
                        },
                    ),
                    (
                        "check_environment",
                        if skip_environment {
                            format!("register_packs{suffix}")
                        } else {
                            format!("build_environments{suffix}")
                        },
                    ),
                    ("build_environments", format!("register_packs{suffix}")),
                ] {
                    let selected: Vec<_> = task(&graph, name)["next"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|transition| {
                            context
                                .evaluate_condition(transition["when"].as_str().unwrap())
                                .unwrap()
                        })
                        .flat_map(|transition| {
                            transition["do"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|target| target.as_str().unwrap().to_owned())
                        })
                        .collect();
                    assert_eq!(selected, vec![expected], "{name}");
                }
            }
        }
    }
}

#[test]
fn forced_environment_failures_preserve_the_same_optional_credential_choice() {
    let graph = graph();
    for token_present in [false, true] {
        for force in [false, true] {
            for outcome in [TaskOutcome::Failed, TaskOutcome::TimedOut] {
                let mut input =
                    json!({"packs": ["https://example.invalid/source.tar.gz"], "force": force});
                if token_present {
                    input["api_token"] = json!("fixture-not-a-live-credential");
                }
                let mut context = context(input, &graph);
                context.set_last_task_outcome(json!({}), outcome);
                let selected: Vec<_> = task(&graph, "build_environments")["next"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|transition| {
                        context
                            .evaluate_condition(transition["when"].as_str().unwrap())
                            .unwrap()
                    })
                    .flat_map(|transition| {
                        transition["do"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|target| target.as_str().unwrap().to_owned())
                    })
                    .collect();
                let expected = if !force {
                    "fail_installation"
                } else if token_present {
                    "register_packs_with_token"
                } else {
                    "register_packs"
                };
                assert_eq!(selected, vec![expected]);
            }
        }
    }
}
