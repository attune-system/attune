//! Workflow YAML parser
//!
//! This module handles parsing workflow YAML files into structured Rust types
//! that can be validated and stored in the database.
//!
//! Supports two task transition formats:
//!
//! **New format (Orquesta-style `next` array):**
//! ```yaml
//! tasks:
//!   - name: task1
//!     action: core.echo
//!     next:
//!       - when: "{{ succeeded() }}"
//!         publish:
//!           - result: "{{ result() }}"
//!         do:
//!           - task2
//!           - log
//!       - when: "{{ failed() }}"
//!         do:
//!           - error_handler
//! ```
//!
//! **Legacy format (flat fields):**
//! ```yaml
//! tasks:
//!   - name: task1
//!     action: core.echo
//!     on_success: task2
//!     on_failure: error_handler
//! ```
//!
//! When legacy fields are present, they are automatically converted to `next`
//! transitions during parsing. The canonical internal representation always
//! uses the `next` array.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use validator::Validate;

/// Result type for parser operations
pub type ParseResult<T> = Result<T, ParseError>;

/// Errors that can occur during workflow parsing
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("YAML parsing error: {0}")]
    YamlError(#[from] serde_yaml_ng::Error),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Invalid task reference: {0}")]
    InvalidTaskReference(String),

    #[error("Circular dependency detected: {0}")]
    CircularDependency(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid field value: {field} - {reason}")]
    InvalidField { field: String, reason: String },
}

impl From<validator::ValidationErrors> for ParseError {
    fn from(errors: validator::ValidationErrors) -> Self {
        ParseError::ValidationError(format!("{}", errors))
    }
}

impl From<ParseError> for crate::error::Error {
    fn from(err: ParseError) -> Self {
        crate::error::Error::validation(err.to_string())
    }
}

/// Complete workflow definition parsed from YAML
///
/// When loaded via an action's `workflow_file` field, the `ref` and `label`
/// fields are optional — the action YAML is authoritative for those values.
/// For standalone workflow files (in `workflows/`), they should be present.
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct WorkflowDefinition {
    /// Unique reference (e.g., "my_pack.deploy_app").
    ///
    /// Optional for action-linked workflow files (supplied by the action YAML).
    /// Required for standalone workflow files.
    #[serde(default)]
    #[validate(length(max = 255))]
    pub r#ref: String,

    /// Human-readable label.
    ///
    /// Optional for action-linked workflow files (supplied by the action YAML).
    /// Required for standalone workflow files.
    #[serde(default)]
    #[validate(length(max = 255))]
    pub label: String,

    /// Optional description
    pub description: Option<String>,

    /// Semantic version
    #[validate(length(min = 1, max = 50))]
    pub version: String,

    /// Input parameter schema (JSON Schema)
    pub parameters: Option<JsonValue>,

    /// Output schema (JSON Schema)
    pub output: Option<JsonValue>,

    /// Workflow-scoped variables with initial values
    #[serde(default)]
    pub vars: HashMap<String, JsonValue>,

    /// Task definitions
    #[validate(length(min = 1))]
    pub tasks: Vec<Task>,

    /// Output mapping (how to construct final workflow output)
    pub output_map: Option<HashMap<String, String>>,

    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Cancellation policy for the workflow.
    ///
    /// Controls what happens to running tasks when the workflow is cancelled:
    /// - `allow_finish` (default): Running tasks are allowed to complete naturally.
    ///   Only pending/requested tasks are cancelled. The workflow waits for running
    ///   tasks to finish but does not dispatch any new tasks.
    /// - `cancel_running`: All running and pending tasks are forcefully cancelled.
    ///   Running processes receive SIGINT → SIGTERM → SIGKILL via the worker.
    #[serde(default, skip_serializing_if = "CancellationPolicy::is_default")]
    pub cancellation_policy: CancellationPolicy,
}

// ---------------------------------------------------------------------------
// Task transition types (Orquesta-style)
// ---------------------------------------------------------------------------

/// A single task transition evaluated after task completion.
///
/// Transitions are evaluated in order. When `when` is not defined,
/// the transition is unconditional (fires on any completion).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTransition {
    /// Condition expression (e.g., "{{ succeeded() }}", "{{ failed() }}")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,

    /// Variables to publish into the workflow context on this transition
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publish: Vec<PublishDirective>,

    /// Next tasks to invoke when transition criteria is met
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#do: Option<Vec<String>>,

    /// Frontend-only visual metadata (label, color, line style, waypoints).
    /// Not consumed by the backend — preserved so the workflow builder can
    /// restore its visual state after a round-trip through the parser.
    #[serde(
        default,
        rename = "__chart_meta__",
        skip_serializing_if = "Option::is_none"
    )]
    pub chart_meta: Option<JsonValue>,
}

// ---------------------------------------------------------------------------
// Task definition
// ---------------------------------------------------------------------------

/// Task definition - can be action, parallel, or workflow type.
///
/// Supports both the new `next` transition format and legacy flat fields
/// (`on_success`, `on_failure`, etc.) for backward compatibility. During
/// deserialization the legacy fields are captured; call
/// [`Task::normalize_transitions`] (done automatically during parsing) to
/// merge them into the canonical `next` array.
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct Task {
    /// Unique task name within the workflow
    #[validate(length(min = 1, max = 255))]
    pub name: String,

    /// Task type (defaults to "action")
    #[serde(default = "default_task_type")]
    pub r#type: TaskType,

    /// Action reference (for action type tasks)
    pub action: Option<String>,

    /// Input parameters (template strings)
    #[serde(default)]
    pub input: HashMap<String, JsonValue>,

    /// Permission set refs to snapshot onto this task's execution token.
    ///
    /// May be a string, array of strings, or template expression resolving to
    /// either shape. If omitted, the task action's default execution permission
    /// set refs are used.
    #[serde(default, alias = "permission_set_ref")]
    pub permission_set_refs: Option<JsonValue>,

    /// Optional template used to resolve this task's execution trace tag.
    ///
    /// If omitted, the workflow parent execution trace tag is inherited.
    #[serde(default)]
    pub trace_tag_template: Option<String>,

    /// Worker selector override for this task's child execution.
    ///
    /// May be a literal object or a template expression resolving to an object.
    /// If omitted, the task action's default worker selector is used.
    #[serde(default)]
    pub worker_selector: Option<JsonValue>,

    /// Worker tolerations override for this task's child execution.
    ///
    /// May be a literal array or a template expression resolving to an array.
    /// If omitted, the task action's default worker tolerations are used.
    #[serde(default)]
    pub worker_tolerations: Option<JsonValue>,

    /// Worker affinity override for this task's child execution.
    ///
    /// May be a literal object or a template expression resolving to an object.
    /// If omitted, the task action's default worker affinity is used.
    #[serde(default)]
    pub worker_affinity: Option<JsonValue>,

    /// Conditional execution (task-level — controls whether this task runs)
    pub when: Option<String>,

    /// With-items iteration
    pub with_items: Option<String>,

    /// Iterate over entries from a cache namespace.
    #[serde(default)]
    pub iterate_cache: Option<IterateCacheConfig>,

    /// Batch size for with-items
    pub batch_size: Option<usize>,

    /// Concurrency limit for with-items
    pub concurrency: Option<usize>,

    /// Retry configuration
    pub retry: Option<RetryConfig>,

    /// Timeout in seconds.
    ///
    /// May be a literal integer or a template expression that resolves to an
    /// integer at execution time (e.g., `{{ parameters.task_timeout_seconds }}`).
    pub timeout: Option<JsonValue>,

    /// Orquesta-style transitions — the canonical representation.
    /// Each entry can specify a `when` condition, `publish` directives,
    /// and a list of next tasks (`do`).
    #[serde(default)]
    pub next: Vec<TaskTransition>,

    // -- Legacy transition fields (read during deserialization) -------------
    // These are kept for backward compatibility with older workflow YAML
    // files. During [`normalize_transitions`] they are folded into `next`.
    /// Legacy: transition on success
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success: Option<String>,

    /// Legacy: transition on failure
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,

    /// Legacy: transition on complete (regardless of status)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_complete: Option<String>,

    /// Legacy: transition on timeout
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_timeout: Option<String>,

    /// Legacy: decision-based transitions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision: Vec<DecisionBranch>,

    /// Legacy: task-level variable publishing (moved to per-transition in new model)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publish: Vec<PublishDirective>,

    /// Join barrier - wait for N inbound tasks to complete before executing.
    /// If not specified, task executes immediately when any predecessor completes.
    /// Special value "all" can be represented as the count of inbound edges.
    pub join: Option<usize>,

    /// Parallel tasks (for parallel type)
    pub tasks: Option<Vec<Task>>,

    /// Frontend-only visual metadata (e.g. canvas position).
    /// Not consumed by the backend — preserved so the workflow builder can
    /// restore its visual state after a round-trip through the parser.
    #[serde(
        default,
        rename = "__chart_meta__",
        skip_serializing_if = "Option::is_none"
    )]
    pub chart_meta: Option<JsonValue>,
}

/// Cache-backed task iteration configuration.
///
/// String selectors may contain workflow template expressions. A pure template
/// expression retains its rendered type when the executor resolves it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IterateCacheConfig {
    /// Optional cache owner type selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_type: Option<String>,

    /// Optional canonical cache owner reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_ref: Option<String>,

    /// Cache namespace name or template.
    pub namespace: String,

    /// `active`, a generation identifier, or a template resolving to either.
    #[serde(default = "default_cache_generation")]
    pub generation: String,

    /// Number of cache entries fetched per page.
    #[serde(default = "default_cache_page_size")]
    pub page_size: usize,

    /// Reject stale active cache generations when true.
    #[serde(default)]
    pub require_fresh: bool,
}

fn default_cache_generation() -> String {
    "active".to_string()
}

fn default_cache_page_size() -> usize {
    100
}

impl Task {
    /// Returns `true` if any legacy transition fields are populated.
    fn has_legacy_transitions(&self) -> bool {
        self.on_success.is_some()
            || self.on_failure.is_some()
            || self.on_complete.is_some()
            || self.on_timeout.is_some()
            || !self.decision.is_empty()
    }

    /// Convert legacy flat transition fields into the `next` array.
    ///
    /// If `next` is already populated, legacy fields are ignored (the new
    /// format takes precedence). After normalization the legacy fields are
    /// cleared so serialization only emits the canonical `next` form.
    pub fn normalize_transitions(&mut self) {
        // If `next` is already populated, the new format wins — clear legacy
        if !self.next.is_empty() {
            self.clear_legacy_fields();
            return;
        }

        // Nothing to convert
        if !self.has_legacy_transitions() && self.publish.is_empty() {
            return;
        }

        let mut transitions: Vec<TaskTransition> = Vec::new();

        if let Some(ref target) = self.on_success {
            transitions.push(TaskTransition {
                when: Some("{{ succeeded() }}".to_string()),
                publish: Vec::new(),
                r#do: Some(vec![target.clone()]),
                chart_meta: None,
            });
        }

        if let Some(ref target) = self.on_failure {
            transitions.push(TaskTransition {
                when: Some("{{ failed() }}".to_string()),
                publish: Vec::new(),
                r#do: Some(vec![target.clone()]),
                chart_meta: None,
            });
        }

        if let Some(ref target) = self.on_complete {
            // on_complete = unconditional
            transitions.push(TaskTransition {
                when: None,
                publish: Vec::new(),
                r#do: Some(vec![target.clone()]),
                chart_meta: None,
            });
        }

        if let Some(ref target) = self.on_timeout {
            transitions.push(TaskTransition {
                when: Some("{{ timed_out() }}".to_string()),
                publish: Vec::new(),
                r#do: Some(vec![target.clone()]),
                chart_meta: None,
            });
        }

        // Convert legacy decision branches
        for branch in &self.decision {
            transitions.push(TaskTransition {
                when: branch.when.clone(),
                publish: Vec::new(),
                r#do: Some(vec![branch.next.clone()]),
                chart_meta: None,
            });
        }

        // Attach legacy task-level publish to the first succeeded transition,
        // or create a publish-only transition if none exist
        if !self.publish.is_empty() {
            let succeeded_idx = transitions
                .iter()
                .position(|t| matches!(&t.when, Some(w) if w.contains("succeeded()")));

            if let Some(idx) = succeeded_idx {
                transitions[idx].publish = self.publish.clone();
            } else if transitions.is_empty() {
                transitions.push(TaskTransition {
                    when: Some("{{ succeeded() }}".to_string()),
                    publish: self.publish.clone(),
                    r#do: None,
                    chart_meta: None,
                });
            } else {
                // Attach to the first transition
                transitions[0].publish = self.publish.clone();
            }
        }

        self.next = transitions;
        self.clear_legacy_fields();
    }

    /// Clear legacy transition fields after normalization
    fn clear_legacy_fields(&mut self) {
        self.on_success = None;
        self.on_failure = None;
        self.on_complete = None;
        self.on_timeout = None;
        self.decision.clear();
        self.publish.clear();
    }

    /// Collect all task names referenced by transitions (both `next` and legacy).
    /// Used for validation.
    pub fn all_transition_targets(&self) -> Vec<&str> {
        let mut targets: Vec<&str> = Vec::new();

        // From `next` array
        for transition in &self.next {
            if let Some(ref do_list) = transition.r#do {
                for target in do_list {
                    targets.push(target.as_str());
                }
            }
        }

        // From legacy fields (in case normalize hasn't been called yet)
        if let Some(ref t) = self.on_success {
            targets.push(t.as_str());
        }
        if let Some(ref t) = self.on_failure {
            targets.push(t.as_str());
        }
        if let Some(ref t) = self.on_complete {
            targets.push(t.as_str());
        }
        if let Some(ref t) = self.on_timeout {
            targets.push(t.as_str());
        }
        for branch in &self.decision {
            targets.push(branch.next.as_str());
        }

        targets
    }
}

fn default_task_type() -> TaskType {
    TaskType::Action
}

/// Policy controlling how running tasks are handled when a workflow is cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CancellationPolicy {
    /// Running tasks are allowed to complete naturally; only pending tasks are
    /// cancelled and no new tasks are dispatched.  This is the default.
    #[default]
    AllowFinish,
    /// All running and pending tasks are forcefully cancelled.  Running
    /// processes receive SIGINT → SIGTERM → SIGKILL via the worker.
    CancelRunning,
}

impl CancellationPolicy {
    /// Returns `true` when the value is the default ([`AllowFinish`]).
    /// Used by `#[serde(skip_serializing_if)]` to keep stored JSON compact.
    pub fn is_default(&self) -> bool {
        matches!(self, Self::AllowFinish)
    }
}

/// Task type enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    /// Execute a single action
    Action,
    /// Execute multiple tasks in parallel
    Parallel,
    /// Execute another workflow
    Workflow,
}

/// Variable publishing directive
///
/// Publish directives map variable names to values.  Values may be template
/// expressions (strings containing `{{ }}`), literal strings, or any other
/// JSON-compatible type (booleans, numbers, arrays, objects).  Non-string
/// literals are preserved through the rendering pipeline so that, for example,
/// `validation_passed: true` publishes the boolean `true`, not the string
/// `"true"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PublishDirective {
    /// Key-value pair where the value can be any JSON-compatible type
    /// (string template, boolean, number, array, object, null).
    Simple(HashMap<String, serde_json::Value>),
    /// Just a key (publishes entire result under that key)
    Key(String),
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct RetryConfig {
    /// Number of retry attempts
    #[validate(range(min = 1, max = 100))]
    pub count: u32,

    /// Initial delay in seconds
    #[validate(range(min = 0))]
    pub delay: u32,

    /// Backoff strategy
    #[serde(default = "default_backoff")]
    pub backoff: BackoffStrategy,

    /// Maximum delay in seconds (for exponential backoff)
    pub max_delay: Option<u32>,

    /// Only retry on specific error conditions (template string)
    pub on_error: Option<String>,
}

fn default_backoff() -> BackoffStrategy {
    BackoffStrategy::Constant
}

/// Backoff strategy for retries
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BackoffStrategy {
    /// Constant delay between retries
    Constant,
    /// Linear increase in delay
    Linear,
    /// Exponential increase in delay
    Exponential,
}

/// Legacy decision-based transition (kept for backward compatibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionBranch {
    /// Condition to evaluate (template string)
    pub when: Option<String>,

    /// Task to transition to
    pub next: String,

    /// Whether this is the default branch
    #[serde(default)]
    pub default: bool,
}

// ---------------------------------------------------------------------------
// Parsing & validation
// ---------------------------------------------------------------------------

/// Parse workflow YAML string into WorkflowDefinition
pub fn parse_workflow_yaml(yaml: &str) -> ParseResult<WorkflowDefinition> {
    // Parse YAML
    let mut workflow: WorkflowDefinition = serde_yaml_ng::from_str(yaml)?;

    // Normalize legacy transitions into `next` arrays
    normalize_all_transitions(&mut workflow);

    // Validate structure
    workflow.validate()?;

    // Additional validation
    validate_workflow_structure(&workflow)?;

    Ok(workflow)
}

/// Parse workflow YAML file
pub fn parse_workflow_file(path: &std::path::Path) -> ParseResult<WorkflowDefinition> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| ParseError::ValidationError(format!("Failed to read file: {}", e)))?;
    parse_workflow_yaml(&contents)
}

/// Normalize all tasks in a workflow definition, converting legacy fields to `next`.
fn normalize_all_transitions(workflow: &mut WorkflowDefinition) {
    for task in &mut workflow.tasks {
        task.normalize_transitions();
        // Recursively normalize sub-tasks (parallel)
        if let Some(ref mut sub_tasks) = task.tasks {
            for sub in sub_tasks {
                sub.normalize_transitions();
            }
        }
    }
}

/// Validate workflow structure and references
fn validate_workflow_structure(workflow: &WorkflowDefinition) -> ParseResult<()> {
    // Collect all task names
    let task_names: std::collections::HashSet<_> =
        workflow.tasks.iter().map(|t| t.name.as_str()).collect();

    // Validate each task
    for task in &workflow.tasks {
        validate_task(task, &task_names)?;
    }

    // Cycles are now allowed in workflows - no cycle detection needed
    // Workflows are directed graphs (not DAGs) and cycles are supported
    // for use cases like monitoring loops, retry patterns, etc.

    Ok(())
}

/// Validate a single task
fn validate_task(task: &Task, task_names: &std::collections::HashSet<&str>) -> ParseResult<()> {
    // Validate action reference exists for action-type tasks
    if task.r#type == TaskType::Action && task.action.is_none() {
        return Err(ParseError::MissingField(format!(
            "Task '{}' of type 'action' must have an 'action' field",
            task.name
        )));
    }

    // Validate parallel tasks
    if task.r#type == TaskType::Parallel {
        if let Some(ref tasks) = task.tasks {
            if tasks.is_empty() {
                return Err(ParseError::InvalidField {
                    field: format!("Task '{}'", task.name),
                    reason: "Parallel task must contain at least one sub-task".to_string(),
                });
            }
        } else {
            return Err(ParseError::MissingField(format!(
                "Task '{}' of type 'parallel' must have a 'tasks' field",
                task.name
            )));
        }
    }

    // Validate all transition targets reference existing tasks
    for target in task.all_transition_targets() {
        if !task_names.contains(target) {
            return Err(ParseError::InvalidTaskReference(format!(
                "Task '{}' references non-existent task '{}'",
                task.name, target
            )));
        }
    }

    // Validate retry configuration
    if let Some(ref retry) = task.retry {
        retry.validate()?;
    }

    // Validate parallel sub-tasks recursively
    if let Some(ref tasks) = task.tasks {
        let subtask_names: std::collections::HashSet<_> =
            tasks.iter().map(|t| t.name.as_str()).collect();
        for subtask in tasks {
            validate_task(subtask, &subtask_names)?;
        }
    }

    Ok(())
}

// Cycle detection functions removed - cycles are now valid in workflow graphs
// Workflows are directed graphs (not DAGs) and cycles are supported
// for use cases like monitoring loops, retry patterns, etc.

/// Convert WorkflowDefinition to JSON for database storage
pub fn workflow_to_json(workflow: &WorkflowDefinition) -> Result<JsonValue, serde_json::Error> {
    serde_json::to_value(workflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Legacy format tests (backward compatibility)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_simple_workflow_legacy() {
        let yaml = r#"
ref: test.simple_workflow
label: Simple Workflow
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    input:
      message: "Hello"
    on_success: task2
  - name: task2
    action: core.echo
    input:
      message: "World"
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks.len(), 2);
        assert_eq!(workflow.tasks[0].name, "task1");

        // Legacy on_success should have been normalized into `next`
        assert!(workflow.tasks[0].on_success.is_none());
        assert_eq!(workflow.tasks[0].next.len(), 1);
        assert_eq!(
            workflow.tasks[0].next[0].when.as_deref(),
            Some("{{ succeeded() }}")
        );
        assert_eq!(
            workflow.tasks[0].next[0].r#do,
            Some(vec!["task2".to_string()])
        );
    }

    #[test]
    fn test_cycles_now_allowed_legacy() {
        let yaml = r#"
ref: test.circular
label: Circular Workflow (Now Allowed)
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    on_success: task2
  - name: task2
    action: core.echo
    on_success: task1
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Cycles should now be allowed in workflows");

        let workflow = result.unwrap();
        assert_eq!(workflow.tasks.len(), 2);
        assert_eq!(workflow.tasks[0].name, "task1");
        assert_eq!(workflow.tasks[1].name, "task2");
    }

    #[test]
    fn test_invalid_task_reference_legacy() {
        let yaml = r#"
ref: test.invalid_ref
label: Invalid Reference
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    on_success: nonexistent_task
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_err());
        match result {
            Err(ParseError::InvalidTaskReference(_)) => (),
            other => panic!("Expected InvalidTaskReference error, got: {:?}", other),
        }
    }

    #[test]
    fn test_parallel_task_legacy() {
        let yaml = r#"
ref: test.parallel
label: Parallel Workflow
version: 1.0.0
tasks:
  - name: parallel_checks
    type: parallel
    tasks:
      - name: check1
        action: core.check_a
      - name: check2
        action: core.check_b
    on_success: final_task
  - name: final_task
    action: core.complete
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks[0].r#type, TaskType::Parallel);
        assert_eq!(workflow.tasks[0].tasks.as_ref().unwrap().len(), 2);
        // Legacy on_success converted to next
        assert_eq!(workflow.tasks[0].next.len(), 1);
    }

    // -----------------------------------------------------------------------
    // New format tests (Orquesta-style `next`)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_next_format_simple() {
        let yaml = r#"
ref: test.next_simple
label: Next Format Workflow
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    input:
      message: "Hello"
    next:
      - when: "{{ succeeded() }}"
        do:
          - task2
  - name: task2
    action: core.echo
    input:
      message: "World"
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks.len(), 2);
        assert_eq!(workflow.tasks[0].next.len(), 1);
        assert_eq!(
            workflow.tasks[0].next[0].when.as_deref(),
            Some("{{ succeeded() }}")
        );
        assert_eq!(
            workflow.tasks[0].next[0].r#do,
            Some(vec!["task2".to_string()])
        );
    }

    #[test]
    fn test_parse_next_format_multiple_transitions() {
        let yaml = r#"
ref: test.next_multi
label: Multi-Transition Workflow
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        publish:
          - msg: "task1 done"
          - result_val: "{{ result() }}"
        do:
          - log
          - task3
      - when: "{{ failed() }}"
        publish:
          - msg: "task1 failed"
        do:
          - log
          - error_handler
  - name: task3
    action: core.complete
  - name: log
    action: core.log
  - name: error_handler
    action: core.handle_error
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();

        let task1 = &workflow.tasks[0];
        assert_eq!(task1.next.len(), 2);

        // First transition: succeeded
        assert_eq!(task1.next[0].when.as_deref(), Some("{{ succeeded() }}"));
        assert_eq!(task1.next[0].publish.len(), 2);
        assert_eq!(
            task1.next[0].r#do,
            Some(vec!["log".to_string(), "task3".to_string()])
        );

        // Second transition: failed
        assert_eq!(task1.next[1].when.as_deref(), Some("{{ failed() }}"));
        assert_eq!(task1.next[1].publish.len(), 1);
        assert_eq!(
            task1.next[1].r#do,
            Some(vec!["log".to_string(), "error_handler".to_string()])
        );
    }

    #[test]
    fn test_parse_next_format_publish_only() {
        let yaml = r#"
ref: test.publish_only
label: Publish Only Workflow
version: 1.0.0
tasks:
  - name: compute
    action: math.add
    next:
      - when: "{{ succeeded() }}"
        publish:
          - result: "{{ result() }}"
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        let task = &workflow.tasks[0];
        assert_eq!(task.next.len(), 1);
        assert!(task.next[0].r#do.is_none());
        assert_eq!(task.next[0].publish.len(), 1);
    }

    #[test]
    fn test_parse_next_format_unconditional() {
        let yaml = r#"
ref: test.unconditional
label: Unconditional Transition
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - do:
          - task2
  - name: task2
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks[0].next.len(), 1);
        assert!(workflow.tasks[0].next[0].when.is_none());
        assert_eq!(
            workflow.tasks[0].next[0].r#do,
            Some(vec!["task2".to_string()])
        );
    }

    #[test]
    fn test_next_takes_precedence_over_legacy() {
        // When both `next` and legacy fields are present, `next` wins
        let yaml = r#"
ref: test.precedence
label: Precedence Test
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    on_success: task2
    next:
      - when: "{{ succeeded() }}"
        do:
          - task3
  - name: task2
    action: core.echo
  - name: task3
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        let task1 = &workflow.tasks[0];

        // `next` should contain only the explicit next entry, not the legacy one
        assert_eq!(task1.next.len(), 1);
        assert_eq!(task1.next[0].r#do, Some(vec!["task3".to_string()]));
        // Legacy field should have been cleared
        assert!(task1.on_success.is_none());
    }

    #[test]
    fn test_invalid_task_reference_in_next() {
        let yaml = r#"
ref: test.invalid_next_ref
label: Invalid Next Ref
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - nonexistent_task
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_err());
        match result {
            Err(ParseError::InvalidTaskReference(msg)) => {
                assert!(msg.contains("nonexistent_task"));
            }
            other => panic!("Expected InvalidTaskReference error, got: {:?}", other),
        }
    }

    #[test]
    fn test_cycles_allowed_in_next_format() {
        let yaml = r#"
ref: test.cycle_next
label: Cycle with Next
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task2
  - name: task2
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task1
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Cycles should be allowed");
    }

    #[test]
    fn test_legacy_all_transition_types() {
        let yaml = r#"
ref: test.all_legacy
label: All Legacy Types
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    on_success: task_s
    on_failure: task_f
    on_complete: task_c
    on_timeout: task_t
  - name: task_s
    action: core.echo
  - name: task_f
    action: core.echo
  - name: task_c
    action: core.echo
  - name: task_t
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        let task1 = &workflow.tasks[0];

        // All legacy fields should be normalized into `next`
        assert_eq!(task1.next.len(), 4);
        assert!(task1.on_success.is_none());
        assert!(task1.on_failure.is_none());
        assert!(task1.on_complete.is_none());
        assert!(task1.on_timeout.is_none());

        // Check the order and conditions
        assert_eq!(task1.next[0].when.as_deref(), Some("{{ succeeded() }}"));
        assert_eq!(task1.next[0].r#do, Some(vec!["task_s".to_string()]));

        assert_eq!(task1.next[1].when.as_deref(), Some("{{ failed() }}"));
        assert_eq!(task1.next[1].r#do, Some(vec!["task_f".to_string()]));

        // on_complete → unconditional
        assert!(task1.next[2].when.is_none());
        assert_eq!(task1.next[2].r#do, Some(vec!["task_c".to_string()]));

        assert_eq!(task1.next[3].when.as_deref(), Some("{{ timed_out() }}"));
        assert_eq!(task1.next[3].r#do, Some(vec!["task_t".to_string()]));
    }

    #[test]
    fn test_legacy_publish_attached_to_succeeded_transition() {
        let yaml = r#"
ref: test.legacy_publish
label: Legacy Publish
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    on_success: task2
    publish:
      - result: "done"
  - name: task2
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        let task1 = &workflow.tasks[0];

        assert_eq!(task1.next.len(), 1);
        assert_eq!(task1.next[0].publish.len(), 1);
        assert!(task1.publish.is_empty()); // cleared after normalization
    }

    #[test]
    fn test_legacy_decision_branches() {
        let yaml = r#"
ref: test.decision
label: Decision Workflow
version: 1.0.0
tasks:
  - name: check
    action: core.check
    decision:
      - when: "{{ result().status == 'ok' }}"
        next: success_task
      - when: "{{ result().status == 'error' }}"
        next: error_task
  - name: success_task
    action: core.echo
  - name: error_task
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        let task = &workflow.tasks[0];

        assert_eq!(task.next.len(), 2);
        assert!(task.decision.is_empty()); // cleared
        assert_eq!(
            task.next[0].when.as_deref(),
            Some("{{ result().status == 'ok' }}")
        );
        assert_eq!(task.next[0].r#do, Some(vec!["success_task".to_string()]));
    }

    // -----------------------------------------------------------------------
    // Existing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_with_items() {
        let yaml = r#"
ref: test.iteration
label: Iteration Workflow
version: 1.0.0
tasks:
  - name: process_items
    action: core.process
    with_items: "{{ parameters.items }}"
    batch_size: 10
    input:
      item: "{{ item }}"
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok());
        let workflow = result.unwrap();
        assert!(workflow.tasks[0].with_items.is_some());
        assert_eq!(workflow.tasks[0].batch_size, Some(10));
    }

    #[test]
    fn test_task_timeout_accepts_literal_and_template() {
        // Task timeouts may be a literal integer or a template expression that
        // resolves to an integer at execution time. Both must parse cleanly.
        let yaml = r#"
ref: test.timeout_values
label: Timeout Values
version: 1.0.0
tasks:
  - name: literal_timeout
    action: core.echo
    timeout: 300
  - name: template_timeout
    action: core.echo
    timeout: "{{ parameters.task_timeout_seconds }}"
  - name: no_timeout
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks[0].timeout, Some(serde_json::json!(300)));
        assert_eq!(
            workflow.tasks[1].timeout,
            Some(serde_json::json!("{{ parameters.task_timeout_seconds }}"))
        );
        assert_eq!(workflow.tasks[2].timeout, None);

        // Round-trip through JSON must preserve both shapes.
        let json = workflow_to_json(&workflow).unwrap();
        assert_eq!(json["tasks"][0]["timeout"], serde_json::json!(300));
        assert_eq!(
            json["tasks"][1]["timeout"],
            serde_json::json!("{{ parameters.task_timeout_seconds }}")
        );
    }

    #[test]
    fn test_iterate_cache_parses_explicit_and_default_fields() {
        let yaml = r#"
ref: test.cache_iteration
label: Cache Iteration Workflow
version: 1.0.0
tasks:
  - name: explicit_cache
    action: core.process
    iterate_cache:
      owner_type: pack
      owner_ref: "{{ parameters.pack_ref }}"
      namespace: inventory
      generation: "{{ parameters.generation }}"
      page_size: 250
      require_fresh: true
    batch_size: 25
    concurrency: 4
  - name: default_cache
    action: core.process
    iterate_cache:
      namespace: inventory
"#;

        let workflow = parse_workflow_yaml(yaml).unwrap();
        let explicit = workflow.tasks[0].iterate_cache.as_ref().unwrap();
        assert_eq!(explicit.owner_type.as_deref(), Some("pack"));
        assert_eq!(
            explicit.owner_ref.as_deref(),
            Some("{{ parameters.pack_ref }}")
        );
        assert_eq!(explicit.namespace, "inventory");
        assert_eq!(explicit.generation, "{{ parameters.generation }}");
        assert_eq!(explicit.page_size, 250);
        assert!(explicit.require_fresh);

        let defaults = workflow.tasks[1].iterate_cache.as_ref().unwrap();
        assert_eq!(defaults.generation, "active");
        assert_eq!(defaults.page_size, 100);
        assert!(!defaults.require_fresh);
    }

    #[test]
    fn test_json_roundtrip() {
        let yaml = r#"
ref: test.roundtrip
label: Roundtrip Test
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        publish:
          - msg: "done"
        do:
          - task2
  - name: task2
    action: core.echo
"#;

        let workflow = parse_workflow_yaml(yaml).unwrap();
        let json = workflow_to_json(&workflow).unwrap();

        // Verify the JSON has the `next` array
        let tasks = json.get("tasks").unwrap().as_array().unwrap();
        let task1_next = tasks[0].get("next").unwrap().as_array().unwrap();
        assert_eq!(task1_next.len(), 1);
        assert_eq!(
            task1_next[0].get("when").unwrap().as_str().unwrap(),
            "{{ succeeded() }}"
        );

        // Verify legacy fields are absent
        assert!(tasks[0].get("on_success").is_none());
    }

    #[test]
    fn test_workflow_with_join() {
        let yaml = r#"
ref: test.join
label: Join Workflow
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task3
  - name: task2
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task3
  - name: task3
    join: 2
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks[2].join, Some(2));
    }

    #[test]
    fn test_multiple_do_targets() {
        let yaml = r#"
ref: test.multi_do
label: Multiple Do Targets
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task2
          - task3
  - name: task2
    action: core.echo
  - name: task3
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();
        let task1 = &workflow.tasks[0];
        assert_eq!(task1.next.len(), 1);
        assert_eq!(
            task1.next[0].r#do,
            Some(vec!["task2".to_string(), "task3".to_string()])
        );
    }

    #[test]
    fn test_chart_meta_roundtrip() {
        // __chart_meta__ is frontend-only visual metadata that must survive
        // a parse → serialize → parse round-trip so the workflow builder can
        // restore node positions, edge colors, waypoints, etc.
        let yaml = r##"
ref: test.chart_meta
label: Chart Meta Roundtrip
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    __chart_meta__:
      position:
        x: 300
        y: 120
    next:
      - when: "{{ succeeded() }}"
        do:
          - task2
        __chart_meta__:
          label: main path
          color: "#22c55e"
          line_style: dashed
          edge_waypoints:
            task2:
              - x: 400
                y: 200
          label_positions:
            task2: 0.35
  - name: task2
    action: core.echo
    __chart_meta__:
      position:
        x: 300
        y: 320
"##;

        let workflow = parse_workflow_yaml(yaml).unwrap();

        // Verify task-level __chart_meta__ was parsed
        let task1_meta = workflow.tasks[0].chart_meta.as_ref().unwrap();
        assert_eq!(task1_meta["position"]["x"], 300);
        assert_eq!(task1_meta["position"]["y"], 120);

        let task2_meta = workflow.tasks[1].chart_meta.as_ref().unwrap();
        assert_eq!(task2_meta["position"]["x"], 300);
        assert_eq!(task2_meta["position"]["y"], 320);

        // Verify transition-level __chart_meta__ was parsed
        let trans_meta = workflow.tasks[0].next[0].chart_meta.as_ref().unwrap();
        assert_eq!(trans_meta["label"], "main path");
        assert_eq!(trans_meta["color"].as_str().unwrap(), "#22c55e");
        assert_eq!(trans_meta["line_style"], "dashed");
        assert_eq!(trans_meta["edge_waypoints"]["task2"][0]["x"], 400);
        assert_eq!(trans_meta["label_positions"]["task2"], 0.35);

        // Round-trip through JSON serialization (simulates DB storage path)
        let json = workflow_to_json(&workflow).unwrap();
        let tasks = json["tasks"].as_array().unwrap();

        // Task __chart_meta__ survives
        assert_eq!(tasks[0]["__chart_meta__"]["position"]["x"], 300);
        assert_eq!(tasks[1]["__chart_meta__"]["position"]["y"], 320);

        // Transition __chart_meta__ survives
        let next0 = &tasks[0]["next"].as_array().unwrap()[0];
        assert_eq!(next0["__chart_meta__"]["label"], "main path");
        assert_eq!(
            next0["__chart_meta__"]["color"].as_str().unwrap(),
            "#22c55e"
        );
        assert_eq!(
            next0["__chart_meta__"]["edge_waypoints"]["task2"][0]["x"],
            400
        );
        assert_eq!(next0["__chart_meta__"]["label_positions"]["task2"], 0.35);

        // Round-trip through YAML serialization (simulates file storage path)
        let yaml_out = serde_yaml_ng::to_string(&workflow).unwrap();
        let workflow2 = parse_workflow_yaml(&yaml_out).unwrap();

        let task1_meta2 = workflow2.tasks[0].chart_meta.as_ref().unwrap();
        assert_eq!(task1_meta2["position"]["x"], 300);

        let trans_meta2 = workflow2.tasks[0].next[0].chart_meta.as_ref().unwrap();
        assert_eq!(trans_meta2["label"], "main path");
        assert_eq!(trans_meta2["color"].as_str().unwrap(), "#22c55e");
        assert_eq!(trans_meta2["edge_waypoints"]["task2"][0]["x"], 400);
    }

    #[test]
    fn test_chart_meta_absent_by_default() {
        // Workflows without __chart_meta__ should parse fine with None values
        let yaml = r#"
ref: test.no_meta
label: No Chart Meta
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task2
  - name: task2
    action: core.echo
"#;

        let workflow = parse_workflow_yaml(yaml).unwrap();
        assert!(workflow.tasks[0].chart_meta.is_none());
        assert!(workflow.tasks[1].chart_meta.is_none());
        assert!(workflow.tasks[0].next[0].chart_meta.is_none());

        // Serialize to JSON and verify __chart_meta__ is omitted (not null)
        let json = workflow_to_json(&workflow).unwrap();
        let tasks = json["tasks"].as_array().unwrap();
        assert!(tasks[0].get("__chart_meta__").is_none());
        assert!(tasks[0]["next"][0].get("__chart_meta__").is_none());
    }

    #[test]
    fn test_legacy_transitions_dont_gain_chart_meta() {
        // Legacy format conversion should produce transitions without chart_meta
        let yaml = r#"
ref: test.legacy_no_meta
label: Legacy No Meta
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    on_success: task2
    on_failure: task2
  - name: task2
    action: core.echo
"#;

        let workflow = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(workflow.tasks[0].next.len(), 2);
        assert!(workflow.tasks[0].next[0].chart_meta.is_none());
        assert!(workflow.tasks[0].next[1].chart_meta.is_none());
    }

    // -----------------------------------------------------------------------
    // Action-linked workflow file (no ref/label)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_action_linked_workflow_without_ref_and_label() {
        // Action-linked workflow files (in actions/workflows/) omit ref and
        // label — those are supplied by the companion action YAML.  The
        // parser must accept such files and default the fields to empty
        // strings.
        let yaml = r#"
version: 1.0.0

vars:
  counter: 0

tasks:
  - name: step1
    action: core.echo
    input:
      message: "hello"
    next:
      - when: "{{ succeeded() }}"
        do:
          - step2
  - name: step2
    action: core.echo
    input:
      message: "world"

output_map:
  result: "{{ task.step2.result }}"
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();

        // ref and label default to empty strings
        assert_eq!(workflow.r#ref, "");
        assert_eq!(workflow.label, "");

        // Graph fields are parsed normally
        assert_eq!(workflow.version, "1.0.0");
        assert_eq!(workflow.tasks.len(), 2);
        assert_eq!(workflow.tasks[0].name, "step1");
        assert!(workflow.vars.contains_key("counter"));
        assert!(workflow.output_map.is_some());

        // No parameters or output schema (those come from the action YAML)
        assert!(workflow.parameters.is_none());
        assert!(workflow.output.is_none());
        assert!(workflow.tags.is_empty());
    }

    #[test]
    fn test_parse_standalone_workflow_still_works_with_ref_and_label() {
        // Standalone workflow files (in workflows/) still carry ref and label.
        // Verify they continue to parse correctly.
        let yaml = r#"
ref: mypack.deploy
label: Deploy Workflow
description: Deploys the application
version: 2.0.0

parameters:
  target:
    type: string
    required: true

tags:
  - deploy
  - production

tasks:
  - name: deploy
    action: core.run
    input:
      target: "{{ parameters.target }}"
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();

        assert_eq!(workflow.r#ref, "mypack.deploy");
        assert_eq!(workflow.label, "Deploy Workflow");
        assert_eq!(
            workflow.description.as_deref(),
            Some("Deploys the application")
        );
        assert_eq!(workflow.version, "2.0.0");
        assert!(workflow.parameters.is_some());
        assert_eq!(workflow.tags, vec!["deploy", "production"]);
    }

    #[test]
    fn test_typed_publish_values_in_transitions() {
        // Regression test: publish directive values that are booleans, numbers,
        // or null must parse successfully (not just strings).  Previously
        // `PublishDirective::Simple(HashMap<String, String>)` rejected them.
        let yaml = r#"
ref: test.typed_publish
label: Typed Publish
version: 1.0.0
tasks:
  - name: validate
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        publish:
          - validation_passed: true
          - count: 42
          - ratio: 3.15
          - label: "hello"
          - template_val: "{{ result().data }}"
          - nothing: null
        do:
          - finalize
      - when: "{{ failed() }}"
        publish:
          - validation_passed: false
        do:
          - handle_error
  - name: finalize
    action: core.echo
  - name: handle_error
    action: core.echo
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok(), "Parse failed: {:?}", result.err());
        let workflow = result.unwrap();

        let task = &workflow.tasks[0];
        assert_eq!(task.name, "validate");
        assert_eq!(task.next.len(), 2);

        // Success transition: 6 publish directives with mixed types
        let success_transition = &task.next[0];
        assert_eq!(success_transition.publish.len(), 6);

        // Verify each typed value survived parsing
        for directive in &success_transition.publish {
            if let PublishDirective::Simple(map) = directive {
                if let Some(val) = map.get("validation_passed") {
                    assert_eq!(val, &serde_json::Value::Bool(true), "boolean true");
                } else if let Some(val) = map.get("count") {
                    assert_eq!(val, &serde_json::json!(42), "integer");
                } else if let Some(val) = map.get("ratio") {
                    assert_eq!(val, &serde_json::json!(3.15), "float");
                } else if let Some(val) = map.get("label") {
                    assert_eq!(val, &serde_json::json!("hello"), "string");
                } else if let Some(val) = map.get("template_val") {
                    assert_eq!(val, &serde_json::json!("{{ result().data }}"), "template");
                } else if let Some(val) = map.get("nothing") {
                    assert!(val.is_null(), "null");
                }
            }
        }

        // Failure transition: boolean false
        let failure_transition = &task.next[1];
        assert_eq!(failure_transition.publish.len(), 1);
        if let PublishDirective::Simple(map) = &failure_transition.publish[0] {
            assert_eq!(
                map.get("validation_passed"),
                Some(&serde_json::Value::Bool(false))
            );
        } else {
            panic!("Expected Simple publish directive");
        }
    }

    #[test]
    fn test_cancellation_policy_defaults_to_allow_finish() {
        let yaml = r#"
            version: "1.0.0"
            tasks:
              - name: task1
                action: core.echo
                input:
                  message: hello
        "#;
        let workflow = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(
            workflow.cancellation_policy,
            CancellationPolicy::AllowFinish
        );
    }

    #[test]
    fn test_cancellation_policy_cancel_running() {
        let yaml = r#"
            version: "1.0.0"
            cancellation_policy: cancel_running
            tasks:
              - name: task1
                action: core.echo
                input:
                  message: hello
        "#;
        let workflow = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(
            workflow.cancellation_policy,
            CancellationPolicy::CancelRunning
        );
    }

    #[test]
    fn test_cancellation_policy_allow_finish_explicit() {
        let yaml = r#"
            version: "1.0.0"
            cancellation_policy: allow_finish
            tasks:
              - name: task1
                action: core.echo
                input:
                  message: hello
        "#;
        let workflow = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(
            workflow.cancellation_policy,
            CancellationPolicy::AllowFinish
        );
    }

    #[test]
    fn test_cancellation_policy_json_roundtrip() {
        let yaml = r#"
            version: "1.0.0"
            cancellation_policy: cancel_running
            tasks:
              - name: step1
                action: core.echo
                input:
                  message: hello
        "#;
        let workflow = parse_workflow_yaml(yaml).unwrap();
        let json = workflow_to_json(&workflow).unwrap();
        let restored: WorkflowDefinition = serde_json::from_value(json).unwrap();
        assert_eq!(
            restored.cancellation_policy,
            CancellationPolicy::CancelRunning
        );
    }

    #[test]
    fn test_task_permission_set_refs_parse_array_and_alias() {
        let yaml = r#"
            version: "1.0.0"
            tasks:
              - name: literal_refs
                action: core.echo
                permission_set_refs:
                  - core.reader
                  - "{{ workflow.dynamic_ref }}"
              - name: singular_ref
                action: core.echo
                permission_set_ref: "{{ parameters.permission_set }}"
        "#;
        let workflow = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(
            workflow.tasks[0].permission_set_refs,
            Some(serde_json::json!([
                "core.reader",
                "{{ workflow.dynamic_ref }}"
            ]))
        );
        assert_eq!(
            workflow.tasks[1].permission_set_refs,
            Some(serde_json::json!("{{ parameters.permission_set }}"))
        );
    }

    #[test]
    fn test_cancellation_policy_absent_in_json_defaults() {
        // Simulate a definition stored in the DB before this field existed
        let json = serde_json::json!({
            "ref": "test.wf",
            "label": "Test",
            "version": "1.0.0",
            "tasks": [{"name": "t1", "action": "core.echo", "input": {"message": "hi"}}]
        });
        let workflow: WorkflowDefinition = serde_json::from_value(json).unwrap();
        assert_eq!(
            workflow.cancellation_policy,
            CancellationPolicy::AllowFinish
        );
    }
}
