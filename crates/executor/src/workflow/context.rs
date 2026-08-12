//! Workflow Context Manager
//!
//! This module manages workflow execution context, including variables,
//! template rendering, and data flow between tasks.
//!
//! ## Canonical Namespaces
//!
//! All data accessible inside `{{ }}` template expressions is organised into
//! well-defined, non-overlapping namespaces:
//!
//! | Namespace | Example | Description |
//! |-----------|---------|-------------|
//! | `parameters` | `{{ parameters.url }}` | Immutable workflow input parameters |
//! | `workflow` | `{{ workflow.counter }}` | Mutable workflow-scoped variables (set via `publish`) |
//! | `task` | `{{ task.fetch.data }}` | Completed task results keyed by task name |
//! | `config` | `{{ config.api_token }}` | Pack configuration values (read-only) |
//! | `keystore` | `{{ keystore.secret_key }}` | Encrypted secrets from the key store (read-only) |
//! | `item` | `{{ item }}` or `{{ item.name }}` | Current element in a `with_items` loop |
//! | `index` | `{{ index }}` | Zero-based iteration index in a `with_items` loop |
//! | `system` | `{{ system.workflow_start }}` | System-provided variables |
//!
//! ### Backward-compatible aliases
//!
//! The following aliases resolve to the same data as their canonical form and
//! are kept for backward compatibility with existing workflow definitions:
//!
//! - `vars` / `variables` → same as `workflow`
//! - `tasks` → same as `task`
//!
//! Bare variable names (e.g. `{{ my_var }}`) also resolve against the
//! `workflow` variable store as a last-resort fallback.
//!
//! ## Function-call expressions
//!
//! Templates support Orquesta-style function calls:
//! - `{{ result() }}` — the last completed task's result
//! - `{{ result().field }}` — nested access into the result
//! - `{{ succeeded() }}` — `true` if the last task succeeded
//! - `{{ failed() }}` — `true` if the last task failed
//! - `{{ timed_out() }}` — `true` if the last task timed out
//!
//! ## Type-preserving rendering
//!
//! When a JSON string value is a *pure* template expression (the entire value
//! is `{{ expr }}`), `render_json` returns the raw `JsonValue` from the
//! expression instead of stringifying it. This means `"{{ item }}"` resolving
//! to integer `5` stays as `5`, not the string `"5"`.

use attune_common::secret_values::{
    pointer_from_dot_path, pointer_join, pointer_suffix, JsonPointer, RenderedJson,
    SecretPathSource, SecretSource,
};
use attune_common::workflow::expression::{
    self, is_truthy, EvalContext, EvalError, EvalResult as ExprResult,
};
use dashmap::DashMap;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

/// Result type for context operations
pub type ContextResult<T> = Result<T, ContextError>;

/// Errors that can occur during context operations
#[derive(Debug, Error)]
pub enum ContextError {
    #[error("Variable not found: {0}")]
    VariableNotFound(String),

    #[error("Invalid expression: {0}")]
    InvalidExpression(String),

    #[error("Type conversion error: {0}")]
    TypeConversion(String),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// The status of the last completed task, used by `succeeded()` / `failed()` /
/// `timed_out()` function expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    Succeeded,
    Failed,
    TimedOut,
}

/// Workflow execution context
///
/// Uses Arc for shared immutable data to enable efficient cloning.
/// When cloning for with-items iterations, only Arc pointers are copied,
/// not the underlying data, making it O(1) instead of O(context_size).
#[derive(Debug, Clone)]
pub struct WorkflowContext {
    /// Mutable workflow-scoped variables. Canonical namespace: `workflow`.
    /// Also accessible as `vars`, `variables`, or bare names (fallback).
    variables: Arc<DashMap<String, JsonValue>>,

    /// Immutable workflow input parameters. Canonical namespace: `parameters`.
    parameters: Arc<JsonValue>,

    /// Completed task results keyed by task name. Canonical namespace: `task`.
    task_results: Arc<DashMap<String, JsonValue>>,

    /// System-provided variables. Canonical namespace: `system`.
    system: Arc<DashMap<String, JsonValue>>,

    /// Pack configuration values (read-only). Canonical namespace: `config`.
    pack_config: Arc<JsonValue>,

    /// Encrypted keystore values (read-only). Canonical namespace: `keystore`.
    keystore: Arc<JsonValue>,

    /// Current item (for with-items iteration) - per-item data
    current_item: Option<JsonValue>,

    /// Current item index (for with-items iteration) - per-item data
    current_index: Option<usize>,

    /// The result of the last completed task (for `result()` expressions)
    last_task_result: Option<JsonValue>,

    /// The outcome of the last completed task (for `succeeded()` / `failed()`)
    last_task_outcome: Option<TaskOutcome>,

    /// Secret source paths keyed by canonical expression path.
    secret_sources: Arc<DashMap<String, Vec<SecretSource>>>,

    /// Secret source paths for the per-clone `item` namespace.
    current_item_secret_sources: Vec<(String, SecretSource)>,
}

impl WorkflowContext {
    /// Create a new workflow context.
    ///
    /// `parameters` — the immutable input parameters for this workflow run.
    /// `initial_vars` — initial workflow-scoped variables (from the workflow
    ///   definition's `vars` section).
    pub fn new(parameters: JsonValue, initial_vars: HashMap<String, JsonValue>) -> Self {
        let system = DashMap::new();
        system.insert("workflow_start".to_string(), json!(chrono::Utc::now()));

        let variables = DashMap::new();
        for (k, v) in initial_vars {
            variables.insert(k, v);
        }

        Self {
            variables: Arc::new(variables),
            parameters: Arc::new(parameters),
            task_results: Arc::new(DashMap::new()),
            system: Arc::new(system),
            pack_config: Arc::new(JsonValue::Null),
            keystore: Arc::new(JsonValue::Null),
            current_item: None,
            current_index: None,
            last_task_result: None,
            last_task_outcome: None,
            secret_sources: Arc::new(DashMap::new()),
            current_item_secret_sources: Vec::new(),
        }
    }

    /// Rebuild a workflow context from persisted workflow execution state.
    ///
    /// This is used when advancing a workflow after a child task completes —
    /// the scheduler reconstructs the context from the `workflow_execution`
    /// record's stored `variables` plus the results of all completed child
    /// executions.
    pub fn rebuild(
        parameters: JsonValue,
        stored_variables: &JsonValue,
        task_results: HashMap<String, JsonValue>,
    ) -> Self {
        let variables = DashMap::new();
        if let Some(obj) = stored_variables.as_object() {
            for (k, v) in obj {
                variables.insert(k.clone(), v.clone());
            }
        }

        let results = DashMap::new();
        for (k, v) in task_results {
            results.insert(k, v);
        }

        let system = DashMap::new();
        system.insert("workflow_start".to_string(), json!(chrono::Utc::now()));

        Self {
            variables: Arc::new(variables),
            parameters: Arc::new(parameters),
            task_results: Arc::new(results),
            system: Arc::new(system),
            pack_config: Arc::new(JsonValue::Null),
            keystore: Arc::new(JsonValue::Null),
            current_item: None,
            current_index: None,
            last_task_result: None,
            last_task_outcome: None,
            secret_sources: Arc::new(DashMap::new()),
            current_item_secret_sources: Vec::new(),
        }
    }

    /// Set a workflow-scoped variable (accessible as `workflow.<name>`).
    pub fn set_var(&mut self, name: &str, value: JsonValue) {
        self.variables.insert(name.to_string(), value);
    }

    /// Get a workflow-scoped variable by name.
    #[allow(dead_code)] // Part of complete context API; used in tests
    pub fn get_var(&self, name: &str) -> Option<JsonValue> {
        self.variables.get(name).map(|entry| entry.value().clone())
    }

    /// Store a completed task's result (accessible as `task.<name>.*`).
    #[allow(dead_code)] // Part of complete context API; used in tests
    pub fn set_task_result(&mut self, task_name: &str, result: JsonValue) {
        self.task_results.insert(task_name.to_string(), result);
    }

    /// Get a task result by task name.
    #[allow(dead_code)] // Part of complete context API; used in tests
    pub fn get_task_result(&self, task_name: &str) -> Option<JsonValue> {
        self.task_results
            .get(task_name)
            .map(|entry| entry.value().clone())
    }

    /// Set the pack configuration (accessible as `config.<key>`).
    #[allow(dead_code)] // Part of complete context API; used in tests
    pub fn set_pack_config(&mut self, config: JsonValue) {
        self.pack_config = Arc::new(config);
    }

    pub fn set_pack_config_with_secret_paths(
        &mut self,
        config: JsonValue,
        pack_ref: Option<String>,
        secret_paths: &[JsonPointer],
    ) {
        self.set_pack_config(config);
        for path in secret_paths {
            self.mark_secret_source_path(
                &format!("config{}", pointer_to_expression_suffix(path)),
                SecretSource::PackConfig {
                    pack_ref: pack_ref.clone(),
                    path: path.clone(),
                },
            );
        }
    }

    /// Set the keystore secrets (accessible as `keystore.<key>`).
    #[allow(dead_code)] // Part of complete context API; used in tests
    pub fn set_keystore(&mut self, secrets: JsonValue) {
        self.keystore = Arc::new(secrets);
    }

    /// Set current item for iteration
    pub fn set_current_item(&mut self, item: JsonValue, index: usize) {
        self.current_item = Some(item);
        self.current_index = Some(index);
        self.current_item_secret_sources.clear();
    }

    pub fn set_current_item_with_secret_paths(
        &mut self,
        item: JsonValue,
        index: usize,
        secret_paths: &[JsonPointer],
        source_for_path: impl Fn(&JsonPointer) -> SecretSource,
    ) {
        self.current_item = Some(item);
        self.current_index = Some(index);
        self.current_item_secret_sources = secret_paths
            .iter()
            .map(|path| {
                (
                    format!("item{}", pointer_to_expression_suffix(path)),
                    source_for_path(path),
                )
            })
            .collect();
    }

    /// Clear current item
    #[allow(dead_code)] // Part of complete context API; symmetric with set_current_item
    pub fn clear_current_item(&mut self) {
        self.current_item = None;
        self.current_index = None;
        self.current_item_secret_sources.clear();
    }

    /// Record the outcome of the last completed task so that `result()`,
    /// `succeeded()`, `failed()`, and `timed_out()` expressions resolve
    /// correctly.
    pub fn set_last_task_outcome(&mut self, result: JsonValue, outcome: TaskOutcome) {
        self.last_task_result = Some(result);
        self.last_task_outcome = Some(outcome);
    }

    pub fn set_last_task_outcome_with_secret_paths(
        &mut self,
        result: JsonValue,
        outcome: TaskOutcome,
        secret_paths: &[JsonPointer],
        source_for_path: impl Fn(&JsonPointer) -> SecretSource,
    ) {
        self.set_last_task_outcome(result, outcome);
        for path in secret_paths {
            self.mark_secret_source_path(
                &format!("result{}", pointer_to_expression_suffix(path)),
                source_for_path(path),
            );
        }
    }

    pub fn mark_secret_source_path(&self, expression_path: &str, source: SecretSource) {
        self.secret_sources
            .entry(expression_path.to_string())
            .or_default()
            .push(source);
    }

    pub fn mark_secret_pointer_paths(
        &self,
        expression_root: &str,
        secret_paths: &[JsonPointer],
        source_for_path: impl Fn(&JsonPointer) -> SecretSource,
    ) {
        for path in secret_paths {
            self.mark_secret_source_path(
                &format!("{expression_root}{}", pointer_to_expression_suffix(path)),
                source_for_path(path),
            );
        }
    }

    /// Export workflow variables as a JSON object suitable for persisting
    /// back to the `workflow_execution.variables` column.
    pub fn export_variables(&self) -> JsonValue {
        let map: HashMap<String, JsonValue> = self
            .variables
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        json!(map)
    }

    /// Render a template string, always returning a `String`.
    ///
    /// For type-preserving rendering of JSON values use [`render_json`].
    pub fn render_template(&self, template: &str) -> ContextResult<String> {
        // Simple template rendering (Jinja2-like syntax)
        // Supports: {{ variable }}, {{ task.result }}, {{ parameters.key }}

        let mut result = template.to_string();

        // Find all template expressions
        let mut start = 0;
        while let Some(open_pos) = result[start..].find("{{") {
            let open_pos = start + open_pos;
            if let Some(close_pos) = result[open_pos..].find("}}") {
                let close_pos = open_pos + close_pos;
                let expr = &result[open_pos + 2..close_pos].trim();

                // Evaluate expression
                let value = self.evaluate_expression(expr)?;

                // Replace template with value
                let value_str = value_to_string(&value);
                result.replace_range(open_pos..close_pos + 2, &value_str);

                start = open_pos + value_str.len();
            } else {
                break;
            }
        }

        Ok(result)
    }

    /// Try to evaluate a string as a single pure template expression.
    ///
    /// Returns `Some(JsonValue)` when the **entire** string is exactly
    /// `{{ expr }}` (with optional whitespace), preserving the original
    /// JSON type of the evaluated expression.  Returns `None` if the
    /// string contains literal text around the template or multiple
    /// template expressions — in that case the caller should fall back
    /// to `render_template` which always stringifies.
    fn try_evaluate_pure_expression(&self, s: &str) -> Option<ContextResult<JsonValue>> {
        let trimmed = s.trim();
        if !trimmed.starts_with("{{") || !trimmed.ends_with("}}") {
            return None;
        }

        // Make sure there is only ONE template expression in the string.
        // Count `{{` occurrences — if more than one, it's not a pure expr.
        if trimmed.matches("{{").count() != 1 {
            return None;
        }

        let expr = trimmed[2..trimmed.len() - 2].trim();
        if expr.is_empty() {
            return None;
        }

        Some(self.evaluate_expression(expr))
    }

    /// Render a JSON value, recursively resolving `{{ }}` templates in
    /// strings.
    ///
    /// **Type-preserving**: when a string value is a *pure* template
    /// expression (the entire string is `{{ expr }}`), the raw `JsonValue`
    /// from the expression is returned.  For example, if `item` is `5`
    /// (a JSON number), then `"{{ item }}"` resolves to `5` not `"5"`.
    pub fn render_json(&self, value: &JsonValue) -> ContextResult<JsonValue> {
        match value {
            JsonValue::String(s) => {
                // Fast path: try as a pure expression to preserve type
                if let Some(result) = self.try_evaluate_pure_expression(s) {
                    return result;
                }
                // Fallback: render as string (interpolation with surrounding text)
                let rendered = self.render_template(s)?;
                Ok(JsonValue::String(rendered))
            }
            JsonValue::Array(arr) => {
                let mut result = Vec::new();
                for item in arr {
                    result.push(self.render_json(item)?);
                }
                Ok(JsonValue::Array(result))
            }
            JsonValue::Object(obj) => {
                let mut result = serde_json::Map::new();
                for (key, val) in obj {
                    result.insert(key.clone(), self.render_json(val)?);
                }
                Ok(JsonValue::Object(result))
            }
            other => Ok(other.clone()),
        }
    }

    pub fn render_json_with_sensitivity(&self, value: &JsonValue) -> ContextResult<RenderedJson> {
        self.render_json_with_sensitivity_at(value, "")
    }

    fn render_json_with_sensitivity_at(
        &self,
        value: &JsonValue,
        pointer: &str,
    ) -> ContextResult<RenderedJson> {
        match value {
            JsonValue::String(s) => {
                if let Some(result) = self.try_evaluate_pure_expression(s) {
                    let trimmed = s.trim();
                    let expr = trimmed[2..trimmed.len() - 2].trim();
                    let value = result?;
                    let path_sources = self.secret_path_sources_for_expression(expr, pointer, true);
                    return Ok(rendered_from_path_sources(value, path_sources));
                }

                let rendered = self.render_template(s)?;
                let mut path_sources = Vec::new();
                for expr in template_expressions(s) {
                    path_sources
                        .extend(self.secret_path_sources_for_expression(&expr, pointer, false));
                }
                Ok(rendered_from_path_sources(
                    JsonValue::String(rendered),
                    path_sources,
                ))
            }
            JsonValue::Array(arr) => {
                let mut result = Vec::new();
                let mut path_sources = Vec::new();
                for (idx, item) in arr.iter().enumerate() {
                    let child_pointer = format!("{pointer}/{idx}");
                    let rendered = self.render_json_with_sensitivity_at(item, &child_pointer)?;
                    result.push(rendered.value);
                    path_sources.extend(rendered.secret_path_sources);
                }
                Ok(rendered_from_path_sources(
                    JsonValue::Array(result),
                    path_sources,
                ))
            }
            JsonValue::Object(obj) => {
                let mut result = serde_json::Map::new();
                let mut path_sources = Vec::new();
                for (key, val) in obj {
                    let child_pointer = format!("{}/{}", pointer, escape_pointer_segment(key));
                    let rendered = self.render_json_with_sensitivity_at(val, &child_pointer)?;
                    result.insert(key.clone(), rendered.value);
                    path_sources.extend(rendered.secret_path_sources);
                }
                Ok(rendered_from_path_sources(
                    JsonValue::Object(result),
                    path_sources,
                ))
            }
            other => Ok(RenderedJson::plain(other.clone())),
        }
    }

    fn secret_path_sources_for_expression(
        &self,
        expr: &str,
        dest_pointer: &str,
        pure_expression: bool,
    ) -> Vec<SecretPathSource> {
        let expr = normalize_expression_path(expr);
        let mut sources = Vec::new();

        for entry in self.secret_sources.iter() {
            collect_matching_sources(
                &expr,
                dest_pointer,
                pure_expression,
                entry.key(),
                entry.value(),
                &mut sources,
            );
        }

        for (path, source) in &self.current_item_secret_sources {
            collect_matching_sources(
                &expr,
                dest_pointer,
                pure_expression,
                path,
                std::slice::from_ref(source),
                &mut sources,
            );
        }

        sources
    }

    /// Evaluate a template expression using the expression engine.
    ///
    /// Supports the full expression language including arithmetic, comparison,
    /// boolean logic, member access, and built-in functions. Falls back to
    /// legacy dot-path resolution for simple variable references when the
    /// expression engine cannot parse the input.
    fn evaluate_expression(&self, expr: &str) -> ContextResult<JsonValue> {
        // Use the expression engine for all expressions. It handles:
        // - Dot-path access: parameters.config.port
        // - Bracket access: arr[0], obj["key"]
        // - Arithmetic: 2 + 3, length(items) * 2
        // - Comparison: x > 5, status == "ok"
        // - Boolean logic: x > 0 and x < 10
        // - Function calls: length(arr), result(), succeeded()
        // - Membership: "key" in obj, 5 in arr
        expression::eval_expression(expr, self).map_err(|e| match e {
            EvalError::VariableNotFound(name) => ContextError::VariableNotFound(name),
            EvalError::TypeError(msg) => ContextError::TypeConversion(msg),
            EvalError::ParseError(msg) => ContextError::InvalidExpression(msg),
            other => ContextError::InvalidExpression(format!("{}", other)),
        })
    }

    /// Evaluate a conditional expression (for 'when' clauses).
    ///
    /// Uses the full expression engine so conditions can contain comparisons,
    /// boolean operators, function calls, and arithmetic. For example:
    ///
    /// ```text
    /// succeeded()
    /// result().status == "ok"
    /// length(items) > 3 and "admin" in roles
    /// not failed()
    /// ```
    pub fn evaluate_condition(&self, condition: &str) -> ContextResult<bool> {
        // Try the expression engine first — it handles complex conditions
        // like `result().code == 200 and succeeded()`.
        match expression::eval_expression(condition, self) {
            Ok(val) => Ok(is_truthy(&val)),
            Err(_) => {
                // Fall back to template rendering for backward compat with
                // simple template conditions like `{{ succeeded() }}` (though
                // bare expressions are preferred going forward).
                let rendered = self.render_template(condition)?;
                match rendered.trim().to_lowercase().as_str() {
                    "true" | "1" | "yes" => Ok(true),
                    "false" | "0" | "no" | "" => Ok(false),
                    _ => Ok(!rendered.trim().is_empty()),
                }
            }
        }
    }

    /// Publish variables from a task result.
    ///
    /// Each publish directive is a `(name, value)` pair where the value is
    /// any JSON-compatible type.  String values are treated as template
    /// expressions (e.g. `"{{ result().data.items }}"`) and rendered with
    /// type preservation.  Non-string values (booleans, numbers, arrays,
    /// objects, null) pass through `render_json` unchanged, preserving
    /// their original type.
    pub fn publish_from_result(
        &mut self,
        result: &JsonValue,
        publish_vars: &[String],
        publish_map: Option<&HashMap<String, JsonValue>>,
    ) -> ContextResult<()> {
        // If publish map is provided, use it
        if let Some(map) = publish_map {
            for (var_name, json_value) in map {
                // render_json handles all types: strings are template-rendered
                // (with type preservation for pure `{{ }}` expressions), while
                // booleans, numbers, arrays, objects, and null pass through
                // unchanged.
                let value = self.render_json(json_value)?;
                self.set_var(var_name, value);
            }
        } else {
            // Simple variable publishing - store entire result
            for var_name in publish_vars {
                self.set_var(var_name, result.clone());
            }
        }

        Ok(())
    }

    /// Export context for storage
    #[allow(dead_code)] // Part of complete context API; used in tests
    pub fn export(&self) -> JsonValue {
        let variables: HashMap<String, JsonValue> = self
            .variables
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        let task_results: HashMap<String, JsonValue> = self
            .task_results
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        let system: HashMap<String, JsonValue> = self
            .system
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        json!({
            "variables": variables,
            "parameters": self.parameters.as_ref(),
            "task_results": task_results,
            "system": system,
            "pack_config": self.pack_config.as_ref(),
            "keystore": self.keystore.as_ref(),
        })
    }

    /// Import context from stored data
    #[allow(dead_code)] // Part of complete context API; used in tests
    pub fn import(data: JsonValue) -> ContextResult<Self> {
        let variables = DashMap::new();
        if let Some(obj) = data["variables"].as_object() {
            for (k, v) in obj {
                variables.insert(k.clone(), v.clone());
            }
        }

        let parameters = data["parameters"].clone();

        let task_results = DashMap::new();
        if let Some(obj) = data["task_results"].as_object() {
            for (k, v) in obj {
                task_results.insert(k.clone(), v.clone());
            }
        }

        let system = DashMap::new();
        if let Some(obj) = data["system"].as_object() {
            for (k, v) in obj {
                system.insert(k.clone(), v.clone());
            }
        }

        let pack_config = data["pack_config"].clone();
        let keystore = data["keystore"].clone();

        Ok(Self {
            variables: Arc::new(variables),
            parameters: Arc::new(parameters),
            task_results: Arc::new(task_results),
            system: Arc::new(system),
            pack_config: Arc::new(pack_config),
            keystore: Arc::new(keystore),
            current_item: None,
            current_index: None,
            last_task_result: None,
            last_task_outcome: None,
            secret_sources: Arc::new(DashMap::new()),
            current_item_secret_sources: Vec::new(),
        })
    }
}

/// Convert a JSON value to a string for template rendering
fn value_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn rendered_from_path_sources(
    value: JsonValue,
    path_sources: Vec<SecretPathSource>,
) -> RenderedJson {
    let mut secret_path_sources = path_sources;
    secret_path_sources.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    secret_path_sources.dedup_by(|a, b| a.path == b.path && a.source == b.source);

    let mut secret_paths = secret_path_sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    secret_paths.sort();
    secret_paths.dedup();

    let mut sources = secret_path_sources
        .iter()
        .map(|source| source.source.clone())
        .collect::<Vec<_>>();
    sources.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    sources.dedup();

    RenderedJson {
        value,
        secret_paths,
        sources,
        secret_path_sources,
    }
}

fn collect_matching_sources(
    expr: &str,
    dest_pointer: &str,
    pure_expression: bool,
    source_expr_path: &str,
    source_values: &[SecretSource],
    output: &mut Vec<SecretPathSource>,
) {
    if source_expr_path == expr || expr.starts_with(&format!("{source_expr_path}.")) {
        output.extend(
            source_values
                .iter()
                .cloned()
                .map(|source| SecretPathSource {
                    path: dest_pointer.to_string(),
                    source,
                }),
        );
        return;
    }

    if source_expr_path.starts_with(&format!("{expr}.")) {
        let path = if pure_expression {
            let source_pointer = pointer_from_dot_path(source_expr_path);
            let expr_pointer = pointer_from_dot_path(expr);
            pointer_suffix(&source_pointer, &expr_pointer)
                .map(|suffix| pointer_join(dest_pointer, &suffix))
                .unwrap_or_else(|| dest_pointer.to_string())
        } else {
            dest_pointer.to_string()
        };

        output.extend(
            source_values
                .iter()
                .cloned()
                .map(|source| SecretPathSource {
                    path: path.clone(),
                    source,
                }),
        );
    }
}

fn template_expressions(s: &str) -> Vec<String> {
    let mut expressions = Vec::new();
    let mut start = 0;
    while let Some(open_pos) = s[start..].find("{{") {
        let open_pos = start + open_pos;
        if let Some(close_pos) = s[open_pos..].find("}}") {
            let close_pos = open_pos + close_pos;
            expressions.push(s[open_pos + 2..close_pos].trim().to_string());
            start = close_pos + 2;
        } else {
            break;
        }
    }
    expressions
}

fn normalize_expression_path(expr: &str) -> String {
    expr.trim()
        .replace("result()", "result")
        .replace("[\"", ".")
        .replace("\"]", "")
        .replace("[\'", ".")
        .replace("']", "")
}

fn pointer_to_expression_suffix(pointer: &str) -> String {
    let suffix = pointer
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(unescape_pointer_segment)
        .collect::<Vec<_>>()
        .join(".");
    if suffix.is_empty() {
        String::new()
    } else {
        format!(".{suffix}")
    }
}

fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn unescape_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

// ---------------------------------------------------------------
// EvalContext implementation — bridges the expression engine into
// the WorkflowContext's variable resolution and workflow functions.
// ---------------------------------------------------------------

impl EvalContext for WorkflowContext {
    fn resolve_variable(&self, name: &str) -> ExprResult<JsonValue> {
        match name {
            // ── Canonical namespaces ──────────────────────────────
            "parameters" => Ok(self.parameters.as_ref().clone()),

            // `workflow` is the canonical name for mutable vars.
            // `vars` and `variables` are backward-compatible aliases.
            "workflow" | "vars" | "variables" => {
                let map: serde_json::Map<String, JsonValue> = self
                    .variables
                    .iter()
                    .map(|entry| (entry.key().clone(), entry.value().clone()))
                    .collect();
                Ok(JsonValue::Object(map))
            }

            // `task` (alias: `tasks`) — completed task results.
            "task" | "tasks" => {
                let map: serde_json::Map<String, JsonValue> = self
                    .task_results
                    .iter()
                    .map(|entry| (entry.key().clone(), entry.value().clone()))
                    .collect();
                Ok(JsonValue::Object(map))
            }

            // `config` — pack configuration (read-only).
            "config" => Ok(self.pack_config.as_ref().clone()),

            // `keystore` — encrypted secrets (read-only).
            "keystore" => Ok(self.keystore.as_ref().clone()),

            // ── Iteration context ────────────────────────────────
            "item" => self
                .current_item
                .clone()
                .ok_or_else(|| EvalError::VariableNotFound("item".to_string())),
            "index" => self
                .current_index
                .map(|i| json!(i))
                .ok_or_else(|| EvalError::VariableNotFound("index".to_string())),

            // ── System variables ──────────────────────────────────
            "system" => {
                let map: serde_json::Map<String, JsonValue> = self
                    .system
                    .iter()
                    .map(|entry| (entry.key().clone(), entry.value().clone()))
                    .collect();
                Ok(JsonValue::Object(map))
            }

            // ── Bare-name fallback ───────────────────────────────
            // Resolve against workflow variables last so that
            // `{{ my_var }}` still works as shorthand for
            // `{{ workflow.my_var }}`.
            _ => {
                if let Some(entry) = self.variables.get(name) {
                    Ok(entry.value().clone())
                } else {
                    Err(EvalError::VariableNotFound(name.to_string()))
                }
            }
        }
    }

    fn call_workflow_function(
        &self,
        name: &str,
        _args: &[JsonValue],
    ) -> ExprResult<Option<JsonValue>> {
        match name {
            "succeeded" => {
                let val = self
                    .last_task_outcome
                    .map(|o| o == TaskOutcome::Succeeded)
                    .unwrap_or(false);
                Ok(Some(json!(val)))
            }
            "failed" => {
                let val = self
                    .last_task_outcome
                    .map(|o| o == TaskOutcome::Failed)
                    .unwrap_or(false);
                Ok(Some(json!(val)))
            }
            "timed_out" => {
                let val = self
                    .last_task_outcome
                    .map(|o| o == TaskOutcome::TimedOut)
                    .unwrap_or(false);
                Ok(Some(json!(val)))
            }
            "result" => {
                let base = self.last_task_result.clone().unwrap_or(JsonValue::Null);
                Ok(Some(base))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // parameters namespace
    // ---------------------------------------------------------------

    #[test]
    fn test_basic_template_rendering() {
        let params = json!({
            "name": "World"
        });
        let ctx = WorkflowContext::new(params, HashMap::new());

        let result = ctx.render_template("Hello {{ parameters.name }}!").unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_nested_value_access() {
        let params = json!({
            "config": {
                "server": {
                    "port": 8080
                }
            }
        });
        let ctx = WorkflowContext::new(params, HashMap::new());

        let result = ctx
            .render_template("Port: {{ parameters.config.server.port }}")
            .unwrap();
        assert_eq!(result, "Port: 8080");
    }

    // ---------------------------------------------------------------
    // workflow namespace (canonical) + vars/variables aliases
    // ---------------------------------------------------------------

    #[test]
    fn test_workflow_namespace_canonical() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_var("greeting", json!("Hello"));

        // Canonical: workflow.<name>
        let result = ctx
            .render_template("{{ workflow.greeting }} World")
            .unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_workflow_namespace_vars_alias() {
        let mut vars = HashMap::new();
        vars.insert("greeting".to_string(), json!("Hello"));
        let ctx = WorkflowContext::new(json!({}), vars);

        // Backward-compat alias: vars.<name>
        let result = ctx.render_template("{{ vars.greeting }} World").unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_workflow_namespace_variables_alias() {
        let mut vars = HashMap::new();
        vars.insert("greeting".to_string(), json!("Hello"));
        let ctx = WorkflowContext::new(json!({}), vars);

        // Backward-compat alias: variables.<name>
        let result = ctx
            .render_template("{{ variables.greeting }} World")
            .unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_variable_access_bare_name_fallback() {
        let mut vars = HashMap::new();
        vars.insert("greeting".to_string(), json!("Hello"));

        let ctx = WorkflowContext::new(json!({}), vars);

        // Bare name falls back to workflow variables
        let result = ctx.render_template("{{ greeting }} World").unwrap();
        assert_eq!(result, "Hello World");
    }

    // ---------------------------------------------------------------
    // task namespace
    // ---------------------------------------------------------------

    #[test]
    fn test_task_result_access() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_task_result("task1", json!({"status": "success"}));

        let result = ctx
            .render_template("Status: {{ task.task1.status }}")
            .unwrap();
        assert_eq!(result, "Status: success");
    }

    #[test]
    fn test_task_result_deep_access() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_task_result("fetch", json!({"data": {"id": 42}}));

        let val = ctx.evaluate_expression("task.fetch.data.id").unwrap();
        assert_eq!(val, json!(42));
    }

    #[test]
    fn test_task_result_stdout() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_task_result("run_cmd", json!({"stdout": "hello world"}));

        let val = ctx.evaluate_expression("task.run_cmd.stdout").unwrap();
        assert_eq!(val, json!("hello world"));
    }

    // ---------------------------------------------------------------
    // config namespace (pack configuration)
    // ---------------------------------------------------------------

    #[test]
    fn test_config_namespace() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_pack_config(
            json!({"api_token": "tok_abc123", "base_url": "https://api.example.com"}),
        );

        let val = ctx.evaluate_expression("config.api_token").unwrap();
        assert_eq!(val, json!("tok_abc123"));

        let result = ctx.render_template("URL: {{ config.base_url }}").unwrap();
        assert_eq!(result, "URL: https://api.example.com");
    }

    #[test]
    fn test_config_namespace_nested() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_pack_config(json!({"slack": {"webhook_url": "https://hooks.slack.com/xxx"}}));

        let val = ctx.evaluate_expression("config.slack.webhook_url").unwrap();
        assert_eq!(val, json!("https://hooks.slack.com/xxx"));
    }

    // ---------------------------------------------------------------
    // keystore namespace (encrypted secrets)
    // ---------------------------------------------------------------

    #[test]
    fn test_keystore_namespace() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_keystore(json!({"secret_key": "s3cr3t", "db_password": "hunter2"}));

        let val = ctx.evaluate_expression("keystore.secret_key").unwrap();
        assert_eq!(val, json!("s3cr3t"));

        let val = ctx.evaluate_expression("keystore.db_password").unwrap();
        assert_eq!(val, json!("hunter2"));
    }

    #[test]
    fn test_keystore_bracket_access() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_keystore(json!({"My Secret Key": "value123"}));

        let val = ctx
            .evaluate_expression("keystore[\"My Secret Key\"]")
            .unwrap();
        assert_eq!(val, json!("value123"));
    }

    // ---------------------------------------------------------------
    // item / index (with_items iteration)
    // ---------------------------------------------------------------

    #[test]
    fn test_item_context() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_current_item(json!({"name": "item1"}), 0);

        let result = ctx
            .render_template("Item: {{ item.name }}, Index: {{ index }}")
            .unwrap();
        assert_eq!(result, "Item: item1, Index: 0");
    }

    // ---------------------------------------------------------------
    // Condition evaluation
    // ---------------------------------------------------------------

    #[test]
    fn test_condition_evaluation() {
        let params = json!({"enabled": true});
        let ctx = WorkflowContext::new(params, HashMap::new());

        assert!(ctx.evaluate_condition("true").unwrap());
        assert!(!ctx.evaluate_condition("false").unwrap());
    }

    #[test]
    fn test_condition_with_comparison() {
        let ctx = WorkflowContext::new(json!({"count": 10}), HashMap::new());
        assert!(ctx.evaluate_condition("parameters.count > 5").unwrap());
        assert!(!ctx.evaluate_condition("parameters.count < 5").unwrap());
        assert!(ctx.evaluate_condition("parameters.count == 10").unwrap());
        assert!(ctx.evaluate_condition("parameters.count >= 10").unwrap());
        assert!(ctx.evaluate_condition("parameters.count != 99").unwrap());
    }

    #[test]
    fn test_condition_with_boolean_operators() {
        let ctx = WorkflowContext::new(json!({"x": 10, "y": 20}), HashMap::new());
        assert!(ctx
            .evaluate_condition("parameters.x > 5 and parameters.y > 15")
            .unwrap());
        assert!(!ctx
            .evaluate_condition("parameters.x > 5 and parameters.y > 25")
            .unwrap());
        assert!(ctx
            .evaluate_condition("parameters.x > 50 or parameters.y > 15")
            .unwrap());
        assert!(ctx.evaluate_condition("not parameters.x > 50").unwrap());
    }

    #[test]
    fn test_condition_with_in_operator() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_var("roles", json!(["admin", "user"]));
        // Via bare-name fallback
        assert!(ctx.evaluate_condition("\"admin\" in roles").unwrap());
        assert!(!ctx.evaluate_condition("\"root\" in roles").unwrap());
        // Via canonical workflow namespace
        assert!(ctx
            .evaluate_condition("\"admin\" in workflow.roles")
            .unwrap());
    }

    #[test]
    fn test_condition_with_function_calls() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_last_task_outcome(json!({"status": "ok", "code": 200}), TaskOutcome::Succeeded);
        assert!(ctx.evaluate_condition("succeeded()").unwrap());
        assert!(!ctx.evaluate_condition("failed()").unwrap());
        assert!(ctx
            .evaluate_condition("succeeded() and result().code == 200")
            .unwrap());
        assert!(!ctx
            .evaluate_condition("succeeded() and result().code == 404")
            .unwrap());
    }

    #[test]
    fn test_condition_with_length() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_var("items", json!([1, 2, 3, 4, 5]));
        assert!(ctx.evaluate_condition("length(items) > 3").unwrap());
        assert!(!ctx.evaluate_condition("length(items) > 10").unwrap());
        assert!(ctx.evaluate_condition("length(items) == 5").unwrap());
    }

    #[test]
    fn test_condition_with_config() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_pack_config(json!({"retries": 3}));
        assert!(ctx.evaluate_condition("config.retries > 0").unwrap());
        assert!(ctx.evaluate_condition("config.retries == 3").unwrap());
    }

    // ---------------------------------------------------------------
    // Expression engine in templates
    // ---------------------------------------------------------------

    #[test]
    fn test_expression_arithmetic() {
        let ctx = WorkflowContext::new(json!({"x": 10}), HashMap::new());
        let input = json!({"result": "{{ parameters.x + 5 }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["result"], json!(15));
    }

    #[test]
    fn test_expression_string_concat() {
        let ctx =
            WorkflowContext::new(json!({"first": "Hello", "second": "World"}), HashMap::new());
        let input = json!({"msg": "{{ parameters.first + \" \" + parameters.second }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["msg"], json!("Hello World"));
    }

    #[test]
    fn test_expression_nested_functions() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_var("data", json!("a,b,c"));
        let input = json!({"count": "{{ length(split(data, \",\")) }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["count"], json!(3));
    }

    #[test]
    fn test_expression_bracket_access() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_var("arr", json!([10, 20, 30]));
        let input = json!({"second": "{{ arr[1] }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["second"], json!(20));
    }

    #[test]
    fn test_expression_type_conversion() {
        let ctx = WorkflowContext::new(json!({}), HashMap::new());
        let input = json!({"val": "{{ int(3.9) }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["val"], json!(3));
    }

    // ---------------------------------------------------------------
    // render_json type-preserving behaviour
    // ---------------------------------------------------------------

    #[test]
    fn test_render_json() {
        let params = json!({"name": "test"});
        let ctx = WorkflowContext::new(params, HashMap::new());

        let input = json!({
            "message": "Hello {{ parameters.name }}",
            "count": 42,
            "nested": {
                "value": "Name is {{ parameters.name }}"
            }
        });

        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["message"], "Hello test");
        assert_eq!(result["count"], 42);
        assert_eq!(result["nested"]["value"], "Name is test");
    }

    #[test]
    fn test_render_json_type_preserving_number() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_current_item(json!(5), 0);

        // Pure expression — should preserve the integer type
        let input = json!({"seconds": "{{ item }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["seconds"], json!(5));
        assert!(result["seconds"].is_number());
    }

    #[test]
    fn test_render_json_type_preserving_array() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_last_task_outcome(
            json!({"data": {"items": [0, 1, 2, 3, 4]}}),
            TaskOutcome::Succeeded,
        );

        // Pure expression into result() — should preserve the array type
        let input = json!({"list": "{{ result().data.items }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["list"], json!([0, 1, 2, 3, 4]));
        assert!(result["list"].is_array());
    }

    #[test]
    fn test_render_json_mixed_template_stays_string() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_current_item(json!(5), 0);

        // Mixed text + template — must remain a string
        let input = json!({"msg": "Sleeping for {{ item }} seconds"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["msg"], json!("Sleeping for 5 seconds"));
        assert!(result["msg"].is_string());
    }

    #[test]
    fn test_render_json_type_preserving_bool() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_last_task_outcome(json!({}), TaskOutcome::Succeeded);

        let input = json!({"ok": "{{ succeeded() }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["ok"], json!(true));
        assert!(result["ok"].is_boolean());
    }

    // ---------------------------------------------------------------
    // result() / succeeded() / failed() / timed_out()
    // ---------------------------------------------------------------

    #[test]
    fn test_result_function() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_last_task_outcome(
            json!({"data": {"items": [10, 20]}, "stdout": "hello"}),
            TaskOutcome::Succeeded,
        );

        // result() returns the full last task result
        let val = ctx.evaluate_expression("result()").unwrap();
        assert_eq!(val["data"]["items"], json!([10, 20]));

        // result().stdout returns nested field
        let val = ctx.evaluate_expression("result().stdout").unwrap();
        assert_eq!(val, json!("hello"));

        // result().data.items returns deeper nested field
        let val = ctx.evaluate_expression("result().data.items").unwrap();
        assert_eq!(val, json!([10, 20]));
    }

    #[test]
    fn test_succeeded_failed_functions() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_last_task_outcome(json!({}), TaskOutcome::Succeeded);

        assert_eq!(ctx.evaluate_expression("succeeded()").unwrap(), json!(true));
        assert_eq!(ctx.evaluate_expression("failed()").unwrap(), json!(false));
        assert_eq!(
            ctx.evaluate_expression("timed_out()").unwrap(),
            json!(false)
        );

        ctx.set_last_task_outcome(json!({}), TaskOutcome::Failed);
        assert_eq!(
            ctx.evaluate_expression("succeeded()").unwrap(),
            json!(false)
        );
        assert_eq!(ctx.evaluate_expression("failed()").unwrap(), json!(true));

        ctx.set_last_task_outcome(json!({}), TaskOutcome::TimedOut);
        assert_eq!(ctx.evaluate_expression("timed_out()").unwrap(), json!(true));
    }

    // ---------------------------------------------------------------
    // Publish
    // ---------------------------------------------------------------

    #[test]
    fn test_publish_with_result_function() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_last_task_outcome(
            json!({"data": {"items": [0, 1, 2]}}),
            TaskOutcome::Succeeded,
        );

        let mut publish_map = HashMap::new();
        publish_map.insert(
            "number_list".to_string(),
            JsonValue::String("{{ result().data.items }}".to_string()),
        );

        ctx.publish_from_result(&json!({}), &[], Some(&publish_map))
            .unwrap();

        let val = ctx.get_var("number_list").unwrap();
        assert_eq!(val, json!([0, 1, 2]));
        assert!(val.is_array());
    }

    #[test]
    fn test_publish_variables() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        let result = json!({"output": "success"});

        ctx.publish_from_result(&result, &["my_var".to_string()], None)
            .unwrap();

        assert_eq!(ctx.get_var("my_var").unwrap(), result);
    }

    #[test]
    fn test_publish_typed_values() {
        // Non-string publish values (booleans, numbers, null) should pass
        // through render_json unchanged and be stored with their original type.
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_last_task_outcome(json!({"status": "ok"}), TaskOutcome::Succeeded);

        let mut publish_map = HashMap::new();
        publish_map.insert("flag".to_string(), JsonValue::Bool(true));
        publish_map.insert("count".to_string(), json!(42));
        publish_map.insert("ratio".to_string(), json!(3.15));
        publish_map.insert("nothing".to_string(), JsonValue::Null);
        publish_map.insert(
            "template".to_string(),
            JsonValue::String("{{ result().status }}".to_string()),
        );
        publish_map.insert(
            "plain_str".to_string(),
            JsonValue::String("hello".to_string()),
        );

        ctx.publish_from_result(&json!({}), &[], Some(&publish_map))
            .unwrap();

        // Boolean preserved as boolean (not string "true")
        assert_eq!(ctx.get_var("flag").unwrap(), json!(true));
        assert!(ctx.get_var("flag").unwrap().is_boolean());

        // Integer preserved
        assert_eq!(ctx.get_var("count").unwrap(), json!(42));
        assert!(ctx.get_var("count").unwrap().is_number());

        // Float preserved
        assert_eq!(ctx.get_var("ratio").unwrap(), json!(3.15));

        // Null preserved
        assert_eq!(ctx.get_var("nothing").unwrap(), json!(null));
        assert!(ctx.get_var("nothing").unwrap().is_null());

        // Template expression rendered with type preservation
        assert_eq!(ctx.get_var("template").unwrap(), json!("ok"));

        // Plain string stays as string
        assert_eq!(ctx.get_var("plain_str").unwrap(), json!("hello"));
    }

    #[test]
    fn test_published_var_accessible_via_workflow_namespace() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_var("counter", json!(42));

        // Via canonical namespace
        let val = ctx.evaluate_expression("workflow.counter").unwrap();
        assert_eq!(val, json!(42));

        // Via backward-compat alias
        let val = ctx.evaluate_expression("vars.counter").unwrap();
        assert_eq!(val, json!(42));

        // Via bare-name fallback
        let val = ctx.evaluate_expression("counter").unwrap();
        assert_eq!(val, json!(42));
    }

    // ---------------------------------------------------------------
    // Rebuild / Export / Import round-trip
    // ---------------------------------------------------------------

    #[test]
    fn test_rebuild_context() {
        let stored_vars = json!({"number_list": [0, 1, 2]});
        let mut task_results = HashMap::new();
        task_results.insert("task1".to_string(), json!({"data": {"items": [0, 1, 2]}}));

        let ctx = WorkflowContext::rebuild(json!({"count": 5}), &stored_vars, task_results);

        assert_eq!(ctx.get_var("number_list").unwrap(), json!([0, 1, 2]));
        assert_eq!(
            ctx.get_task_result("task1").unwrap(),
            json!({"data": {"items": [0, 1, 2]}})
        );
        let rendered = ctx.render_template("{{ parameters.count }}").unwrap();
        assert_eq!(rendered, "5");
    }

    #[test]
    fn test_export_import() {
        let mut ctx = WorkflowContext::new(json!({"key": "value"}), HashMap::new());
        ctx.set_var("test", json!("data"));
        ctx.set_task_result("task1", json!({"result": "ok"}));
        ctx.set_pack_config(json!({"setting": "val"}));
        ctx.set_keystore(json!({"secret": "hidden"}));

        let exported = ctx.export();
        let imported = WorkflowContext::import(exported).unwrap();

        assert_eq!(imported.get_var("test").unwrap(), json!("data"));
        assert_eq!(
            imported.get_task_result("task1").unwrap(),
            json!({"result": "ok"})
        );
        assert_eq!(
            imported.evaluate_expression("config.setting").unwrap(),
            json!("val")
        );
        assert_eq!(
            imported.evaluate_expression("keystore.secret").unwrap(),
            json!("hidden")
        );
    }

    // ---------------------------------------------------------------
    // with_items type preservation
    // ---------------------------------------------------------------

    #[test]
    fn test_with_items_integer_type_preservation() {
        // Simulates the sleep_2 task from the hello_workflow:
        // input: { seconds: "{{ item }}" }
        // with_items: [0, 1, 2, 3, 4]
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_current_item(json!(3), 3);

        let input = json!({
            "message": "Sleeping for {{ item }} seconds ",
            "seconds": "{{item}}"
        });

        let rendered = ctx.render_json(&input).unwrap();

        // seconds should be integer 3, not string "3"
        assert_eq!(rendered["seconds"], json!(3));
        assert!(rendered["seconds"].is_number());

        // message should be a string with the value interpolated
        assert_eq!(rendered["message"], json!("Sleeping for 3 seconds "));
        assert!(rendered["message"].is_string());
    }

    // ---------------------------------------------------------------
    // Cross-namespace expressions
    // ---------------------------------------------------------------

    #[test]
    fn test_cross_namespace_expression() {
        let mut ctx = WorkflowContext::new(json!({"limit": 5}), HashMap::new());
        ctx.set_var("items", json!([1, 2, 3]));
        ctx.set_pack_config(json!({"multiplier": 2}));

        assert!(ctx
            .evaluate_condition("length(workflow.items) < parameters.limit")
            .unwrap());
        let val = ctx
            .evaluate_expression("parameters.limit * config.multiplier")
            .unwrap();
        assert_eq!(val, json!(10));
    }

    #[test]
    fn test_keystore_in_template() {
        let mut ctx = WorkflowContext::new(json!({}), HashMap::new());
        ctx.set_keystore(json!({"api_key": "abc-123"}));

        let input = json!({"auth": "Bearer {{ keystore.api_key }}"});
        let result = ctx.render_json(&input).unwrap();
        assert_eq!(result["auth"], json!("Bearer abc-123"));
    }

    #[test]
    fn render_json_with_sensitivity_tracks_parameter_source() {
        let ctx = WorkflowContext::new(json!({"token": "secret-token"}), HashMap::new());
        ctx.mark_secret_pointer_paths("parameters", &["/token".to_string()], |path| {
            SecretSource::WorkflowParameter {
                execution_id: 42,
                path: path.clone(),
            }
        });

        let rendered = ctx
            .render_json_with_sensitivity(&json!({"password": "{{ parameters.token }}"}))
            .unwrap();

        assert_eq!(rendered.value["password"], "secret-token");
        assert_eq!(rendered.secret_paths, vec!["/password"]);
    }

    #[test]
    fn test_config_null_when_not_set() {
        let ctx = WorkflowContext::new(json!({}), HashMap::new());
        let val = ctx.evaluate_expression("config").unwrap();
        assert_eq!(val, json!(null));
    }
}
