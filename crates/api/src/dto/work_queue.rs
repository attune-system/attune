//! Work queue DTOs for API requests and responses.

use std::{borrow::Cow, fmt};

use chrono::{DateTime, Utc};
use serde::{
    de::{self, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationError};

use attune_common::{
    models::{
        ActionReferenceVisibility, Id, WorkQueue, WorkQueueBatchMode, WorkQueueItem,
        WorkQueueItemStatus, WorkQueueUpdateStrategy,
    },
    queue_definition::{validate_work_queue_action_params, validate_work_queue_config},
    schema::RefValidator,
};

use crate::dto::common::deserialize_double_option;
use crate::dto::runtime::NullableStringPatch;

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateWorkQueueRequest {
    #[validate(custom(function = "validate_queue_ref_field"))]
    #[schema(example = "core.inbox")]
    pub r#ref: String,

    #[validate(custom(function = "validate_pack_ref_field"))]
    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Core Inbox")]
    pub label: String,

    #[schema(
        example = "Dispatches inbound work items to the core processor",
        nullable = true
    )]
    pub description: Option<String>,

    #[schema(example = true, default = true)]
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[schema(example = true, default = true)]
    #[serde(default = "default_true")]
    pub accepting_new_items: bool,

    #[validate(custom(function = "validate_action_ref_field"))]
    #[schema(example = "core.process_item")]
    pub dispatch_action_ref: String,

    #[schema(example = 0, default = 0)]
    #[serde(default)]
    pub default_priority: i32,

    #[schema(example = false, default = false)]
    #[serde(default)]
    pub allow_pending_update: bool,

    #[serde(default)]
    pub update_strategy: WorkQueueUpdateStrategy,

    #[serde(default)]
    pub batch_mode: WorkQueueBatchMode,

    #[validate(custom(function = "validate_item_schema_field"))]
    #[schema(value_type = Object, example = json!({"order_id": {"type": "integer", "required": true}}))]
    #[serde(default = "default_json_object")]
    pub item_schema: JsonValue,

    #[validate(custom(function = "validate_action_params_field"))]
    #[schema(value_type = Object, example = json!({"items": "{{ items }}"}))]
    #[serde(default = "default_json_object")]
    pub action_params: JsonValue,

    /// Permission set refs to apply to executions dispatched by this queue. Omit
    /// to inherit the dispatch action default. Provide an empty array to force no
    /// API token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Vec<String>>,

    #[validate(custom(function = "validate_queue_config_field"))]
    #[schema(value_type = Object, example = json!({"dispatch": {"concurrency": {"source": "literal", "value": 5}}}))]
    #[serde(default = "default_json_object")]
    pub config: JsonValue,

    /// Pack-level visibility for queue item submission/targeting. Omitted defaults to public.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "public", default = "public", nullable = true)]
    pub reference_visibility: Option<ActionReferenceVisibility>,

    /// Pack refs allowed to target this queue when visibility is restricted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["incident_response", "deployments"]), default = json!([]))]
    pub reference_allowed_pack_refs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdateWorkQueueRequest {
    #[validate(custom(function = "validate_pack_ref_patch"))]
    pub pack_ref: Option<NullableStringPatch>,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Core Inbox (Updated)")]
    pub label: Option<String>,

    pub description: Option<NullableStringPatch>,

    #[schema(example = false)]
    pub enabled: Option<bool>,

    #[schema(example = true)]
    pub accepting_new_items: Option<bool>,

    #[validate(custom(function = "validate_action_ref_field"))]
    #[schema(example = "core.process_item")]
    pub dispatch_action_ref: Option<String>,

    #[schema(example = 10)]
    pub default_priority: Option<i32>,

    #[schema(example = true)]
    pub allow_pending_update: Option<bool>,

    pub update_strategy: Option<WorkQueueUpdateStrategy>,

    pub batch_mode: Option<WorkQueueBatchMode>,

    #[validate(custom(function = "validate_item_schema_field"))]
    #[schema(value_type = Object, nullable = true)]
    pub item_schema: Option<JsonValue>,

    #[validate(custom(function = "validate_action_params_field"))]
    #[schema(value_type = Object, nullable = true)]
    pub action_params: Option<JsonValue>,

    /// Permission set refs to apply to executions dispatched by this queue. Omit
    /// to keep the current value. Provide null to inherit the dispatch action
    /// default, or an empty array to force no API token.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_double_option"
    )]
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Option<Vec<String>>>,

    #[validate(custom(function = "validate_queue_config_field"))]
    #[schema(value_type = Object, nullable = true)]
    pub config: Option<JsonValue>,

    /// Pack-level visibility for queue item submission/targeting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "restricted", nullable = true)]
    pub reference_visibility: Option<ActionReferenceVisibility>,

    /// Replace the restricted visibility allow-list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = json!(["incident_response", "deployments"]), nullable = true)]
    pub reference_allowed_pack_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ResolvedWorkQueueDispatchTuningResponse {
    #[schema(example = 5, nullable = true)]
    pub concurrency: Option<u32>,
    #[schema(example = 10, nullable = true)]
    pub batch_size: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkQueueResponse {
    #[schema(example = 1)]
    pub id: Id,
    #[schema(example = "core.inbox")]
    pub r#ref: String,
    #[schema(example = 1, nullable = true)]
    pub pack: Option<Id>,
    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,
    #[schema(example = false)]
    pub is_adhoc: bool,
    #[schema(example = "Core Inbox")]
    pub label: String,
    #[schema(
        example = "Dispatches inbound work items to the core processor",
        nullable = true
    )]
    pub description: Option<String>,
    #[schema(example = true)]
    pub enabled: bool,
    #[schema(example = true)]
    pub accepting_new_items: bool,
    #[schema(example = 42, nullable = true)]
    pub dispatch_action: Option<Id>,
    #[schema(example = "core.process_item")]
    pub dispatch_action_ref: String,
    #[schema(example = 0)]
    pub default_priority: i32,
    #[schema(example = false)]
    pub allow_pending_update: bool,
    pub update_strategy: WorkQueueUpdateStrategy,
    pub batch_mode: WorkQueueBatchMode,
    #[schema(value_type = Object)]
    pub item_schema: JsonValue,
    #[schema(value_type = Object)]
    pub action_params: JsonValue,
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Vec<String>>,
    #[schema(value_type = Object)]
    pub config: JsonValue,
    #[schema(example = "public")]
    pub reference_visibility: ActionReferenceVisibility,
    #[schema(example = json!(["incident_response", "deployments"]))]
    pub reference_allowed_pack_refs: Vec<String>,
    #[schema(nullable = true)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_dispatch_tuning: Option<ResolvedWorkQueueDispatchTuningResponse>,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkQueueSummary {
    #[schema(example = 1)]
    pub id: Id,
    #[schema(example = "core.inbox")]
    pub r#ref: String,
    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,
    #[schema(example = false)]
    pub is_adhoc: bool,
    #[schema(example = "Core Inbox")]
    pub label: String,
    #[schema(
        example = "Dispatches inbound work items to the core processor",
        nullable = true
    )]
    pub description: Option<String>,
    #[schema(example = true)]
    pub enabled: bool,
    #[schema(example = true)]
    pub accepting_new_items: bool,
    #[schema(example = "core.process_item")]
    pub dispatch_action_ref: String,
    #[schema(example = "public")]
    pub reference_visibility: ActionReferenceVisibility,
    #[schema(example = json!(["incident_response", "deployments"]))]
    pub reference_allowed_pack_refs: Vec<String>,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkQueueQueryParams {
    #[param(example = true)]
    pub enabled: Option<bool>,

    #[param(example = false)]
    pub is_adhoc: Option<bool>,

    #[param(example = "inbox")]
    pub search: Option<String>,

    /// Pack ref that intends to target/submit to this queue; used to reveal allowed restricted queues.
    #[param(example = "incident_response")]
    pub referencing_pack_ref: Option<String>,

    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    #[serde(default = "default_per_page")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub per_page: u32,
}

impl WorkQueueQueryParams {
    pub fn offset(&self) -> u32 {
        (self.page - 1) * self.per_page
    }

    pub fn limit(&self) -> u32 {
        self.per_page
    }
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct EnqueueWorkQueueItemRequest {
    #[validate(custom(function = "validate_item_key_field"))]
    #[schema(example = "order-123", nullable = true)]
    pub item_key: Option<String>,

    #[schema(example = 5, nullable = true)]
    pub priority: Option<i32>,

    #[schema(value_type = Object, example = json!({"order_id": 123, "customer": "alice"}))]
    pub payload: JsonValue,

    #[schema(value_type = Object, example = json!({"source": "api"}))]
    #[serde(default = "default_json_object")]
    pub metadata: JsonValue,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdateWorkQueueItemRequest {
    #[validate(custom(function = "validate_optional_item_key_patch"))]
    pub item_key: Option<NullableStringPatch>,

    #[schema(example = 10)]
    pub priority: Option<i32>,

    #[schema(value_type = Object, nullable = true, example = json!({"status": "retrying"}))]
    pub payload: Option<JsonValue>,

    #[schema(value_type = Object, nullable = true, example = json!({"attempt": 2}))]
    pub metadata: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkQueueItemResponse {
    #[schema(example = 1)]
    pub id: Id,
    #[schema(example = 10)]
    pub queue: Id,
    #[schema(example = "core.inbox")]
    pub queue_ref: String,
    #[schema(example = "order-123", nullable = true)]
    pub item_key: Option<String>,
    #[schema(example = 5)]
    pub priority: i32,
    pub status: WorkQueueItemStatus,
    #[schema(value_type = Object)]
    pub payload: JsonValue,
    #[schema(value_type = Object)]
    pub metadata: JsonValue,
    #[schema(example = "api")]
    pub enqueue_source: String,
    #[schema(example = 42, nullable = true)]
    pub requested_by_identity: Option<Id>,
    #[schema(example = 99, nullable = true)]
    pub requested_by_execution: Option<Id>,
    #[schema(example = 100, nullable = true)]
    pub requested_by_enforcement: Option<Id>,
    #[schema(example = 101, nullable = true)]
    pub leased_execution: Option<Id>,
    #[schema(nullable = true)]
    pub lease_token: Option<uuid::Uuid>,
    #[schema(example = "2024-01-13T10:30:00Z", nullable = true)]
    pub lease_expires_at: Option<DateTime<Utc>>,
    #[schema(example = 0)]
    pub attempt_count: i32,
    #[schema(value_type = Object, nullable = true)]
    pub last_error: Option<JsonValue>,
    #[schema(value_type = Object, nullable = true)]
    pub ack_summary: Option<JsonValue>,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkQueueItemQueryParams {
    #[param(example = "order-123")]
    pub item_key: Option<String>,

    #[param(example = "api")]
    pub enqueue_source: Option<String>,

    #[serde(default, deserialize_with = "deserialize_status_filters")]
    #[param(value_type = Vec<WorkQueueItemStatus>, example = json!(["queued", "retry"]))]
    pub statuses: Vec<WorkQueueItemStatus>,

    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    #[serde(default = "default_per_page")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub per_page: u32,
}

impl WorkQueueItemQueryParams {
    pub fn offset(&self) -> u32 {
        (self.page - 1) * self.per_page
    }

    pub fn limit(&self) -> u32 {
        self.per_page
    }
}

impl From<WorkQueue> for WorkQueueResponse {
    fn from(queue: WorkQueue) -> Self {
        Self {
            id: queue.id,
            r#ref: queue.r#ref,
            pack: queue.pack,
            pack_ref: queue.pack_ref,
            is_adhoc: queue.is_adhoc,
            label: queue.label,
            description: queue.description,
            enabled: queue.enabled,
            accepting_new_items: queue.accepting_new_items,
            dispatch_action: queue.dispatch_action,
            dispatch_action_ref: queue.dispatch_action_ref,
            default_priority: queue.default_priority,
            allow_pending_update: queue.allow_pending_update,
            update_strategy: queue.update_strategy,
            batch_mode: queue.batch_mode,
            item_schema: queue.item_schema,
            action_params: queue.action_params,
            permission_set_refs: queue.permission_set_refs,
            config: queue.config,
            reference_visibility: queue.reference_visibility,
            reference_allowed_pack_refs: queue.reference_allowed_pack_refs,
            resolved_dispatch_tuning: None,
            created: queue.created,
            updated: queue.updated,
        }
    }
}

impl WorkQueueResponse {
    pub fn from_with_resolved_tuning(
        queue: WorkQueue,
        resolved_dispatch_tuning: Option<ResolvedWorkQueueDispatchTuningResponse>,
    ) -> Self {
        let mut response = Self::from(queue);
        response.resolved_dispatch_tuning = resolved_dispatch_tuning;
        response
    }
}

impl From<WorkQueue> for WorkQueueSummary {
    fn from(queue: WorkQueue) -> Self {
        Self {
            id: queue.id,
            r#ref: queue.r#ref,
            pack_ref: queue.pack_ref,
            is_adhoc: queue.is_adhoc,
            label: queue.label,
            description: queue.description,
            enabled: queue.enabled,
            accepting_new_items: queue.accepting_new_items,
            dispatch_action_ref: queue.dispatch_action_ref,
            reference_visibility: queue.reference_visibility,
            reference_allowed_pack_refs: queue.reference_allowed_pack_refs,
            created: queue.created,
            updated: queue.updated,
        }
    }
}

impl From<WorkQueueItem> for WorkQueueItemResponse {
    fn from(item: WorkQueueItem) -> Self {
        Self {
            id: item.id,
            queue: item.queue,
            queue_ref: item.queue_ref,
            item_key: item.item_key,
            priority: item.priority,
            status: item.status,
            payload: item.payload,
            metadata: item.metadata,
            enqueue_source: item.enqueue_source,
            requested_by_identity: item.requested_by_identity,
            requested_by_execution: item.requested_by_execution,
            requested_by_enforcement: item.requested_by_enforcement,
            leased_execution: item.leased_execution,
            lease_token: item.lease_token,
            lease_expires_at: item.lease_expires_at,
            attempt_count: item.attempt_count,
            last_error: item.last_error,
            ack_summary: item.ack_summary,
            created: item.created,
            updated: item.updated,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_json_object() -> JsonValue {
    serde_json::json!({})
}

fn default_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    50
}

fn deserialize_status_filters<'de, D>(deserializer: D) -> Result<Vec<WorkQueueItemStatus>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StatusFilterVisitor;

    impl<'de> Visitor<'de> for StatusFilterVisitor {
        type Value = Vec<WorkQueueItemStatus>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(
                "a work queue item status, a comma-separated list, or repeated query parameters",
            )
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse_status_filters([value]).map_err(E::custom)
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse_status_filters([value]).map_err(E::custom)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element::<String>()? {
                values.push(value);
            }
            parse_status_filters(values).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(StatusFilterVisitor)
}

fn parse_status_filters<I, S>(values: I) -> std::result::Result<Vec<WorkQueueItemStatus>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut statuses = Vec::new();

    for value in values {
        for part in value.as_ref().split(',') {
            let status = match part.trim() {
                "" => continue,
                "queued" => WorkQueueItemStatus::Queued,
                "leased" => WorkQueueItemStatus::Leased,
                "retry" => WorkQueueItemStatus::Retry,
                "completed" => WorkQueueItemStatus::Completed,
                "failed" => WorkQueueItemStatus::Failed,
                other => {
                    return Err(format!(
                        "invalid work queue item status '{}' (expected one of queued, leased, retry, completed, failed)",
                        other
                    ));
                }
            };
            statuses.push(status);
        }
    }

    Ok(statuses)
}

fn validation_error(code: &'static str, message: String) -> ValidationError {
    let mut error = ValidationError::new(code);
    error.message = Some(Cow::Owned(message));
    error
}

fn validate_queue_ref_field(value: &str) -> Result<(), ValidationError> {
    RefValidator::validate_work_queue_ref(value)
        .map_err(|e| validation_error("queue_ref", e.to_string()))
}

fn validate_action_ref_field(value: &str) -> Result<(), ValidationError> {
    RefValidator::validate_component_ref(value)
        .map_err(|e| validation_error("dispatch_action_ref", e.to_string()))
}

fn validate_pack_ref_field(value: &str) -> Result<(), ValidationError> {
    RefValidator::validate_pack_ref(value).map_err(|e| validation_error("pack_ref", e.to_string()))
}

fn validate_pack_ref_patch(value: &NullableStringPatch) -> Result<(), ValidationError> {
    if let NullableStringPatch::Set(value) = value {
        RefValidator::validate_pack_ref(value)
            .map_err(|e| validation_error("pack_ref", e.to_string()))?;
    }
    Ok(())
}

fn validate_item_key_field(value: &str) -> Result<(), ValidationError> {
    validate_item_key(value)
}

fn validate_optional_item_key_patch(value: &NullableStringPatch) -> Result<(), ValidationError> {
    if let NullableStringPatch::Set(value) = value {
        validate_item_key(value)?;
    }
    Ok(())
}

fn validate_item_key(value: &str) -> Result<(), ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(validation_error(
            "item_key",
            "item_key cannot be empty".to_string(),
        ));
    }
    if trimmed.len() > 255 {
        return Err(validation_error(
            "item_key",
            "item_key must be at most 255 characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_queue_config_field(value: &JsonValue) -> Result<(), ValidationError> {
    validate_work_queue_config(value)
        .map(|_| ())
        .map_err(|e| validation_error("config", e.to_string()))
}

fn validate_item_schema_field(value: &JsonValue) -> Result<(), ValidationError> {
    attune_common::queue_definition::validate_work_queue_item_schema(value)
        .map(|_| ())
        .map_err(|e| validation_error("item_schema", e.to_string()))
}

fn validate_action_params_field(value: &JsonValue) -> Result<(), ValidationError> {
    validate_work_queue_action_params(value)
        .map(|_| ())
        .map_err(|e| validation_error("action_params", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{parse_status_filters, WorkQueueItemStatus};

    #[test]
    fn parse_status_filters_accepts_comma_separated_values() {
        let parsed = parse_status_filters(["queued,retry"]).expect("parse statuses");
        assert_eq!(
            parsed,
            vec![WorkQueueItemStatus::Queued, WorkQueueItemStatus::Retry]
        );
    }

    #[test]
    fn parse_status_filters_accepts_repeated_values() {
        let parsed = parse_status_filters(["queued", "retry"]).expect("parse statuses");
        assert_eq!(
            parsed,
            vec![WorkQueueItemStatus::Queued, WorkQueueItemStatus::Retry]
        );
    }
}
