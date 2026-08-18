//! Workflow YAML parser
//!
//! This module handles parsing workflow YAML files into structured Rust types
//! that can be validated and stored in the database.

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
    YamlError(#[from] serde_yaml::Error),

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

impl From<ParseError> for attune_common::error::Error {
    fn from(err: ParseError) -> Self {
        attune_common::error::Error::validation(err.to_string())
    }
}

/// Complete workflow definition parsed from YAML
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct WorkflowDefinition {
    /// Unique reference (e.g., "my_pack.deploy_app")
    #[validate(length(min = 1, max = 255))]
    pub r#ref: String,

    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
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
}

/// Task definition - can be action, parallel, or workflow type
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

    /// Conditional execution
    pub when: Option<String>,

    /// With-items iteration
    pub with_items: Option<String>,

    /// Batch size for with-items
    pub batch_size: Option<usize>,

    /// Concurrency limit for with-items
    pub concurrency: Option<usize>,

    /// Variable publishing
    #[serde(default)]
    pub publish: Vec<PublishDirective>,

    /// Retry configuration
    pub retry: Option<RetryConfig>,

    /// Timeout in seconds.
    ///
    /// May be a literal integer or a template expression that resolves to an
    /// integer at execution time (e.g., `{{ parameters.task_timeout_seconds }}`).
    pub timeout: Option<JsonValue>,

    /// Transition on success
    pub on_success: Option<String>,

    /// Transition on failure
    pub on_failure: Option<String>,

    /// Transition on complete (regardless of status)
    pub on_complete: Option<String>,

    /// Transition on timeout
    pub on_timeout: Option<String>,

    /// Decision-based transitions
    #[serde(default)]
    pub decision: Vec<DecisionBranch>,

    /// Parallel tasks (for parallel type)
    pub tasks: Option<Vec<Task>>,
}

fn default_task_type() -> TaskType {
    TaskType::Action
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
/// Values may be template expressions (strings containing `{{ }}`), literal
/// strings, or any other JSON-compatible type (booleans, numbers, arrays,
/// objects).  Non-string literals are preserved through the rendering pipeline.
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

/// Decision-based transition
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

/// Parse workflow YAML string into WorkflowDefinition
pub fn parse_workflow_yaml(yaml: &str) -> ParseResult<WorkflowDefinition> {
    // Parse YAML
    let workflow: WorkflowDefinition = serde_yaml::from_str(yaml)?;

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

    // Validate transitions reference existing tasks
    for transition in [
        &task.on_success,
        &task.on_failure,
        &task.on_complete,
        &task.on_timeout,
    ]
    .iter()
    .filter_map(|t| t.as_ref())
    {
        if !task_names.contains(transition.as_str()) {
            return Err(ParseError::InvalidTaskReference(format!(
                "Task '{}' references non-existent task '{}'",
                task.name, transition
            )));
        }
    }

    // Validate decision branches
    for branch in &task.decision {
        if !task_names.contains(branch.next.as_str()) {
            return Err(ParseError::InvalidTaskReference(format!(
                "Task '{}' decision branch references non-existent task '{}'",
                task.name, branch.next
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

    #[test]
    fn test_parse_simple_workflow() {
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
        assert!(result.is_ok());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks.len(), 2);
        assert_eq!(workflow.tasks[0].name, "task1");
    }

    #[test]
    fn test_detect_circular_dependency() {
        let yaml = r#"
ref: test.circular
label: Circular Workflow
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
        assert!(result.is_err());
        match result {
            Err(ParseError::CircularDependency(_)) => (),
            _ => panic!("Expected CircularDependency error"),
        }
    }

    #[test]
    fn test_invalid_task_reference() {
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
            _ => panic!("Expected InvalidTaskReference error"),
        }
    }

    #[test]
    fn test_parallel_task() {
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
        assert!(result.is_ok());
        let workflow = result.unwrap();
        assert_eq!(workflow.tasks[0].r#type, TaskType::Parallel);
        assert_eq!(workflow.tasks[0].tasks.as_ref().unwrap().len(), 2);
    }

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
    fn test_retry_config() {
        let yaml = r#"
ref: test.retry
label: Retry Workflow
version: 1.0.0
tasks:
  - name: flaky_task
    action: core.flaky
    retry:
      count: 5
      delay: 10
      backoff: exponential
      max_delay: 60
"#;

        let result = parse_workflow_yaml(yaml);
        assert!(result.is_ok());
        let workflow = result.unwrap();
        let retry = workflow.tasks[0].retry.as_ref().unwrap();
        assert_eq!(retry.count, 5);
        assert_eq!(retry.delay, 10);
        assert_eq!(retry.backoff, BackoffStrategy::Exponential);
    }
}
