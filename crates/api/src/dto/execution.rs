//! Execution DTOs for API requests and responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};

use attune_common::models::enums::ExecutionStatus;
use attune_common::models::enums::RetentionPolicyType;
use attune_common::models::enums::{
    WorkflowCacheIterationState, WorkflowTaskWaitKind, WorkflowTaskWaitState,
};
use attune_common::models::execution::WorkflowTaskMetadata;
use attune_common::models::{WorkflowCacheIteration, WorkflowTaskWait, WorkflowTaskWaitTarget};
use attune_common::repositories::execution::ExecutionWithRefs;

const MAX_WORKFLOW_CACHE_ITERATION_ERROR_SUMMARY_CHARS: usize = 1024;

/// Request DTO for creating a manual execution
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateExecutionRequest {
    /// Action reference to execute
    #[schema(example = "slack.post_message")]
    pub action_ref: String,

    /// Execution parameters/configuration
    #[serde(default)]
    #[schema(value_type = Option<Object>, example = json!({"channel": "#alerts", "message": "Manual test"}))]
    pub parameters: Option<JsonValue>,

    /// Environment overrides for this execution. ATTUNE_ names are reserved for internal use.
    #[serde(default)]
    #[schema(example = json!({"DEBUG": "true", "LOG_LEVEL": "info"}))]
    pub env_vars: Option<HashMap<String, String>>,

    /// Permission set refs to apply to this execution's API token. Omit to use
    /// the action default. Provide an empty array to force no API token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Vec<String>>,

    /// Retention policy override for non-log artifacts created by this execution.
    /// Omit to inherit the action default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "versions", nullable = true)]
    pub artifact_retention_policy: Option<RetentionPolicyType>,

    /// Retention limit override for non-log artifacts created by this execution.
    /// Omit to inherit the action default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = 10, nullable = true)]
    pub artifact_retention_limit: Option<i32>,

    /// Worker label selector override. Omit to inherit the action default;
    /// provide `{}` to explicitly clear selector requirements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>, example = json!({"pool": "gpu"}), nullable = true)]
    pub worker_selector: Option<JsonValue>,

    /// Worker taint tolerations override. Omit to inherit the action default;
    /// provide `[]` to explicitly clear tolerations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<Object>>, example = json!([{"key": "dedicated", "operator": "equal", "value": "gpu", "effect": "no_schedule"}]), nullable = true)]
    pub worker_tolerations: Option<JsonValue>,

    /// Worker affinity override. Omit to inherit the action default; provide
    /// `{}` to explicitly clear affinity requirements/preferences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>, nullable = true)]
    pub worker_affinity: Option<JsonValue>,

    /// Execution timeout override in seconds. Omit to inherit the action default
    /// (or the app-level `default_execution_timeout_seconds` when the action has
    /// no default). Must be a positive integer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = 300, nullable = true)]
    pub timeout_seconds: Option<i32>,
}

impl CreateExecutionRequest {
    pub fn validate_environment(&self) -> attune_common::Result<()> {
        if let Some(vars) = &self.env_vars {
            let mut entries = vars.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            for (key, value) in entries {
                attune_common::execution_env::validate_execution_env_var(key, value)?;
            }
        }
        Ok(())
    }
}

/// Response DTO for execution information
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExecutionResponse {
    /// Execution ID
    #[schema(example = 1)]
    pub id: i64,

    /// Action ID (optional, may be null for ad-hoc executions)
    #[schema(example = 1)]
    pub action: Option<i64>,

    /// Action reference
    #[schema(example = "slack.post_message")]
    pub action_ref: String,

    /// Execution configuration/parameters
    #[schema(value_type = Object, example = json!({"channel": "#alerts", "message": "System error detected"}))]
    pub config: Option<JsonValue>,

    /// Parent execution ID (for nested/child executions)
    #[schema(example = 1)]
    pub parent: Option<i64>,

    /// Enforcement ID (rule enforcement that triggered this)
    #[schema(example = 1)]
    pub enforcement: Option<i64>,

    /// Identity ID that initiated this execution
    #[schema(example = 1)]
    pub executor: Option<i64>,

    /// Permission set refs embedded in the execution-scoped API token.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["core.agent_reader"]))]
    pub permission_set_refs: Vec<String>,

    /// Retention policy override for non-log artifacts created by this execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "versions", nullable = true)]
    pub artifact_retention_policy: Option<RetentionPolicyType>,

    /// Retention limit override for non-log artifacts created by this execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 10, nullable = true)]
    pub artifact_retention_limit: Option<i32>,

    /// Worker selector override stored on the execution, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>, nullable = true)]
    pub worker_selector: Option<JsonValue>,

    /// Worker tolerations override stored on the execution, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<Object>>, nullable = true)]
    pub worker_tolerations: Option<JsonValue>,

    /// Worker affinity override stored on the execution, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>, nullable = true)]
    pub worker_affinity: Option<JsonValue>,

    /// Worker ID currently assigned to this execution
    #[schema(example = 1)]
    pub worker: Option<i64>,

    /// Execution status
    #[schema(example = "succeeded")]
    pub status: ExecutionStatus,

    /// System-wide trace tag for correlating related automatic activity.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "core.timer.1234", nullable = true)]
    pub trace_tag: Option<String>,

    /// Resolved execution timeout in seconds, snapshotted at creation time.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 600, nullable = true)]
    pub timeout_seconds: Option<i32>,

    /// Execution result/output
    #[schema(value_type = Object, example = json!({"message_id": "1234567890.123456"}))]
    pub result: Option<JsonValue>,

    /// ID of the original execution if this execution is a retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 1, nullable = true)]
    pub original_execution: Option<i64>,

    /// When the execution actually started running (worker picked it up).
    /// Null if the execution hasn't started running yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "2024-01-13T10:31:00Z", nullable = true)]
    pub started_at: Option<DateTime<Utc>>,

    /// Workflow task metadata (only populated for workflow task executions)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>, nullable = true)]
    pub workflow_task: Option<WorkflowTaskMetadata>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:35:00Z")]
    pub updated: DateTime<Utc>,
}

/// Response DTO for manual execution reschedule requests.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExecutionRescheduleResponse {
    /// Human-readable status of the republish request.
    #[schema(example = "Execution request republished; pending scheduling")]
    pub message: String,

    /// Number of reschedule attempts recorded for this execution.
    #[schema(example = 1)]
    pub attempt_count: i32,

    /// Timestamp for the recorded reschedule attempt.
    #[schema(example = "2024-01-13T10:35:00Z")]
    pub last_attempt_at: DateTime<Utc>,

    /// Current execution row after republish.
    pub execution: ExecutionResponse,
}

/// Safe operational status for one workflow cache iteration.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowCacheIterationResponse {
    pub task_name: String,
    pub namespace_id: i64,
    pub generation_id: i64,
    pub state: WorkflowCacheIterationState,
    pub scanned_count: i64,
    pub dispatched_count: i64,
    pub page_size: i32,
    pub batch_size: i32,
    pub concurrency: i32,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    #[schema(max_length = 1024)]
    pub error_summary: Option<String>,
}

impl From<WorkflowCacheIteration> for WorkflowCacheIterationResponse {
    fn from(iteration: WorkflowCacheIteration) -> Self {
        Self {
            task_name: iteration.task_name,
            namespace_id: iteration.namespace,
            generation_id: iteration.generation,
            state: iteration.state,
            scanned_count: iteration.scanned_count,
            dispatched_count: iteration.dispatched_count,
            page_size: iteration.page_size,
            batch_size: iteration.batch_size,
            concurrency: iteration.concurrency,
            created: iteration.created,
            updated: iteration.updated,
            completed_at: iteration.completed_at,
            error_summary: iteration.error_summary.map(|summary| {
                summary
                    .chars()
                    .take(MAX_WORKFLOW_CACHE_ITERATION_ERROR_SUMMARY_CHARS)
                    .collect()
            }),
        }
    }
}

/// Safe operational metadata for one workflow task wait.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowTaskWaitResponse {
    pub id: i64,
    pub task_name: String,
    pub kind: WorkflowTaskWaitKind,
    pub state: WorkflowTaskWaitState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_queue_ref: Option<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl WorkflowTaskWaitResponse {
    pub fn try_from_wait(
        wait: WorkflowTaskWait,
        target_visible: bool,
        work_queue_ref: Option<String>,
    ) -> attune_common::Result<Self> {
        let target = wait.target()?;
        let state = if wait.state == WorkflowTaskWaitState::Failed
            && wait
                .result
                .as_ref()
                .and_then(|result| result.get("status"))
                .and_then(serde_json::Value::as_str)
                == Some("cancelled")
        {
            WorkflowTaskWaitState::Cancelled
        } else {
            wait.state
        };
        Ok(Self {
            id: wait.id,
            task_name: wait.task_name,
            kind: target.kind(),
            state,
            target_id: target_visible.then(|| target.id()),
            work_queue_ref: match target {
                WorkflowTaskWaitTarget::WorkQueueItem(_) if target_visible => work_queue_ref,
                WorkflowTaskWaitTarget::Inquiry(_) | WorkflowTaskWaitTarget::Execution(_) => None,
                WorkflowTaskWaitTarget::WorkQueueItem(_) => None,
            },
            created: wait.created,
            updated: wait.updated,
            resolved_at: wait.resolved_at,
        })
    }
}

/// Simplified execution response (for list endpoints)
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExecutionSummary {
    /// Execution ID
    #[schema(example = 1)]
    pub id: i64,

    /// Action reference
    #[schema(example = "slack.post_message")]
    pub action_ref: String,

    /// Execution status
    #[schema(example = "succeeded")]
    pub status: ExecutionStatus,

    /// System-wide trace tag for correlating related automatic activity.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "core.timer.1234", nullable = true)]
    pub trace_tag: Option<String>,

    /// Resolved execution timeout in seconds, snapshotted at creation time.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 600, nullable = true)]
    pub timeout_seconds: Option<i32>,

    /// Parent execution ID
    #[schema(example = 1)]
    pub parent: Option<i64>,

    /// Enforcement ID
    #[schema(example = 1)]
    pub enforcement: Option<i64>,

    /// Rule reference (if triggered by a rule)
    #[schema(example = "core.on_timer")]
    pub rule_ref: Option<String>,

    /// Trigger reference (if triggered by a trigger)
    #[schema(example = "core.timer")]
    pub trigger_ref: Option<String>,

    /// When the execution actually started running (worker picked it up).
    /// Null if the execution hasn't started running yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "2024-01-13T10:31:00Z", nullable = true)]
    pub started_at: Option<DateTime<Utc>>,

    /// Workflow task metadata (only populated for workflow task executions)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>, nullable = true)]
    pub workflow_task: Option<WorkflowTaskMetadata>,

    /// ID of the original execution if this execution is a retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 1, nullable = true)]
    pub original_execution: Option<i64>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:35:00Z")]
    pub updated: DateTime<Utc>,
}

/// Query parameters for filtering executions
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct ExecutionQueryParams {
    /// Filter by execution status
    #[param(example = "succeeded")]
    pub status: Option<ExecutionStatus>,

    /// Filter by action reference.
    /// Supports exact refs and `<pack>.*` wildcards such as `core.*`.
    #[param(example = "slack.post_message")]
    pub action_ref: Option<String>,

    /// Filter by pack name
    #[param(example = "core")]
    pub pack_name: Option<String>,

    /// Filter by rule reference.
    /// Supports exact refs and `<pack>.*` wildcards such as `core.*`.
    #[param(example = "core.on_timer")]
    pub rule_ref: Option<String>,

    /// Filter by trigger reference.
    /// Supports exact refs and `<pack>.*` wildcards such as `core.*`.
    #[param(example = "core.timer")]
    pub trigger_ref: Option<String>,

    /// Filter by exact trace tag.
    #[param(example = "core.timer.1234")]
    pub trace_tag: Option<String>,

    /// Filter by executor ID
    #[param(example = 1)]
    pub executor: Option<i64>,

    /// Search in result JSON (case-insensitive substring match)
    #[param(example = "error")]
    pub result_contains: Option<String>,

    /// Filter by enforcement ID
    #[param(example = 1)]
    pub enforcement: Option<i64>,

    /// Filter by parent execution ID
    #[param(example = 1)]
    pub parent: Option<i64>,

    /// If true, only return top-level executions (those without a parent).
    /// Useful for the "By Workflow" view where child tasks are loaded separately.
    #[serde(default)]
    #[param(example = false)]
    pub top_level_only: Option<bool>,

    /// If true, include exact total counts in pagination metadata.
    /// Defaults to false for the main executions list to avoid expensive count queries.
    #[serde(default)]
    #[param(example = false)]
    pub include_total: Option<bool>,

    /// Page number (for pagination)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Items per page (for pagination)
    #[serde(default = "default_per_page")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub per_page: u32,
}

/// Query parameters for fetching one execution.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct ExecutionDetailQueryParams {
    /// Include decrypted secret parameter/result values. Requires executions:decrypt.
    #[serde(default)]
    #[param(example = false)]
    pub include_secret_values: bool,
}

impl ExecutionQueryParams {
    /// Get the SQL offset value
    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)) * self.per_page
    }

    /// Get the limit value (with max cap)
    pub fn limit(&self) -> u32 {
        self.per_page.min(100)
    }
}

/// Convert from Execution model to ExecutionResponse
impl From<attune_common::models::execution::Execution> for ExecutionResponse {
    fn from(execution: attune_common::models::execution::Execution) -> Self {
        Self {
            id: execution.id,
            action: execution.action,
            action_ref: execution.action_ref,
            config: execution
                .config
                .map(|c| serde_json::to_value(c).unwrap_or(JsonValue::Null)),
            parent: execution.parent,
            enforcement: execution.enforcement,
            executor: execution.executor,
            permission_set_refs: execution.permission_set_refs,
            artifact_retention_policy: execution.artifact_retention_policy,
            artifact_retention_limit: execution.artifact_retention_limit,
            worker_selector: execution.worker_selector,
            worker_tolerations: execution.worker_tolerations,
            worker_affinity: execution.worker_affinity,
            worker: execution.worker,
            status: execution.status,
            trace_tag: execution.trace_tag,
            timeout_seconds: execution.timeout_seconds,
            result: execution
                .result
                .map(|r| serde_json::to_value(r).unwrap_or(JsonValue::Null)),
            original_execution: execution.original_execution,
            started_at: execution.started_at,
            workflow_task: execution.workflow_task,
            created: execution.created,
            updated: execution.updated,
        }
    }
}

/// Convert from Execution model to ExecutionSummary
impl From<attune_common::models::execution::Execution> for ExecutionSummary {
    fn from(execution: attune_common::models::execution::Execution) -> Self {
        Self {
            id: execution.id,
            action_ref: execution.action_ref,
            status: execution.status,
            trace_tag: execution.trace_tag,
            timeout_seconds: execution.timeout_seconds,
            parent: execution.parent,
            enforcement: execution.enforcement,
            rule_ref: None,    // Populated separately via enforcement lookup
            trigger_ref: None, // Populated separately via enforcement lookup
            started_at: execution.started_at,
            workflow_task: execution.workflow_task,
            original_execution: execution.original_execution,
            created: execution.created,
            updated: execution.updated,
        }
    }
}

/// Convert from the joined query result (execution + enforcement refs).
/// `rule_ref` and `trigger_ref` are already populated from the SQL JOIN.
impl From<ExecutionWithRefs> for ExecutionSummary {
    fn from(row: ExecutionWithRefs) -> Self {
        Self {
            id: row.id,
            action_ref: row.action_ref,
            status: row.status,
            trace_tag: row.trace_tag,
            timeout_seconds: row.timeout_seconds,
            parent: row.parent,
            enforcement: row.enforcement,
            rule_ref: row.rule_ref,
            trigger_ref: row.trigger_ref,
            started_at: row.started_at,
            workflow_task: row.workflow_task,
            original_execution: row.original_execution,
            created: row.created,
            updated: row.updated,
        }
    }
}

fn default_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_environment_validation_is_explicit_and_does_not_echo_values() {
        let request: CreateExecutionRequest = serde_json::from_value(serde_json::json!({
            "action_ref":"core.echo", "env_vars":{"ATTUNE_API_TOKEN":"hidden"}
        }))
        .unwrap();
        let error = request.validate_environment().unwrap_err().to_string();
        assert!(error.contains("ATTUNE_API_TOKEN"));
        assert!(error.contains("reserved ATTUNE_ prefix"));
        assert!(!error.contains("hidden"));
        let invalid = serde_json::from_value::<CreateExecutionRequest>(serde_json::json!({
            "action_ref":"core.echo", "env_vars":{"COUNT":3}
        }));
        assert!(invalid.is_err());
        let valid: CreateExecutionRequest = serde_json::from_value(serde_json::json!({
            "action_ref":"core.echo", "env_vars":{"LOG_LEVEL":"debug", "EMPTY":""}
        }))
        .unwrap();
        valid.validate_environment().unwrap();
    }

    #[test]
    fn unsupported_component_environment_fields_are_not_silently_discarded() {
        fn assert_rejected<T: serde::de::DeserializeOwned>() {
            let body = serde_json::json!({"env_vars":{"ATTUNE_API_TOKEN":"hidden"}});
            let error = match serde_json::from_value::<T>(body) {
                Ok(_) => panic!("unsupported env_vars field was accepted"),
                Err(error) => error.to_string(),
            };
            assert!(error.contains("unknown field `env_vars`"), "{error}");
            assert!(!error.contains("hidden"));
        }
        assert_rejected::<crate::dto::action::UpdateActionRequest>();
        assert_rejected::<crate::dto::rule::UpdateRuleRequest>();
        assert_rejected::<crate::dto::work_queue::UpdateWorkQueueRequest>();
    }

    #[test]
    fn test_query_params_defaults() {
        let json = r#"{}"#;
        let params: ExecutionQueryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.page, 1);
        assert_eq!(params.per_page, 20);
        assert!(params.status.is_none());
    }

    #[test]
    fn test_query_params_with_filters() {
        let json = r#"{
            "status": "completed",
            "action_ref": "test.action",
            "page": 2,
            "per_page": 50
        }"#;
        let params: ExecutionQueryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.page, 2);
        assert_eq!(params.per_page, 50);
        assert_eq!(params.status, Some(ExecutionStatus::Completed));
        assert_eq!(params.action_ref, Some("test.action".to_string()));
    }

    #[test]
    fn test_query_params_offset() {
        let params = ExecutionQueryParams {
            status: None,
            action_ref: None,
            enforcement: None,
            parent: None,
            top_level_only: None,
            include_total: None,
            pack_name: None,
            rule_ref: None,
            trigger_ref: None,
            trace_tag: None,
            executor: None,
            result_contains: None,
            page: 3,
            per_page: 20,
        };
        assert_eq!(params.offset(), 40); // (3-1) * 20
    }

    #[test]
    fn test_query_params_limit_cap() {
        let params = ExecutionQueryParams {
            status: None,
            action_ref: None,
            enforcement: None,
            parent: None,
            top_level_only: None,
            include_total: None,
            pack_name: None,
            rule_ref: None,
            trigger_ref: None,
            trace_tag: None,
            executor: None,
            result_contains: None,
            page: 1,
            per_page: 200, // Exceeds max
        };
        assert_eq!(params.limit(), 100); // Capped at 100
    }

    #[test]
    fn workflow_cache_iteration_response_is_bounded_and_omits_cursor_fields() {
        let now = Utc::now();
        let response = WorkflowCacheIterationResponse::from(WorkflowCacheIteration {
            id: 1,
            workflow_execution: 2,
            task_name: "iterate".to_string(),
            namespace: 3,
            generation: 4,
            state: WorkflowCacheIterationState::Failed,
            last_external_id: Some("secret-cursor".to_string()),
            next_batch_index: 7,
            scanned_count: 80,
            dispatched_count: 60,
            page_size: 100,
            batch_size: 10,
            concurrency: 4,
            completed_at: Some(now),
            error_summary: Some("x".repeat(2048)),
            created: now,
            updated: now,
        });

        assert_eq!(
            response.error_summary.as_deref().unwrap().chars().count(),
            1024
        );
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["namespace_id"], 3);
        assert_eq!(json["generation_id"], 4);
        assert!(json.get("last_external_id").is_none());
        assert!(json.get("next_batch_index").is_none());
        assert!(json.get("workflow_execution").is_none());
        assert!(json.get("id").is_none());
    }

    #[test]
    fn workflow_task_wait_response_contains_only_safe_metadata_for_all_targets() {
        let now = Utc::now();
        let cases = [
            (WorkflowTaskWaitKind::Inquiry, Some(3), None, None, None),
            (WorkflowTaskWaitKind::Execution, None, Some(4), None, None),
            (
                WorkflowTaskWaitKind::WorkQueueItem,
                None,
                None,
                Some(5),
                Some("core.inbox".to_string()),
            ),
        ];

        for (kind, inquiry, target_execution, work_queue_item, queue_ref) in cases {
            let response = WorkflowTaskWaitResponse::try_from_wait(
                WorkflowTaskWait {
                    id: 1,
                    workflow_execution: 2,
                    task_name: "wait".to_string(),
                    kind,
                    state: WorkflowTaskWaitState::Released,
                    inquiry,
                    target_execution,
                    work_queue_item,
                    result: Some(serde_json::json!({"private": true})),
                    resolved_at: Some(now),
                    released_at: Some(now),
                    created: now,
                    updated: now,
                },
                true,
                queue_ref.clone(),
            )
            .unwrap();

            let json = serde_json::to_value(response).unwrap();
            assert_eq!(json["kind"], serde_json::to_value(kind).unwrap());
            assert_eq!(
                json["target_id"],
                inquiry.or(target_execution).or(work_queue_item).unwrap()
            );
            assert_eq!(
                json.get("work_queue_ref").and_then(|value| value.as_str()),
                queue_ref.as_deref()
            );
            assert!(json.get("result").is_none());
            assert!(json.get("released_at").is_none());
            assert!(json.get("inquiry_id").is_none());
        }

        let cancelled = WorkflowTaskWaitResponse::try_from_wait(
            WorkflowTaskWait {
                id: 2,
                workflow_execution: 2,
                task_name: "cancelled_target".to_string(),
                kind: WorkflowTaskWaitKind::Execution,
                state: WorkflowTaskWaitState::Failed,
                inquiry: None,
                target_execution: Some(98),
                work_queue_item: None,
                result: Some(serde_json::json!({"status": "cancelled", "private": true})),
                resolved_at: Some(now),
                released_at: None,
                created: now,
                updated: now,
            },
            true,
            None,
        )
        .unwrap();
        let cancelled_json = serde_json::to_value(cancelled).unwrap();
        assert_eq!(cancelled_json["state"], "cancelled");
        assert!(cancelled_json.get("result").is_none());

        let hidden = WorkflowTaskWaitResponse::try_from_wait(
            WorkflowTaskWait {
                id: 2,
                workflow_execution: 2,
                task_name: "private_target".to_string(),
                kind: WorkflowTaskWaitKind::Execution,
                state: WorkflowTaskWaitState::Waiting,
                inquiry: None,
                target_execution: Some(99),
                work_queue_item: None,
                result: None,
                resolved_at: None,
                released_at: None,
                created: now,
                updated: now,
            },
            false,
            None,
        )
        .unwrap();
        assert!(serde_json::to_value(hidden)
            .unwrap()
            .get("target_id")
            .is_none());
    }
}
