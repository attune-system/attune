//! # Workflow Expression Validator
//!
//! Static validation of `{{ }}` template expressions in workflow definitions.
//! Catches syntax errors and unresolved variable references **before** the
//! workflow is saved, so users get immediate feedback instead of opaque
//! runtime failures during execution.
//!
//! ## What is validated
//!
//! 1. **Syntax** — every `{{ expr }}` block must parse successfully.
//! 2. **Variable references** — top-level identifiers that are not a known
//!    namespace (`parameters`, `workflow`, `task`, `config`, `keystore`,
//!    `item`, `index`, `system`, …) must exist in either the workflow's
//!    `vars` map or its `param_schema` keys (bare-name fallback targets).
//!
//! ## What is NOT validated
//!
//! - **Type correctness** — e.g. whether `range(parameters.n)` actually
//!   receives an integer.  That requires runtime values.
//! - **Deep property paths** — e.g. `task.fetch.result.data`.  We validate
//!   that `task` is a known namespace but not that `fetch` is a real task
//!   name (it might not exist yet at save time if tasks are re-ordered).
//! - **Function arity** — built-in functions are not checked for argument
//!   count here; the evaluator already reports those errors at runtime.

use std::collections::HashSet;

use serde_json::Value as JsonValue;

use super::expression::{parse_expression, Expr};
use super::parser::{PublishDirective, WorkflowDefinition};

// ───────────────────────────────────────────────────────────────────────────
// Public API
// ───────────────────────────────────────────────────────────────────────────

/// A single validation diagnostic.
#[derive(Debug, Clone)]
pub struct ExpressionWarning {
    /// Human-readable location (e.g. `task 'sleep_1' with_items`).
    pub location: String,
    /// The raw template string that was checked.
    pub expression: String,
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for ExpressionWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} — `{}`",
            self.location, self.message, self.expression
        )
    }
}

/// Validate all template expressions in a workflow definition.
///
/// Returns an empty vec on success, or one [`ExpressionWarning`] per problem
/// found.  The caller decides whether warnings are fatal (block save) or
/// advisory.
///
/// `param_schema` is the *flat-format* schema (`{ "url": { "type": "string" }, … }`)
/// passed alongside the definition in the save request.  Its top-level keys
/// are the declared parameter names.
pub fn validate_workflow_expressions(
    workflow: &WorkflowDefinition,
    param_schema: Option<&JsonValue>,
) -> Vec<ExpressionWarning> {
    let known_names = build_known_names(workflow, param_schema);
    let mut warnings = Vec::new();

    for task in &workflow.tasks {
        let task_loc = format!("task '{}'", task.name);

        // ── with_items expression ────────────────────────────────────
        if let Some(ref expr) = task.with_items {
            validate_template(
                expr,
                &format!("{task_loc} with_items"),
                &known_names,
                &mut warnings,
            );
        }

        if let Some(ref iterate_cache) = task.iterate_cache {
            for (field, template) in [
                ("owner_type", iterate_cache.owner_type.as_deref()),
                ("owner_ref", iterate_cache.owner_ref.as_deref()),
                ("namespace", Some(iterate_cache.namespace.as_str())),
                ("generation", Some(iterate_cache.generation.as_str())),
            ] {
                if let Some(template) = template {
                    validate_template(
                        template,
                        &format!("{task_loc} iterate_cache.{field}"),
                        &known_names,
                        &mut warnings,
                    );
                }
            }
        }

        // ── task-level when condition ────────────────────────────────
        if let Some(ref expr) = task.when {
            validate_template(
                expr,
                &format!("{task_loc} when"),
                &known_names,
                &mut warnings,
            );
        }

        // ── input templates ──────────────────────────────────────────
        for (key, value) in &task.input {
            collect_json_templates(
                value,
                &format!("{task_loc} input.{key}"),
                &known_names,
                &mut warnings,
            );
        }

        // ── next transitions ─────────────────────────────────────────
        for (ti, transition) in task.next.iter().enumerate() {
            if let Some(ref when_expr) = transition.when {
                validate_template(
                    when_expr,
                    &format!("{task_loc} next[{ti}].when"),
                    &known_names,
                    &mut warnings,
                );
            }

            for directive in &transition.publish {
                match directive {
                    PublishDirective::Simple(map) => {
                        for (pk, pv) in map {
                            // Only validate string values as templates;
                            // non-string literals (booleans, numbers, etc.)
                            // pass through unchanged and have no expressions.
                            if let Some(s) = pv.as_str() {
                                validate_template(
                                    s,
                                    &format!("{task_loc} next[{ti}].publish.{pk}"),
                                    &known_names,
                                    &mut warnings,
                                );
                            }
                        }
                    }
                    PublishDirective::Key(_) => { /* nothing to validate */ }
                }
            }
        }

        // ── legacy task-level publish ────────────────────────────────
        for directive in &task.publish {
            if let PublishDirective::Simple(map) = directive {
                for (pk, pv) in map {
                    // Only validate string values as templates;
                    // non-string literals pass through unchanged.
                    if let Some(s) = pv.as_str() {
                        validate_template(
                            s,
                            &format!("{task_loc} publish.{pk}"),
                            &known_names,
                            &mut warnings,
                        );
                    }
                }
            }
        }
    }

    warnings
}

// ───────────────────────────────────────────────────────────────────────────
// Internals
// ───────────────────────────────────────────────────────────────────────────

/// Canonical namespace identifiers that are always valid as top-level names
/// inside `{{ }}` expressions.  These are resolved by `WorkflowContext` at
/// runtime and never need to exist in `vars` or `param_schema`.
const CANONICAL_NAMESPACES: &[&str] = &[
    "parameters",
    "workflow",
    "vars",
    "variables",
    "task",
    "tasks",
    "config",
    "keystore",
    "item",
    "index",
    "system",
];

/// Built-in constants that are valid bare identifiers.
const BUILTIN_LITERALS: &[&str] = &["true", "false", "null"];

/// Build the set of bare names that are valid in expressions:
/// canonical namespaces + workflow var names + param_schema keys.
fn build_known_names(
    workflow: &WorkflowDefinition,
    param_schema: Option<&JsonValue>,
) -> HashSet<String> {
    let mut names: HashSet<String> = CANONICAL_NAMESPACES
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    for lit in BUILTIN_LITERALS {
        names.insert((*lit).to_string());
    }

    // Workflow vars
    for key in workflow.vars.keys() {
        names.insert(key.clone());
    }

    // Parameter schema keys (flat format: top-level keys are param names)
    if let Some(JsonValue::Object(map)) = param_schema {
        for key in map.keys() {
            names.insert(key.clone());
        }
    }

    // Also accept the workflow-level `parameters` schema if present on the
    // definition itself (some loaders put it there).
    if let Some(JsonValue::Object(ref map)) = workflow.parameters {
        for key in map.keys() {
            names.insert(key.clone());
        }
    }

    names
}

/// Extract `{{ … }}` blocks from a template string and validate each one.
fn validate_template(
    template: &str,
    location: &str,
    known_names: &HashSet<String>,
    warnings: &mut Vec<ExpressionWarning>,
) {
    for raw_expr in extract_expressions(template) {
        let trimmed = raw_expr.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Phase 1: parse
        match parse_expression(trimmed) {
            Err(e) => {
                warnings.push(ExpressionWarning {
                    location: location.to_string(),
                    expression: raw_expr.to_string(),
                    message: format!("syntax error: {e}"),
                });
            }
            Ok(ast) => {
                // Phase 2: check bare-name references
                let mut bare_idents = Vec::new();
                collect_bare_idents(&ast, &mut bare_idents);

                for ident in bare_idents {
                    if !known_names.contains(&ident) {
                        warnings.push(ExpressionWarning {
                            location: location.to_string(),
                            expression: raw_expr.to_string(),
                            message: format!(
                                "unknown variable '{}'. Use 'parameters.{}' for input \
                                 parameters, or define it in workflow vars",
                                ident, ident,
                            ),
                        });
                    }
                }
            }
        }
    }
}

/// Recursively walk a JSON value looking for string leaves that contain
/// `{{ }}` templates.
fn collect_json_templates(
    value: &JsonValue,
    location: &str,
    known_names: &HashSet<String>,
    warnings: &mut Vec<ExpressionWarning>,
) {
    match value {
        JsonValue::String(s) => {
            validate_template(s, location, known_names, warnings);
        }
        JsonValue::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                collect_json_templates(item, &format!("{location}[{i}]"), known_names, warnings);
            }
        }
        JsonValue::Object(map) => {
            for (key, val) in map {
                collect_json_templates(val, &format!("{location}.{key}"), known_names, warnings);
            }
        }
        _ => { /* numbers, bools, null — nothing to validate */ }
    }
}

/// Extract the inner expression strings from all `{{ … }}` blocks in a
/// template.  Handles nested braces conservatively (takes everything between
/// the outermost `{{` and `}}`).
fn extract_expressions(template: &str) -> Vec<&str> {
    let mut results = Vec::new();
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        let after_open = start + 2;
        if let Some(end) = rest[after_open..].find("}}") {
            results.push(&rest[after_open..after_open + end]);
            rest = &rest[after_open + end + 2..];
        } else {
            // Unclosed `{{` — skip
            break;
        }
    }

    results
}

/// Collect bare `Ident` nodes that appear at the *top level* of an
/// expression — i.e. identifiers that are not the right-hand side of a
/// `.field` access (those are field names, not variable references).
///
/// For `DotAccess { object: Ident("parameters"), field: "n" }` we collect
/// `"parameters"` but NOT `"n"`.
///
/// For `FunctionCall { name: "range", args: [Ident("n")] }` we collect
/// `"n"` (it's a bare variable reference used as a function argument).
fn collect_bare_idents(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Ident(name) => {
            out.push(name.clone());
        }
        Expr::Literal(_) => {}
        Expr::Array(items) => {
            for item in items {
                collect_bare_idents(item, out);
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_bare_idents(left, out);
            collect_bare_idents(right, out);
        }
        Expr::UnaryOp { operand, .. } => {
            collect_bare_idents(operand, out);
        }
        Expr::DotAccess { object, .. } => {
            // Only recurse into the object side — the field name is not a
            // variable reference.
            collect_bare_idents(object, out);
        }
        Expr::IndexAccess { object, index } => {
            collect_bare_idents(object, out);
            collect_bare_idents(index, out);
        }
        Expr::FunctionCall { args, .. } => {
            // Function name itself is not a variable reference.
            for arg in args {
                collect_bare_idents(arg, out);
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn minimal_workflow(tasks: Vec<super::super::parser::Task>) -> WorkflowDefinition {
        WorkflowDefinition {
            r#ref: "test.wf".to_string(),
            label: "Test".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            parameters: None,
            output: None,
            vars: HashMap::new(),
            tasks,
            output_map: None,
            tags: vec![],
            cancellation_policy: super::super::parser::CancellationPolicy::default(),
        }
    }

    fn action_task(name: &str) -> super::super::parser::Task {
        super::super::parser::Task {
            name: name.to_string(),
            r#type: super::super::parser::TaskType::Action,
            action: Some("core.echo".to_string()),
            input: HashMap::new(),
            permission_set_refs: None,
            trace_tag_template: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            when: None,
            with_items: None,
            iterate_cache: None,
            batch_size: None,
            concurrency: None,
            retry: None,
            timeout: None,
            next: vec![],
            on_success: None,
            on_failure: None,
            on_complete: None,
            on_timeout: None,
            decision: vec![],
            publish: vec![],
            join: None,
            tasks: None,
            chart_meta: None,
        }
    }

    // ── extract_expressions ──────────────────────────────────────────

    #[test]
    fn test_extract_single() {
        let exprs = extract_expressions("{{ parameters.n }}");
        assert_eq!(exprs, vec![" parameters.n "]);
    }

    #[test]
    fn test_extract_multiple() {
        let exprs = extract_expressions("Hello {{ name }}, you have {{ count }} items");
        assert_eq!(exprs.len(), 2);
        assert_eq!(exprs[0].trim(), "name");
        assert_eq!(exprs[1].trim(), "count");
    }

    #[test]
    fn test_extract_no_expressions() {
        let exprs = extract_expressions("plain text");
        assert!(exprs.is_empty());
    }

    #[test]
    fn test_extract_unclosed() {
        let exprs = extract_expressions("{{ oops");
        assert!(exprs.is_empty());
    }

    // ── collect_bare_idents ──────────────────────────────────────────

    #[test]
    fn test_bare_ident() {
        let ast = parse_expression("n").unwrap();
        let mut idents = Vec::new();
        collect_bare_idents(&ast, &mut idents);
        assert_eq!(idents, vec!["n"]);
    }

    #[test]
    fn test_dot_access_does_not_collect_field() {
        let ast = parse_expression("parameters.n").unwrap();
        let mut idents = Vec::new();
        collect_bare_idents(&ast, &mut idents);
        assert_eq!(idents, vec!["parameters"]);
    }

    #[test]
    fn test_function_arg_collected() {
        let ast = parse_expression("range(n)").unwrap();
        let mut idents = Vec::new();
        collect_bare_idents(&ast, &mut idents);
        assert_eq!(idents, vec!["n"]);
    }

    #[test]
    fn test_nested_dot_access() {
        let ast = parse_expression("task.fetch.result.data").unwrap();
        let mut idents = Vec::new();
        collect_bare_idents(&ast, &mut idents);
        assert_eq!(idents, vec!["task"]);
    }

    #[test]
    fn test_binary_op() {
        let ast = parse_expression("parameters.x + workflow.y").unwrap();
        let mut idents = Vec::new();
        collect_bare_idents(&ast, &mut idents);
        assert_eq!(idents, vec!["parameters", "workflow"]);
    }

    // ── validate_workflow_expressions ─────────────────────────────────

    #[test]
    fn test_valid_workflow_no_warnings() {
        let mut task = action_task("greet");
        task.with_items = Some("{{ range(parameters.n) }}".to_string());
        task.input
            .insert("message".to_string(), serde_json::json!("Hello {{ item }}"));

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn test_iterate_cache_templates_are_validated() {
        let mut task = action_task("process");
        task.iterate_cache = Some(super::super::parser::IterateCacheConfig {
            owner_type: Some("pack".to_string()),
            owner_ref: Some("{{ missing_owner }}".to_string()),
            namespace: "{{ parameters.namespace }}".to_string(),
            generation: "{{ parameters.generation }}".to_string(),
            page_size: 100,
            require_fresh: false,
        });

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].location.contains("iterate_cache.owner_ref"));
        assert!(warnings[0]
            .message
            .contains("unknown variable 'missing_owner'"));
    }

    #[test]
    fn test_bare_name_from_vars_ok() {
        let mut task = action_task("greet");
        task.with_items = Some("{{ range(n) }}".to_string());

        let mut wf = minimal_workflow(vec![task]);
        wf.vars.insert("n".to_string(), serde_json::json!(5));

        let warnings = validate_workflow_expressions(&wf, None);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn test_bare_name_from_param_schema_ok() {
        let mut task = action_task("greet");
        task.with_items = Some("{{ range(n) }}".to_string());

        let wf = minimal_workflow(vec![task]);
        let schema = serde_json::json!({
            "n": { "type": "integer", "required": true }
        });

        let warnings = validate_workflow_expressions(&wf, Some(&schema));
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn test_unknown_bare_name_warning() {
        let mut task = action_task("greet");
        task.with_items = Some("{{ range(n) }}".to_string());

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown variable 'n'"));
        assert!(warnings[0].message.contains("parameters.n"));
        assert!(warnings[0].location.contains("with_items"));
    }

    #[test]
    fn test_syntax_error_warning() {
        let mut task = action_task("greet");
        task.input
            .insert("msg".to_string(), serde_json::json!("{{ +++ }}"));

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("syntax error"));
    }

    #[test]
    fn test_transition_when_validated() {
        let mut task = action_task("step1");
        task.next = vec![super::super::parser::TaskTransition {
            when: Some("{{ bad_var > 3 }}".to_string()),
            publish: vec![],
            r#do: Some(vec!["step2".to_string()]),
            chart_meta: None,
        }];

        let wf = minimal_workflow(vec![task, action_task("step2")]);
        let warnings = validate_workflow_expressions(&wf, None);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown variable 'bad_var'"));
        assert!(warnings[0].location.contains("next[0].when"));
    }

    #[test]
    fn test_transition_publish_validated() {
        let mut task = action_task("step1");
        let mut publish_map = HashMap::new();
        publish_map.insert(
            "out".to_string(),
            serde_json::Value::String("{{ unknown_thing }}".to_string()),
        );
        task.next = vec![super::super::parser::TaskTransition {
            when: Some("{{ succeeded() }}".to_string()),
            publish: vec![PublishDirective::Simple(publish_map)],
            r#do: Some(vec!["step2".to_string()]),
            chart_meta: None,
        }];

        let wf = minimal_workflow(vec![task, action_task("step2")]);
        let warnings = validate_workflow_expressions(&wf, None);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .message
            .contains("unknown variable 'unknown_thing'"));
        assert!(warnings[0].location.contains("publish.out"));
    }

    #[test]
    fn test_workflow_functions_no_warning() {
        // succeeded(), failed(), result() etc. are function calls,
        // not variable references — should not produce warnings.
        let mut task = action_task("step1");
        task.next = vec![super::super::parser::TaskTransition {
            when: Some("{{ succeeded() and result().code == 200 }}".to_string()),
            publish: vec![],
            r#do: Some(vec!["step2".to_string()]),
            chart_meta: None,
        }];

        let wf = minimal_workflow(vec![task, action_task("step2")]);
        let warnings = validate_workflow_expressions(&wf, None);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn test_plain_text_no_warning() {
        let mut task = action_task("step1");
        task.input
            .insert("msg".to_string(), serde_json::json!("just plain text"));

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_multiple_errors_collected() {
        let mut task = action_task("step1");
        task.with_items = Some("{{ range(a) }}".to_string());
        task.input
            .insert("x".to_string(), serde_json::json!("{{ b + c }}"));

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);

        // a, b, c are all unknown
        assert_eq!(warnings.len(), 3);
        let names: HashSet<_> = warnings
            .iter()
            .flat_map(|w| {
                // extract the variable name from "unknown variable 'X'"
                w.message
                    .strip_prefix("unknown variable '")
                    .and_then(|s| s.split('\'').next())
                    .map(|s| s.to_string())
            })
            .collect();
        assert!(names.contains("a"));
        assert!(names.contains("b"));
        assert!(names.contains("c"));
    }

    #[test]
    fn test_index_access_validated() {
        let mut task = action_task("step1");
        task.input
            .insert("val".to_string(), serde_json::json!("{{ items[idx] }}"));

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);

        // Both `items` and `idx` are bare unknowns
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn test_builtin_literals_ok() {
        let mut task = action_task("step1");
        task.next = vec![super::super::parser::TaskTransition {
            when: Some("{{ true and not false }}".to_string()),
            publish: vec![],
            r#do: None,
            chart_meta: None,
        }];

        let wf = minimal_workflow(vec![task]);
        let warnings = validate_workflow_expressions(&wf, None);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }
}
