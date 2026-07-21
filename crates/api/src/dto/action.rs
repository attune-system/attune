//! Action DTOs for API requests and responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use utoipa::ToSchema;
use validator::Validate;

use attune_common::models::enums::{ActionReferenceVisibility, RetentionPolicyType};
use attune_common::scheduling::{WorkerAffinity, WorkerToleration};

/// Request DTO for creating a new action
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateActionRequest {
    /// Unique reference identifier (e.g., "core.http", "aws.ec2.start_instance")
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "slack.post_message")]
    pub r#ref: String,

    /// Pack reference this action belongs to
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Post Message to Slack")]
    pub label: String,

    /// Action description
    #[schema(example = "Posts a message to a Slack channel")]
    pub description: Option<String>,

    /// Entry point for action execution (e.g., path to script, function name)
    #[validate(length(min = 1, max = 1024))]
    #[schema(example = "/actions/slack/post_message.py")]
    pub entrypoint: String,

    /// Optional runtime ID for this action
    #[schema(example = 1)]
    pub runtime: Option<i64>,

    /// Whether this action is enabled. Omitted defaults to true.
    #[schema(example = true, default = true, nullable = true)]
    pub enabled: Option<bool>,

    /// Optional runtime reference for this action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "core.python", nullable = true)]
    pub runtime_ref: Option<String>,

    /// Optional semver version constraint for the runtime (e.g., ">=3.12", ">=3.12,<4.0", "~18.0")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = ">=3.12", nullable = true)]
    pub runtime_version_constraint: Option<String>,

    /// Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object, example = json!({"node": "*", "python": ">=3.12"}), default = json!({}))]
    pub required_worker_runtimes: BTreeMap<String, String>,

    /// Exact worker label requirements. All labels must match the selected worker.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object, example = json!({"gpu": "nvidia", "zone": "us-east-1a"}), default = json!({}))]
    pub worker_selector: BTreeMap<String, String>,

    /// Tolerations that allow scheduling onto workers with matching taints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(default = json!([]))]
    pub worker_tolerations: Vec<WorkerToleration>,

    /// Required/preferred worker label affinity and required anti-affinity.
    #[serde(default)]
    pub worker_affinity: WorkerAffinity,

    /// Parameter schema (StackStorm-style) defining expected inputs with inline required/secret
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object, nullable = true, example = json!({"channel": {"type": "string", "description": "Slack channel", "required": true}, "message": {"type": "string", "description": "Message text", "required": true}}))]
    pub param_schema: Option<JsonValue>,

    /// Output schema (flat format) defining expected outputs with inline required/secret
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object, nullable = true, example = json!({"message_id": {"type": "string", "description": "ID of the sent message", "required": true}}))]
    pub out_schema: Option<JsonValue>,

    /// Hint that this action may invoke the Attune MCP server and spawn child executions.
    /// When true, consumers (UI, CLI, timeline charts) render subtask views eagerly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = false, default = false, nullable = true)]
    pub accesses_mcp: Option<bool>,

    /// Default permission set refs for execution-scoped API tokens.
    /// Empty or omitted means executions of this action receive no API token by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["core.agent_reader"]), default = json!([]))]
    pub default_execution_permission_set_refs: Vec<String>,

    /// Pack-level visibility for references from rules, workflows, and queues.
    /// Omitted defaults to public.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "public", default = "public", nullable = true)]
    pub reference_visibility: Option<ActionReferenceVisibility>,

    /// Pack refs allowed to reference this action when visibility is restricted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["incident_response", "deployments"]), default = json!([]))]
    pub reference_allowed_pack_refs: Vec<String>,

    /// Optional per-action retention policy override for non-log artifacts created by executions.
    #[schema(example = "versions", nullable = true)]
    pub artifact_retention_policy: Option<RetentionPolicyType>,

    /// Optional per-action retention limit override for non-log artifacts created by executions.
    #[validate(range(min = 1))]
    #[schema(example = 10, nullable = true)]
    pub artifact_retention_limit: Option<i32>,

    /// Optional per-action retention policy override for stdout/stderr execution log artifacts.
    #[schema(example = "versions", nullable = true)]
    pub log_retention_policy: Option<RetentionPolicyType>,

    /// Optional per-action retention limit override for stdout/stderr execution log artifacts.
    #[validate(range(min = 1))]
    #[schema(example = 4, nullable = true)]
    pub log_retention_limit: Option<i32>,

    /// Optional default execution timeout in seconds for executions of this action.
    /// When omitted, executions fall back to the app-level
    /// `default_execution_timeout_seconds`.
    #[validate(range(min = 1))]
    #[schema(example = 300, nullable = true)]
    pub timeout_seconds: Option<i32>,
}

/// Request DTO for updating an action
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdateActionRequest {
    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Post Message to Slack (Updated)")]
    pub label: Option<String>,

    /// Action description
    #[schema(example = "Posts a message to a Slack channel with enhanced features")]
    pub description: Option<String>,

    /// Entry point for action execution
    #[validate(length(min = 1, max = 1024))]
    #[schema(example = "/actions/slack/post_message_v2.py")]
    pub entrypoint: Option<String>,

    /// Runtime ID
    #[schema(example = 1)]
    pub runtime: Option<i64>,

    /// Whether this action is enabled.
    #[schema(example = true, nullable = true)]
    pub enabled: Option<bool>,

    /// Runtime reference
    #[schema(example = "core.python", nullable = true)]
    pub runtime_ref: Option<String>,

    /// Optional semver version constraint patch for the runtime.
    pub runtime_version_constraint: Option<RuntimeVersionConstraintPatch>,

    /// Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object, example = json!({"node": "*", "python": ">=3.12"}), nullable = true)]
    pub required_worker_runtimes: Option<BTreeMap<String, String>>,

    /// Exact worker label requirements. All labels must match the selected worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object, example = json!({"gpu": "nvidia"}), nullable = true)]
    pub worker_selector: Option<BTreeMap<String, String>>,

    /// Tolerations that allow scheduling onto workers with matching taints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = true)]
    pub worker_tolerations: Option<Vec<WorkerToleration>>,

    /// Required/preferred worker label affinity and required anti-affinity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = true)]
    pub worker_affinity: Option<WorkerAffinity>,

    /// Parameter schema (StackStorm-style with inline required/secret)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object, nullable = true)]
    pub param_schema: Option<JsonValue>,

    /// Output schema
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object, nullable = true)]
    pub out_schema: Option<JsonValue>,

    /// Hint that this action may invoke the Attune MCP server and spawn child executions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = false, nullable = true)]
    pub accesses_mcp: Option<bool>,

    /// Default permission set refs for execution-scoped API tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub default_execution_permission_set_refs: Option<Vec<String>>,

    /// Pack-level visibility for references from rules, workflows, and queues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "restricted", nullable = true)]
    pub reference_visibility: Option<ActionReferenceVisibility>,

    /// Replace the restricted visibility allow-list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = json!(["incident_response", "deployments"]), nullable = true)]
    pub reference_allowed_pack_refs: Option<Vec<String>>,

    /// Patch the per-action retention policy override for non-log artifacts created by executions.
    pub artifact_retention_policy: Option<LogRetentionPolicyPatch>,

    /// Patch the per-action retention limit override for non-log artifacts created by executions.
    pub artifact_retention_limit: Option<LogRetentionLimitPatch>,

    /// Patch the per-action retention policy override for stdout/stderr execution log artifacts.
    pub log_retention_policy: Option<LogRetentionPolicyPatch>,

    /// Patch the per-action retention limit override for stdout/stderr execution log artifacts.
    pub log_retention_limit: Option<LogRetentionLimitPatch>,

    /// Patch the per-action default execution timeout (seconds).
    pub timeout_seconds: Option<TimeoutSecondsPatch>,
}

/// Explicit patch operation for a nullable runtime version constraint.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum RuntimeVersionConstraintPatch {
    Set(String),
    Clear,
}

/// Explicit patch operation for a nullable log retention policy override.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum LogRetentionPolicyPatch {
    Set(RetentionPolicyType),
    Clear,
}

/// Explicit patch operation for a nullable log retention limit override.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum LogRetentionLimitPatch {
    Set(i32),
    Clear,
}

/// Explicit patch operation for a nullable action default timeout (seconds).
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum TimeoutSecondsPatch {
    Set(i32),
    Clear,
}

/// Response DTO for action information
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActionResponse {
    /// Action ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack.post_message")]
    pub r#ref: String,

    /// Pack ID
    #[schema(example = 1)]
    pub pack: i64,

    /// Pack reference
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[schema(example = "Post Message to Slack")]
    pub label: String,

    /// Action description
    #[schema(example = "Posts a message to a Slack channel")]
    pub description: Option<String>,

    /// Entry point
    #[schema(example = "/actions/slack/post_message.py")]
    pub entrypoint: String,

    /// Runtime ID
    #[schema(example = 1)]
    pub runtime: Option<i64>,

    /// Whether this action is enabled
    #[schema(example = true)]
    pub enabled: bool,

    /// Runtime reference (stable identifier, e.g., "core.python")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "core.python", nullable = true)]
    pub runtime_ref: Option<String>,

    /// Semver version constraint for the runtime (e.g., ">=3.12", ">=3.12,<4.0", "~18.0")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = ">=3.12", nullable = true)]
    pub runtime_version_constraint: Option<String>,

    /// Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object, example = json!({"node": "*", "python": ">=3.12"}))]
    pub required_worker_runtimes: BTreeMap<String, String>,

    /// Exact worker label requirements.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object, example = json!({"gpu": "nvidia"}))]
    pub worker_selector: BTreeMap<String, String>,

    /// Tolerations for worker taints.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub worker_tolerations: Vec<WorkerToleration>,

    /// Required/preferred worker label affinity and required anti-affinity.
    #[serde(skip_serializing_if = "WorkerAffinity::is_empty")]
    pub worker_affinity: WorkerAffinity,

    /// Parameter schema (StackStorm-style with inline required/secret)
    #[schema(value_type = Object, nullable = true)]
    pub param_schema: Option<JsonValue>,

    /// Output schema
    #[schema(value_type = Object, nullable = true)]
    pub out_schema: Option<JsonValue>,

    /// Workflow definition ID (non-null if this action is a workflow)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 42, nullable = true)]
    pub workflow_def: Option<i64>,

    /// Whether this is an ad-hoc action (not from pack installation)
    #[schema(example = false)]
    pub is_adhoc: bool,

    /// Hint that this action may invoke the Attune MCP server and spawn child executions.
    #[schema(example = false, default = false)]
    pub accesses_mcp: bool,

    /// Default permission set refs used when executions do not explicitly override token permissions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["core.agent_reader"]))]
    pub default_execution_permission_set_refs: Vec<String>,

    /// Pack-level visibility for references from rules, workflows, and queues.
    #[schema(example = "public")]
    pub reference_visibility: ActionReferenceVisibility,

    /// Pack refs allowed to reference this action when visibility is restricted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["incident_response", "deployments"]))]
    pub reference_allowed_pack_refs: Vec<String>,

    /// Per-action retention policy override for non-log artifacts created by executions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "versions", nullable = true)]
    pub artifact_retention_policy: Option<RetentionPolicyType>,

    /// Per-action retention limit override for non-log artifacts created by executions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 10, nullable = true)]
    pub artifact_retention_limit: Option<i32>,

    /// Per-action retention policy override for stdout/stderr execution log artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "versions", nullable = true)]
    pub log_retention_policy: Option<RetentionPolicyType>,

    /// Per-action retention limit override for stdout/stderr execution log artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 4, nullable = true)]
    pub log_retention_limit: Option<i32>,

    /// Default execution timeout (seconds) snapshotted onto executions of this action.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 300, nullable = true)]
    pub timeout_seconds: Option<i32>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Simplified action response (for list endpoints)
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActionSummary {
    /// Action ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack.post_message")]
    pub r#ref: String,

    /// Pack reference
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[schema(example = "Post Message to Slack")]
    pub label: String,

    /// Action description
    #[schema(example = "Posts a message to a Slack channel")]
    pub description: Option<String>,

    /// Entry point
    #[schema(example = "/actions/slack/post_message.py")]
    pub entrypoint: String,

    /// Runtime ID
    #[schema(example = 1)]
    pub runtime: Option<i64>,

    /// Whether this action is enabled
    #[schema(example = true)]
    pub enabled: bool,

    /// Runtime reference (stable identifier, e.g., "core.python")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "core.python", nullable = true)]
    pub runtime_ref: Option<String>,

    /// Semver version constraint for the runtime
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = ">=3.12", nullable = true)]
    pub runtime_version_constraint: Option<String>,

    /// Additional worker runtime requirements keyed by runtime name/alias. Use "*" for any available version.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object, example = json!({"node": "*", "python": ">=3.12"}))]
    pub required_worker_runtimes: BTreeMap<String, String>,

    /// Exact worker label requirements.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object, example = json!({"gpu": "nvidia"}))]
    pub worker_selector: BTreeMap<String, String>,

    /// Tolerations for worker taints.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub worker_tolerations: Vec<WorkerToleration>,

    /// Required/preferred worker label affinity and required anti-affinity.
    #[serde(skip_serializing_if = "WorkerAffinity::is_empty")]
    pub worker_affinity: WorkerAffinity,

    /// Workflow definition ID (non-null if this action is a workflow)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 42, nullable = true)]
    pub workflow_def: Option<i64>,

    /// Hint that this action may invoke the Attune MCP server and spawn child executions.
    #[schema(example = false, default = false)]
    pub accesses_mcp: bool,

    /// Default permission set refs used when executions do not explicitly override token permissions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["core.agent_reader"]))]
    pub default_execution_permission_set_refs: Vec<String>,

    /// Pack-level visibility for references from rules, workflows, and queues.
    #[schema(example = "public")]
    pub reference_visibility: ActionReferenceVisibility,

    /// Pack refs allowed to reference this action when visibility is restricted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["incident_response", "deployments"]))]
    pub reference_allowed_pack_refs: Vec<String>,

    /// Per-action retention policy override for non-log artifacts created by executions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "versions", nullable = true)]
    pub artifact_retention_policy: Option<RetentionPolicyType>,

    /// Per-action retention limit override for non-log artifacts created by executions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 10, nullable = true)]
    pub artifact_retention_limit: Option<i32>,

    /// Per-action retention policy override for stdout/stderr execution log artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "versions", nullable = true)]
    pub log_retention_policy: Option<RetentionPolicyType>,

    /// Per-action retention limit override for stdout/stderr execution log artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 4, nullable = true)]
    pub log_retention_limit: Option<i32>,

    /// Default execution timeout (seconds) snapshotted onto executions of this action.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 300, nullable = true)]
    pub timeout_seconds: Option<i32>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Convert from Action model to ActionResponse
impl From<attune_common::models::action::Action> for ActionResponse {
    fn from(action: attune_common::models::action::Action) -> Self {
        let required_worker_runtimes = action.required_worker_runtime_constraints();
        let worker_selector = action.worker_selector_labels();
        let worker_tolerations = action.worker_toleration_specs();
        let worker_affinity = action.worker_affinity_spec();
        Self {
            id: action.id,
            r#ref: action.r#ref,
            pack: action.pack,
            pack_ref: action.pack_ref,
            label: action.label,
            description: action.description,
            entrypoint: action.entrypoint,
            runtime: action.runtime,
            enabled: action.enabled,
            runtime_ref: None,
            runtime_version_constraint: action.runtime_version_constraint,
            required_worker_runtimes,
            worker_selector,
            worker_tolerations,
            worker_affinity,
            param_schema: action.param_schema,
            out_schema: action.out_schema,
            workflow_def: action.workflow_def,
            is_adhoc: action.is_adhoc,
            accesses_mcp: action.accesses_mcp,
            default_execution_permission_set_refs: action.default_execution_permission_set_refs,
            reference_visibility: action.reference_visibility,
            reference_allowed_pack_refs: action.reference_allowed_pack_refs,
            artifact_retention_policy: action.artifact_retention_policy,
            artifact_retention_limit: action.artifact_retention_limit,
            log_retention_policy: action.log_retention_policy,
            log_retention_limit: action.log_retention_limit,
            timeout_seconds: action.timeout_seconds,
            created: action.created,
            updated: action.updated,
        }
    }
}

/// Convert from Action model to ActionSummary
impl From<attune_common::models::action::Action> for ActionSummary {
    fn from(action: attune_common::models::action::Action) -> Self {
        let required_worker_runtimes = action.required_worker_runtime_constraints();
        let worker_selector = action.worker_selector_labels();
        let worker_tolerations = action.worker_toleration_specs();
        let worker_affinity = action.worker_affinity_spec();
        Self {
            id: action.id,
            r#ref: action.r#ref,
            pack_ref: action.pack_ref,
            label: action.label,
            description: action.description,
            entrypoint: action.entrypoint,
            runtime: action.runtime,
            enabled: action.enabled,
            runtime_ref: None,
            runtime_version_constraint: action.runtime_version_constraint,
            required_worker_runtimes,
            worker_selector,
            worker_tolerations,
            worker_affinity,
            workflow_def: action.workflow_def,
            accesses_mcp: action.accesses_mcp,
            default_execution_permission_set_refs: action.default_execution_permission_set_refs,
            reference_visibility: action.reference_visibility,
            reference_allowed_pack_refs: action.reference_allowed_pack_refs,
            artifact_retention_policy: action.artifact_retention_policy,
            artifact_retention_limit: action.artifact_retention_limit,
            log_retention_policy: action.log_retention_policy,
            log_retention_limit: action.log_retention_limit,
            timeout_seconds: action.timeout_seconds,
            created: action.created,
            updated: action.updated,
        }
    }
}

/// Lean search hit for action discovery — designed to minimize context bloat
/// for AI agents and humans browsing large action catalogs. Excludes ID,
/// timestamps, schemas, and runtime internals.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActionSearchHit {
    /// Action reference (globally unique identifier, e.g., "slack.post_message")
    #[schema(example = "slack.post_message")]
    pub r#ref: String,

    /// Pack reference
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[schema(example = "Post Message to Slack")]
    pub label: String,

    /// Action description
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "Posts a message to a Slack channel", nullable = true)]
    pub description: Option<String>,

    /// Runtime reference (e.g., "core.python"). None for workflow actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "core.python", nullable = true)]
    pub runtime_ref: Option<String>,

    /// True when this action is a workflow (orchestrates child executions)
    #[schema(example = false)]
    pub is_workflow: bool,

    /// Hint that this action may invoke the Attune MCP server and spawn child executions.
    #[schema(example = false)]
    pub accesses_mcp: bool,

    /// Pack-level visibility for references from rules, workflows, and queues.
    #[schema(example = "public")]
    pub reference_visibility: ActionReferenceVisibility,
}

/// Convert from Action model to ActionSearchHit (runtime_ref populated by handler)
impl From<attune_common::models::action::Action> for ActionSearchHit {
    fn from(action: attune_common::models::action::Action) -> Self {
        Self {
            r#ref: action.r#ref,
            pack_ref: action.pack_ref,
            label: action.label,
            description: action.description,
            runtime_ref: None,
            is_workflow: action.workflow_def.is_some(),
            accesses_mcp: action.accesses_mcp,
            reference_visibility: action.reference_visibility,
        }
    }
}

/// Query parameters for `GET /api/v1/actions/search`.
#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
pub struct ActionSearchParams {
    /// Keyword query. Whitespace-separated tokens are AND-matched against
    /// `ref`, `label`, `description`, and `pack_ref` (case-insensitive substring).
    #[param(example = "slack post message")]
    pub q: Option<String>,

    /// Restrict to one or more pack refs. Comma-separated (e.g., `core,slack,jira`)
    /// or repeated query params (e.g., `?packs=core&packs=slack`).
    #[param(example = "core,slack")]
    pub packs: Option<String>,

    /// Optional pack ref that wants to reference the returned actions.
    /// When set, restricted actions allow-listed for this pack are included.
    #[param(example = "deployments")]
    pub referencing_pack_ref: Option<String>,
}

/// Query parameters for `GET /api/v1/actions`.
#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
pub struct ActionListParams {
    /// Page number (1-based)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Number of items per page
    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    /// Keyword query. Whitespace-separated tokens are AND-matched against
    /// `ref`, `label`, `description`, and `pack_ref` (case-insensitive substring).
    #[param(example = "slack post message")]
    pub q: Option<String>,

    /// When true, only return actions the current token can execute and whose
    /// default execution permission sets can be delegated by the current token.
    #[serde(default)]
    #[param(example = true)]
    pub executable_with_current_access: bool,

    /// Optional pack ref that wants to reference the returned actions.
    #[param(example = "deployments")]
    pub referencing_pack_ref: Option<String>,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

impl ActionListParams {
    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)) * self.page_size
    }

    pub fn limit(&self) -> u32 {
        self.page_size.min(100)
    }
}

/// Response DTO for queue statistics
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QueueStatsResponse {
    /// Action ID
    #[schema(example = 1)]
    pub action_id: i64,

    /// Action reference
    #[schema(example = "slack.post_message")]
    pub action_ref: String,

    /// Number of executions waiting in queue
    #[schema(example = 5)]
    pub queue_length: i32,

    /// Number of currently running executions
    #[schema(example = 2)]
    pub active_count: i32,

    /// Maximum concurrent executions allowed
    #[schema(example = 3)]
    pub max_concurrent: i32,

    /// Timestamp of oldest queued execution (if any)
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub oldest_enqueued_at: Option<DateTime<Utc>>,

    /// Total executions enqueued since queue creation
    #[schema(example = 100)]
    pub total_enqueued: i64,

    /// Total executions completed since queue creation
    #[schema(example = 95)]
    pub total_completed: i64,

    /// Timestamp of last statistics update
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub last_updated: DateTime<Utc>,
}

/// Convert from QueueStats repository model to QueueStatsResponse
impl From<attune_common::repositories::queue_stats::QueueStats> for QueueStatsResponse {
    fn from(stats: attune_common::repositories::queue_stats::QueueStats) -> Self {
        Self {
            action_id: stats.action_id,
            action_ref: String::new(), // Will be populated by the handler
            queue_length: stats.queue_length,
            active_count: stats.active_count,
            max_concurrent: stats.max_concurrent,
            oldest_enqueued_at: stats.oldest_enqueued_at,
            total_enqueued: stats.total_enqueued,
            total_completed: stats.total_completed,
            last_updated: stats.last_updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_action_request_validation() {
        let req = CreateActionRequest {
            r#ref: "".to_string(), // Invalid: empty
            pack_ref: "test-pack".to_string(),
            label: "Test Action".to_string(),
            description: Some("Test description".to_string()),
            entrypoint: "/actions/test.py".to_string(),
            runtime: None,
            enabled: None,
            runtime_ref: None,
            runtime_version_constraint: None,
            required_worker_runtimes: BTreeMap::new(),
            worker_selector: BTreeMap::new(),
            worker_tolerations: Vec::new(),
            worker_affinity: WorkerAffinity::default(),
            param_schema: None,
            out_schema: None,
            accesses_mcp: None,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: None,
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        };

        assert!(req.validate().is_err());
    }

    #[test]
    fn test_create_action_request_valid() {
        let req = CreateActionRequest {
            r#ref: "test.action".to_string(),
            pack_ref: "test-pack".to_string(),
            label: "Test Action".to_string(),
            description: Some("Test description".to_string()),
            entrypoint: "/actions/test.py".to_string(),
            runtime: None,
            enabled: Some(true),
            runtime_ref: None,
            runtime_version_constraint: None,
            required_worker_runtimes: BTreeMap::new(),
            worker_selector: BTreeMap::new(),
            worker_tolerations: Vec::new(),
            worker_affinity: WorkerAffinity::default(),
            param_schema: None,
            out_schema: None,
            accesses_mcp: None,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: None,
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        };

        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_update_action_request_all_none() {
        let req = UpdateActionRequest {
            label: None,
            description: None,
            entrypoint: None,
            runtime: None,
            enabled: None,
            runtime_ref: None,
            runtime_version_constraint: None,
            required_worker_runtimes: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            param_schema: None,
            out_schema: None,
            accesses_mcp: None,
            default_execution_permission_set_refs: None,
            reference_visibility: None,
            reference_allowed_pack_refs: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        };

        // Should be valid even with all None values
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_timeout_seconds_patch_deserialization() {
        // Set variant carries a positive value.
        let set: TimeoutSecondsPatch = serde_json::from_str(r#"{"op":"set","value":120}"#).unwrap();
        assert!(matches!(set, TimeoutSecondsPatch::Set(120)));

        // Clear variant explicitly removes the action default.
        let clear: TimeoutSecondsPatch = serde_json::from_str(r#"{"op":"clear"}"#).unwrap();
        assert!(matches!(clear, TimeoutSecondsPatch::Clear));
    }
}
