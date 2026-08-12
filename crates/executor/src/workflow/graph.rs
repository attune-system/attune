//! Task Graph Builder
//!
//! This module builds executable task graphs from workflow definitions.
//! Workflows are directed graphs where tasks are nodes and transitions are edges.
//! Execution follows transitions from completed tasks, naturally supporting cycles.
//!
//! Uses the Orquesta-style `next` transition model where each task has an ordered
//! list of transitions. Each transition can specify:
//!   - `when` — a condition expression (e.g., "{{ succeeded() }}", "{{ failed() }}")
//!   - `publish` — variables to publish into the workflow context
//!   - `do` — next tasks to invoke when the condition is met

use attune_common::workflow::{IterateCacheConfig, Task, TaskType, WorkflowDefinition};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};

/// Result type for graph operations
pub type GraphResult<T> = Result<T, GraphError>;

/// Errors that can occur during graph building
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("Invalid task reference: {0}")]
    InvalidTaskReference(String),
}

/// Executable task graph
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskGraph {
    /// All nodes in the graph
    pub nodes: HashMap<String, TaskNode>,

    /// Entry points (tasks with no inbound edges)
    pub entry_points: Vec<String>,

    /// Inbound edges map (task -> tasks that can transition to it)
    pub inbound_edges: HashMap<String, HashSet<String>>,

    /// Outbound edges map (task -> tasks it can transition to)
    pub outbound_edges: HashMap<String, HashSet<String>>,
}

/// A node in the task graph
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskNode {
    /// Task name
    pub name: String,

    /// Task type
    pub task_type: TaskType,

    /// Action reference (for action tasks)
    pub action: Option<String>,

    /// Input template
    pub input: serde_json::Value,

    /// Templatable permission set refs for the child execution token
    #[serde(
        default,
        deserialize_with = "deserialize_present_json_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_set_refs: Option<JsonValue>,

    /// Optional template used to resolve the child execution trace tag.
    pub trace_tag_template: Option<String>,

    /// Templatable worker selector override for the child execution
    pub worker_selector: Option<JsonValue>,

    /// Templatable worker tolerations override for the child execution
    pub worker_tolerations: Option<JsonValue>,

    /// Templatable worker affinity override for the child execution
    pub worker_affinity: Option<JsonValue>,

    /// Conditional execution (task-level — controls whether the task runs at all)
    pub when: Option<String>,

    /// With-items iteration
    pub with_items: Option<String>,

    /// Cache-backed iteration configuration.
    #[serde(default)]
    pub iterate_cache: Option<IterateCacheConfig>,

    /// Batch size for iterations
    pub batch_size: Option<usize>,

    /// Concurrency limit
    pub concurrency: Option<usize>,

    /// Retry configuration
    pub retry: Option<RetryConfig>,

    /// Timeout in seconds
    pub timeout: Option<u32>,

    /// Orquesta-style transitions — evaluated in order after task completes
    pub transitions: Vec<GraphTransition>,

    /// Sub-tasks (for parallel tasks)
    pub sub_tasks: Option<Vec<TaskNode>>,

    /// Inbound tasks (computed - tasks that can transition to this one)
    pub inbound_tasks: HashSet<String>,

    /// Join count (if specified, wait for N inbound tasks to complete)
    pub join: Option<usize>,
}

/// A single transition in the task graph (Orquesta-style).
///
/// Transitions are evaluated in order after a task completes. Every matching
/// transition fires; when `when` is `None` the transition is unconditional.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphTransition {
    /// Condition expression (e.g., "{{ succeeded() }}", "{{ failed() }}")
    pub when: Option<String>,

    /// Variable publishing directives (key-value pairs)
    pub publish: Vec<PublishVar>,

    /// Next tasks to invoke when transition criteria is met
    pub do_tasks: Vec<String>,
}

fn deserialize_present_json_value<'de, D>(deserializer: D) -> Result<Option<JsonValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
}

/// A single publish variable (key = value).
///
/// The `value` field holds either a template expression (as a `JsonValue::String`
/// containing `{{ }}`), a literal string, or any other JSON-compatible type
/// (boolean, number, array, object, null).  The workflow context's `render_json`
/// method handles all of these: strings are template-rendered (with type
/// preservation for pure expressions), while non-string values pass through
/// unchanged.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PublishVar {
    pub name: String,
    /// The publish value — may be a template string, literal boolean, number,
    /// array, object, or null.  Renamed from `expression` (which only supported
    /// strings); the serde alias ensures existing serialized task graphs that
    /// use the old field name still deserialize correctly.
    #[serde(alias = "expression")]
    pub value: JsonValue,
}

/// Retry configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetryConfig {
    pub count: u32,
    pub delay: u32,
    pub backoff: BackoffStrategy,
    pub max_delay: Option<u32>,
    pub on_error: Option<String>,
}

/// Backoff strategy
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BackoffStrategy {
    Constant,
    Linear,
    Exponential,
}

// ---------------------------------------------------------------------------
// Transition classification helpers
// ---------------------------------------------------------------------------

/// Classify a `when` expression for quick matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    /// Matches `succeeded()` expressions
    Succeeded,
    /// Matches `failed()` expressions
    Failed,
    /// Matches `timed_out()` expressions
    TimedOut,
    /// No condition — fires on any completion
    Always,
    /// Custom condition expression
    Custom,
}

impl GraphTransition {
    /// Classify this transition's `when` expression into a [`TransitionKind`].
    pub fn kind(&self) -> TransitionKind {
        match &self.when {
            None => TransitionKind::Always,
            Some(expr) => {
                let normalized = expr.to_lowercase().replace(|c: char| c.is_whitespace(), "");
                match normalized.as_str() {
                    "succeeded()" | "{{succeeded()}}" => TransitionKind::Succeeded,
                    "failed()" | "{{failed()}}" => TransitionKind::Failed,
                    "timed_out()" | "{{timed_out()}}" => TransitionKind::TimedOut,
                    _ => TransitionKind::Custom,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TaskGraph implementation
// ---------------------------------------------------------------------------

impl TaskGraph {
    /// Create a graph from a workflow definition.
    ///
    /// The workflow's tasks should already have their transitions normalized
    /// (legacy `on_success`/`on_failure` fields merged into `next`) — this is
    /// done automatically by [`attune_common::workflow::parse_workflow_yaml`].
    pub fn from_workflow(workflow: &WorkflowDefinition) -> GraphResult<Self> {
        let mut builder = GraphBuilder::new();

        for task in &workflow.tasks {
            builder.add_task(task)?;
        }

        // Build the graph
        let builder = builder.build()?;
        Ok(builder.into())
    }

    /// Get a task node by name
    pub fn get_task(&self, name: &str) -> Option<&TaskNode> {
        self.nodes.get(name)
    }

    /// Get all tasks that can transition into the given task (inbound edges)
    #[allow(dead_code)] // Part of complete graph API; used in tests
    pub fn get_inbound_tasks(&self, task_name: &str) -> Vec<String> {
        self.inbound_edges
            .get(task_name)
            .map(|tasks| tasks.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get the next tasks to execute after a task completes.
    ///
    /// Evaluates transitions in order based on the task's completion status.
    /// A transition fires if its `when` condition matches the task status:
    ///   - `succeeded()` fires when `success == true`
    ///   - `failed()` fires when `success == false`
    ///   - No condition (always) fires regardless
    ///   - Custom conditions are included (actual expression evaluation
    ///     happens in the workflow coordinator with runtime context)
    ///
    /// Multiple transitions can fire — they are independent of each other.
    ///
    /// # Arguments
    /// * `task_name` - The name of the task that completed
    /// * `success` - Whether the task succeeded
    ///
    /// # Returns
    /// A vector of task names to schedule next
    #[allow(dead_code)] // Part of complete graph API; used in tests
    pub fn next_tasks(&self, task_name: &str, success: bool) -> Vec<String> {
        let mut next = Vec::new();

        if let Some(node) = self.nodes.get(task_name) {
            for transition in &node.transitions {
                let should_fire = match transition.kind() {
                    TransitionKind::Succeeded => success,
                    TransitionKind::Failed => !success,
                    TransitionKind::TimedOut => !success, // timeout is a form of failure
                    TransitionKind::Always => true,
                    TransitionKind::Custom => true, // include custom — real eval in coordinator
                };

                if should_fire {
                    for target in &transition.do_tasks {
                        if !next.contains(target) {
                            next.push(target.clone());
                        }
                    }
                }
            }
        }

        next
    }

    /// Get the next tasks with full transition information.
    ///
    /// Returns matching transitions with their publish directives and targets,
    /// giving the caller full context for variable publishing.
    #[allow(dead_code)] // Part of complete graph API; used in tests
    pub fn matching_transitions(&self, task_name: &str, success: bool) -> Vec<&GraphTransition> {
        let mut matching = Vec::new();

        if let Some(node) = self.nodes.get(task_name) {
            for transition in &node.transitions {
                let should_fire = match transition.kind() {
                    TransitionKind::Succeeded => success,
                    TransitionKind::Failed => !success,
                    TransitionKind::TimedOut => !success,
                    TransitionKind::Always => true,
                    TransitionKind::Custom => true,
                };

                if should_fire {
                    matching.push(transition);
                }
            }
        }

        matching
    }

    /// Collect all unique target task names from all transitions of a given task.
    #[allow(dead_code)] // Part of complete graph API; used in tests
    pub fn all_transition_targets(&self, task_name: &str) -> HashSet<String> {
        let mut targets = HashSet::new();
        if let Some(node) = self.nodes.get(task_name) {
            for transition in &node.transitions {
                for target in &transition.do_tasks {
                    targets.insert(target.clone());
                }
            }
        }
        targets
    }
}

// ---------------------------------------------------------------------------
// Graph builder
// ---------------------------------------------------------------------------

/// Graph builder helper
struct GraphBuilder {
    nodes: HashMap<String, TaskNode>,
    inbound_edges: HashMap<String, HashSet<String>>,
}

impl GraphBuilder {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            inbound_edges: HashMap::new(),
        }
    }

    fn add_task(&mut self, task: &Task) -> GraphResult<()> {
        let node = Self::task_to_node(task)?;
        self.nodes.insert(task.name.clone(), node);
        Ok(())
    }

    fn task_to_node(task: &Task) -> GraphResult<TaskNode> {
        let retry = task.retry.as_ref().map(|r| RetryConfig {
            count: r.count,
            delay: r.delay,
            backoff: match r.backoff {
                attune_common::workflow::BackoffStrategy::Constant => BackoffStrategy::Constant,
                attune_common::workflow::BackoffStrategy::Linear => BackoffStrategy::Linear,
                attune_common::workflow::BackoffStrategy::Exponential => {
                    BackoffStrategy::Exponential
                }
            },
            max_delay: r.max_delay,
            on_error: r.on_error.clone(),
        });

        // Convert parser TaskTransition list → graph GraphTransition list
        let transitions: Vec<GraphTransition> = task
            .next
            .iter()
            .map(|t| GraphTransition {
                when: t.when.clone(),
                publish: extract_publish_vars(&t.publish),
                do_tasks: t.r#do.clone().unwrap_or_default(),
            })
            .collect();

        let sub_tasks = if let Some(ref tasks) = task.tasks {
            let mut sub_nodes = Vec::new();
            for subtask in tasks {
                sub_nodes.push(Self::task_to_node(subtask)?);
            }
            Some(sub_nodes)
        } else {
            None
        };

        Ok(TaskNode {
            name: task.name.clone(),
            task_type: task.r#type.clone(),
            action: task.action.clone(),
            input: serde_json::to_value(&task.input).unwrap_or(serde_json::json!({})),
            permission_set_refs: task.permission_set_refs.clone(),
            trace_tag_template: task.trace_tag_template.clone(),
            worker_selector: task.worker_selector.clone(),
            worker_tolerations: task.worker_tolerations.clone(),
            worker_affinity: task.worker_affinity.clone(),
            when: task.when.clone(),
            with_items: task.with_items.clone(),
            iterate_cache: task.iterate_cache.clone(),
            batch_size: task.batch_size,
            concurrency: task.concurrency,
            retry,
            timeout: task.timeout,
            transitions,
            sub_tasks,
            inbound_tasks: HashSet::new(),
            join: task.join,
        })
    }

    fn build(mut self) -> GraphResult<Self> {
        // Compute inbound edges from transitions
        self.compute_inbound_edges()?;
        Ok(self)
    }

    fn compute_inbound_edges(&mut self) -> GraphResult<()> {
        let node_names: Vec<String> = self.nodes.keys().cloned().collect();

        for node_name in &node_names {
            // Collect all successor task names from this node's transitions
            let successors: Vec<String> = {
                let node = self.nodes.get(node_name).unwrap();
                node.transitions
                    .iter()
                    .flat_map(|t| t.do_tasks.iter().cloned())
                    .collect()
            };

            for successor in &successors {
                if !self.nodes.contains_key(successor) {
                    return Err(GraphError::InvalidTaskReference(format!(
                        "Task '{}' references non-existent task '{}'",
                        node_name, successor
                    )));
                }

                self.inbound_edges
                    .entry(successor.clone())
                    .or_default()
                    .insert(node_name.clone());
            }
        }

        // Update node inbound_tasks
        for (name, inbound) in &self.inbound_edges {
            if let Some(node) = self.nodes.get_mut(name) {
                node.inbound_tasks = inbound.clone();
            }
        }

        Ok(())
    }
}

impl From<GraphBuilder> for TaskGraph {
    fn from(builder: GraphBuilder) -> Self {
        // Entry points are tasks with no inbound edges
        let entry_points: Vec<String> = builder
            .nodes
            .keys()
            .filter(|name| {
                builder
                    .inbound_edges
                    .get(*name)
                    .map(|edges| edges.is_empty())
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        // Build outbound edges map (reverse of inbound)
        let mut outbound_edges: HashMap<String, HashSet<String>> = HashMap::new();
        for (task, inbound) in &builder.inbound_edges {
            for source in inbound {
                outbound_edges
                    .entry(source.clone())
                    .or_default()
                    .insert(task.clone());
            }
        }

        TaskGraph {
            nodes: builder.nodes,
            entry_points,
            inbound_edges: builder.inbound_edges,
            outbound_edges,
        }
    }
}

// ---------------------------------------------------------------------------
// Publish variable extraction
// ---------------------------------------------------------------------------

/// Extract publish variable names and expressions from parser publish directives.
fn extract_publish_vars(publish: &[attune_common::workflow::PublishDirective]) -> Vec<PublishVar> {
    use attune_common::workflow::PublishDirective;

    let mut vars = Vec::new();
    for directive in publish {
        match directive {
            PublishDirective::Simple(map) => {
                for (key, value) in map {
                    vars.push(PublishVar {
                        name: key.clone(),
                        value: value.clone(),
                    });
                }
            }
            PublishDirective::Key(key) => {
                vars.push(PublishVar {
                    name: key.clone(),
                    value: JsonValue::String("{{ result() }}".to_string()),
                });
            }
        }
    }
    vars
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workflow;

    #[test]
    fn test_simple_sequential_graph() {
        let yaml = r#"
ref: test.sequential
label: Sequential Workflow
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
          - task3
  - name: task3
    action: core.echo
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.entry_points.len(), 1);
        assert_eq!(graph.entry_points[0], "task1");

        // Check inbound edges
        assert!(graph
            .inbound_edges
            .get("task1")
            .map(|e| e.is_empty())
            .unwrap_or(true));
        assert_eq!(graph.inbound_edges["task2"].len(), 1);
        assert!(graph.inbound_edges["task2"].contains("task1"));
        assert_eq!(graph.inbound_edges["task3"].len(), 1);
        assert!(graph.inbound_edges["task3"].contains("task2"));

        // Check transitions via next_tasks
        let next = graph.next_tasks("task1", true);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0], "task2");

        let next = graph.next_tasks("task2", true);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0], "task3");
    }

    #[test]
    fn test_task_graph_preserves_worker_placement_overrides() {
        let yaml = r#"
ref: test.placement
label: Placement Workflow
version: 1.0.0
tasks:
  - name: gpu_task
    action: core.echo
    worker_selector:
      pool: gpu
    worker_tolerations:
      - key: dedicated
        operator: equal
        value: gpu
        effect: no_schedule
    worker_affinity:
      preferred:
        - weight: 50
          preference:
            match_labels:
              zone: east
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();
        let task = graph.get_task("gpu_task").unwrap();

        assert_eq!(
            task.worker_selector,
            Some(serde_json::json!({"pool": "gpu"}))
        );
        assert_eq!(
            task.worker_tolerations,
            Some(serde_json::json!([{
                "key": "dedicated",
                "operator": "equal",
                "value": "gpu",
                "effect": "no_schedule"
            }]))
        );
        assert_eq!(
            task.worker_affinity,
            Some(serde_json::json!({
                "preferred": [{
                    "weight": 50,
                    "preference": {
                        "match_labels": {"zone": "east"}
                    }
                }]
            }))
        );
    }

    #[test]
    fn test_simple_sequential_graph_legacy() {
        // Legacy format should still work (parser normalizes to `next`)
        let yaml = r#"
ref: test.sequential_legacy
label: Sequential Workflow (Legacy)
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    on_success: task2
  - name: task2
    action: core.echo
    on_success: task3
  - name: task3
    action: core.echo
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.entry_points.len(), 1);

        let next = graph.next_tasks("task1", true);
        assert_eq!(next, vec!["task2"]);

        let next = graph.next_tasks("task2", true);
        assert_eq!(next, vec!["task3"]);
    }

    #[test]
    fn test_parallel_entry_points() {
        let yaml = r#"
ref: test.parallel_start
label: Parallel Start
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - final_task
  - name: task2
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - final_task
  - name: final_task
    action: core.complete
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        assert_eq!(graph.entry_points.len(), 2);
        assert!(graph.entry_points.contains(&"task1".to_string()));
        assert!(graph.entry_points.contains(&"task2".to_string()));

        // final_task should have both as inbound edges
        assert_eq!(graph.inbound_edges["final_task"].len(), 2);
        assert!(graph.inbound_edges["final_task"].contains("task1"));
        assert!(graph.inbound_edges["final_task"].contains("task2"));
    }

    #[test]
    fn test_transitions_success_and_failure() {
        let yaml = r#"
ref: test.transitions
label: Transition Test
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task2
      - when: "{{ failed() }}"
        do:
          - error_handler
  - name: task2
    action: core.echo
  - name: error_handler
    action: core.handle_error
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        // On success, should go to task2
        let next = graph.next_tasks("task1", true);
        assert_eq!(next, vec!["task2"]);

        // On failure, should go to error_handler
        let next = graph.next_tasks("task1", false);
        assert_eq!(next, vec!["error_handler"]);

        // task2 has no transitions
        let next = graph.next_tasks("task2", true);
        assert!(next.is_empty());
    }

    #[test]
    fn test_multiple_do_targets() {
        let yaml = r#"
ref: test.multi_do
label: Multi Do Targets
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        publish:
          - msg: "task1 done"
        do:
          - log
          - task2
  - name: task2
    action: core.echo
  - name: log
    action: core.log
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        let next = graph.next_tasks("task1", true);
        assert_eq!(next.len(), 2);
        assert!(next.contains(&"log".to_string()));
        assert!(next.contains(&"task2".to_string()));

        // Check publish vars
        let transitions = graph.matching_transitions("task1", true);
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].publish.len(), 1);
        assert_eq!(transitions[0].publish[0].name, "msg");
        assert_eq!(
            transitions[0].publish[0].value,
            JsonValue::String("task1 done".to_string())
        );
    }

    #[test]
    fn test_unconditional_transition() {
        let yaml = r#"
ref: test.unconditional
label: Unconditional
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

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        // Unconditional fires on both success and failure
        let next = graph.next_tasks("task1", true);
        assert_eq!(next, vec!["task2"]);

        let next = graph.next_tasks("task1", false);
        assert_eq!(next, vec!["task2"]);
    }

    #[test]
    fn test_iterate_cache_propagates_to_graph_node_snapshot() {
        let yaml = r#"
ref: test.cache_iteration
label: Cache Iteration
version: 1.0.0
tasks:
  - name: process
    action: core.process
    iterate_cache:
      owner_type: pack
      owner_ref: test_pack
      namespace: inventory
      generation: "{{ parameters.generation }}"
      page_size: 200
      require_fresh: true
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();
        let iterate_cache = graph
            .get_task("process")
            .unwrap()
            .iterate_cache
            .as_ref()
            .unwrap();

        assert_eq!(iterate_cache.owner_type.as_deref(), Some("pack"));
        assert_eq!(iterate_cache.owner_ref.as_deref(), Some("test_pack"));
        assert_eq!(iterate_cache.namespace, "inventory");
        assert_eq!(iterate_cache.generation, "{{ parameters.generation }}");
        assert_eq!(iterate_cache.page_size, 200);
        assert!(iterate_cache.require_fresh);

        let snapshot = serde_json::to_value(&graph).unwrap();
        assert_eq!(
            snapshot["nodes"]["process"]["iterate_cache"]["namespace"],
            "inventory"
        );
    }

    #[test]
    fn test_cycle_support() {
        let yaml = r#"
ref: test.cycle
label: Cycle Test
version: 1.0.0
tasks:
  - name: check
    action: core.check
    next:
      - when: "{{ succeeded() }}"
        do:
          - process
      - when: "{{ failed() }}"
        do:
          - check
  - name: process
    action: core.process
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        // Should not error on cycles
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        // check has a self-reference (check -> check on failure)
        // So it has an inbound edge and is not an entry point
        // process also has an inbound edge (check -> process on success)
        assert_eq!(graph.entry_points.len(), 0);

        // check transitions to itself on failure (cycle)
        let next = graph.next_tasks("check", false);
        assert_eq!(next, vec!["check"]);

        // check transitions to process on success
        let next = graph.next_tasks("check", true);
        assert_eq!(next, vec!["process"]);
    }

    #[test]
    fn test_inbound_tasks() {
        let yaml = r#"
ref: test.inbound
label: Inbound Test
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - final_task
  - name: task2
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - final_task
  - name: final_task
    action: core.complete
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        let inbound = graph.get_inbound_tasks("final_task");
        assert_eq!(inbound.len(), 2);
        assert!(inbound.contains(&"task1".to_string()));
        assert!(inbound.contains(&"task2".to_string()));

        let inbound = graph.get_inbound_tasks("task1");
        assert_eq!(inbound.len(), 0);
    }

    #[test]
    fn test_transition_kind_classification() {
        let succeeded = GraphTransition {
            when: Some("{{ succeeded() }}".to_string()),
            publish: vec![],
            do_tasks: vec!["t".to_string()],
        };
        assert_eq!(succeeded.kind(), TransitionKind::Succeeded);

        let failed = GraphTransition {
            when: Some("{{ failed() }}".to_string()),
            publish: vec![],
            do_tasks: vec!["t".to_string()],
        };
        assert_eq!(failed.kind(), TransitionKind::Failed);

        let timed_out = GraphTransition {
            when: Some("{{ timed_out() }}".to_string()),
            publish: vec![],
            do_tasks: vec!["t".to_string()],
        };
        assert_eq!(timed_out.kind(), TransitionKind::TimedOut);

        let always = GraphTransition {
            when: None,
            publish: vec![],
            do_tasks: vec!["t".to_string()],
        };
        assert_eq!(always.kind(), TransitionKind::Always);

        let custom = GraphTransition {
            when: Some("{{ result().status == 'ok' }}".to_string()),
            publish: vec![],
            do_tasks: vec!["t".to_string()],
        };
        assert_eq!(custom.kind(), TransitionKind::Custom);

        let compound = GraphTransition {
            when: Some("{{ succeeded() and result().status == 'ok' }}".to_string()),
            publish: vec![],
            do_tasks: vec!["t".to_string()],
        };
        assert_eq!(compound.kind(), TransitionKind::Custom);
    }

    #[test]
    fn test_publish_extraction() {
        let yaml = r#"
ref: test.publish
label: Publish Test
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        publish:
          - result_val: "{{ result() }}"
          - msg: "done"
        do:
          - task2
  - name: task2
    action: core.echo
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        let task1 = graph.get_task("task1").unwrap();
        assert_eq!(task1.transitions.len(), 1);
        assert_eq!(task1.transitions[0].publish.len(), 2);

        // Note: HashMap ordering is not guaranteed, so just check both exist
        let publish_names: Vec<&str> = task1.transitions[0]
            .publish
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(publish_names.contains(&"result_val"));
        assert!(publish_names.contains(&"msg"));
    }

    #[test]
    fn test_all_transition_targets() {
        let yaml = r#"
ref: test.all_targets
label: All Targets Test
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - task2
          - task3
      - when: "{{ failed() }}"
        do:
          - error_handler
  - name: task2
    action: core.echo
  - name: task3
    action: core.echo
  - name: error_handler
    action: core.handle_error
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        let targets = graph.all_transition_targets("task1");
        assert_eq!(targets.len(), 3);
        assert!(targets.contains("task2"));
        assert!(targets.contains("task3"));
        assert!(targets.contains("error_handler"));
    }

    #[test]
    fn test_mixed_success_failure_and_always() {
        let yaml = r#"
ref: test.mixed
label: Mixed Transitions
version: 1.0.0
tasks:
  - name: task1
    action: core.echo
    next:
      - when: "{{ succeeded() }}"
        do:
          - success_task
      - when: "{{ failed() }}"
        do:
          - failure_task
      - do:
          - always_task
  - name: success_task
    action: core.echo
  - name: failure_task
    action: core.echo
  - name: always_task
    action: core.echo
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        // On success: succeeded + always fire
        let next = graph.next_tasks("task1", true);
        assert_eq!(next.len(), 2);
        assert!(next.contains(&"success_task".to_string()));
        assert!(next.contains(&"always_task".to_string()));

        // On failure: failed + always fire
        let next = graph.next_tasks("task1", false);
        assert_eq!(next.len(), 2);
        assert!(next.contains(&"failure_task".to_string()));
        assert!(next.contains(&"always_task".to_string()));
    }

    #[test]
    fn test_typed_publish_values() {
        // Verify that non-string publish values (booleans, numbers, null)
        // are preserved through parsing and graph construction.
        let yaml = r#"
ref: test.typed_publish
label: Typed Publish Test
version: 1.0.0
tasks:
  - name: task1
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
          - task2
      - when: "{{ failed() }}"
        publish:
          - validation_passed: false
        do:
          - task2
  - name: task2
    action: core.echo
"#;

        let workflow = workflow::parse_workflow_yaml(yaml).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();

        let task1 = graph.get_task("task1").unwrap();
        assert_eq!(task1.transitions.len(), 2);

        // Success transition should have 6 publish vars
        let success_publish = &task1.transitions[0].publish;
        assert_eq!(success_publish.len(), 6);

        // Build a lookup map for easier assertions
        let publish_map: HashMap<&str, &JsonValue> = success_publish
            .iter()
            .map(|p| (p.name.as_str(), &p.value))
            .collect();

        // Boolean true is preserved as a JSON boolean
        assert_eq!(publish_map["validation_passed"], &JsonValue::Bool(true));

        // Integer is preserved as a JSON number
        assert_eq!(publish_map["count"], &serde_json::json!(42));

        // Float is preserved as a JSON number
        assert_eq!(publish_map["ratio"], &serde_json::json!(3.15));

        // Plain string stays as a string
        assert_eq!(
            publish_map["label"],
            &JsonValue::String("hello".to_string())
        );

        // Template expression stays as a string (rendered later by context)
        assert_eq!(
            publish_map["template_val"],
            &JsonValue::String("{{ result().data }}".to_string())
        );

        // Null is preserved
        assert_eq!(publish_map["nothing"], &JsonValue::Null);

        // Failure transition should have boolean false
        let failure_publish = &task1.transitions[1].publish;
        assert_eq!(failure_publish.len(), 1);
        assert_eq!(failure_publish[0].name, "validation_passed");
        assert_eq!(failure_publish[0].value, JsonValue::Bool(false));
    }
}
