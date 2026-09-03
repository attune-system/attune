//! Rule DTOs for API requests and responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::dto::common::deserialize_double_option;

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

/// Query parameters for `GET /api/v1/rules`.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct RuleListParams {
    /// Page number (1-based)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Number of items per page
    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    /// Keyword query. Tokens are AND-matched across ref, label, description,
    /// pack_ref, action_ref, and trigger_ref.
    #[param(example = "slack post message")]
    pub q: Option<String>,

    /// Optional pack ref filter
    #[param(example = "core")]
    pub pack_ref: Option<String>,

    /// Optional action ref filter
    #[param(example = "core.echo")]
    pub action_ref: Option<String>,

    /// Optional trigger ref filter
    #[param(example = "core.timer")]
    pub trigger_ref: Option<String>,

    /// Optional enabled-state filter
    #[param(example = true)]
    pub enabled: Option<bool>,
}

impl RuleListParams {
    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)) * self.page_size
    }

    pub fn limit(&self) -> u32 {
        self.page_size.min(100)
    }
}

/// Request DTO for creating a new rule
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateRuleRequest {
    /// Unique reference identifier (e.g., "mypack.notify_on_error")
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "slack.notify_on_error")]
    pub r#ref: String,

    /// Pack reference this rule belongs to
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Notify on Error")]
    pub label: String,

    /// Rule description
    #[schema(example = "Send Slack notification when an error occurs")]
    pub description: Option<String>,

    /// Action reference to execute when rule matches
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "slack.post_message")]
    pub action_ref: String,

    /// Trigger reference that activates this rule
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "system.error_event")]
    pub trigger_ref: String,

    /// Conditions for rule evaluation (JSON Logic or custom format)
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object, example = json!({"var": "event.severity", ">=": 3}))]
    pub conditions: JsonValue,

    /// Parameters to pass to the action when rule is triggered
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object, example = json!({"message": "hello, world"}))]
    pub action_params: JsonValue,

    /// Parameters for trigger configuration and event filtering
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object, example = json!({"severity": "high"}))]
    pub trigger_params: JsonValue,

    /// Required labels for the sensor worker that runs this rule's managed sensor.
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object)]
    pub sensor_worker_selector: JsonValue,

    /// Taints tolerated by the sensor worker for this rule.
    #[serde(default = "default_empty_array")]
    #[schema(value_type = Vec<Object>)]
    pub sensor_worker_tolerations: JsonValue,

    /// Required and preferred sensor-worker affinity for this rule.
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object)]
    pub sensor_worker_affinity: JsonValue,

    /// Optional template used to resolve execution trace tags for this rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "{{ event.trigger }}.{{ event.id }}", nullable = true)]
    pub trace_tag_template: Option<String>,

    /// Permission set refs to apply to executions created by this rule. Omit to
    /// inherit the action default. Provide an empty array to force no API token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Vec<String>>,

    /// Whether the rule is enabled
    #[serde(default = "default_true")]
    #[schema(example = true)]
    pub enabled: bool,
}

/// Request DTO for updating a rule
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdateRuleRequest {
    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Notify on Error (Updated)")]
    pub label: Option<String>,

    /// Rule description
    #[schema(example = "Enhanced error notification with filtering")]
    pub description: Option<String>,

    /// Action reference to execute when rule matches
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "slack.post_message")]
    pub action_ref: Option<String>,

    /// Trigger reference that activates this rule
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "system.error_event")]
    pub trigger_ref: Option<String>,

    /// Conditions for rule evaluation
    #[schema(value_type = Object, nullable = true)]
    pub conditions: Option<JsonValue>,

    /// Parameters to pass to the action when rule is triggered
    #[schema(value_type = Object, nullable = true)]
    pub action_params: Option<JsonValue>,

    /// Parameters for trigger configuration and event filtering
    #[schema(value_type = Object, nullable = true)]
    pub trigger_params: Option<JsonValue>,

    /// Replacement sensor-worker selector.
    #[schema(value_type = Object, nullable = true)]
    pub sensor_worker_selector: Option<JsonValue>,

    /// Replacement sensor-worker tolerations.
    #[schema(value_type = Vec<Object>, nullable = true)]
    pub sensor_worker_tolerations: Option<JsonValue>,

    /// Replacement sensor-worker affinity.
    #[schema(value_type = Object, nullable = true)]
    pub sensor_worker_affinity: Option<JsonValue>,

    /// Optional template used to resolve execution trace tags for this rule.
    /// Omit to keep current value. Provide null to clear.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_double_option"
    )]
    #[schema(example = "{{ event.trigger }}.{{ event.id }}", nullable = true)]
    pub trace_tag_template: Option<Option<String>>,

    /// Permission set refs to apply to executions created by this rule. Omit to
    /// keep the current value. Provide null to inherit the action default, or an
    /// empty array to force no API token.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_double_option"
    )]
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Option<Vec<String>>>,

    /// Whether the rule is enabled
    #[schema(example = false)]
    pub enabled: Option<bool>,
}

/// Response DTO for rule information
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RuleResponse {
    /// Rule ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack.notify_on_error")]
    pub r#ref: String,

    /// Pack ID
    #[schema(example = 1)]
    pub pack: i64,

    /// Pack reference
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[schema(example = "Notify on Error")]
    pub label: String,

    /// Rule description
    #[schema(example = "Send Slack notification when an error occurs")]
    pub description: Option<String>,

    /// Action ID (null if the referenced action has been deleted)
    #[schema(example = 1)]
    pub action: Option<i64>,

    /// Action reference
    #[schema(example = "slack.post_message")]
    pub action_ref: String,

    /// Trigger ID (null if the referenced trigger has been deleted)
    #[schema(example = 1)]
    pub trigger: Option<i64>,

    /// Trigger reference
    #[schema(example = "system.error_event")]
    pub trigger_ref: String,

    /// Conditions for rule evaluation
    #[schema(value_type = Object)]
    pub conditions: JsonValue,

    /// Parameters to pass to the action when rule is triggered
    #[schema(value_type = Object)]
    pub action_params: JsonValue,

    /// Parameters for trigger configuration and event filtering
    #[schema(value_type = Object)]
    pub trigger_params: JsonValue,

    #[schema(value_type = Object)]
    pub sensor_worker_selector: JsonValue,

    #[schema(value_type = Vec<Object>)]
    pub sensor_worker_tolerations: JsonValue,

    #[schema(value_type = Object)]
    pub sensor_worker_affinity: JsonValue,

    /// Optional template used to resolve execution trace tags for this rule.
    #[schema(example = "{{ event.trigger }}.{{ event.id }}", nullable = true)]
    pub trace_tag_template: Option<String>,

    /// Optional execution permission override. Null means inherit action default;
    /// empty array means force no execution API token.
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Vec<String>>,

    /// Whether the rule is enabled
    #[schema(example = true)]
    pub enabled: bool,

    /// Whether this is an ad-hoc rule (not from pack installation)
    #[schema(example = false)]
    pub is_adhoc: bool,

    /// Identity that registered the rule. NULL for system-loaded rules.
    #[schema(example = 1, nullable = true)]
    pub owner_identity: Option<i64>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Simplified rule response (for list endpoints)
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RuleSummary {
    /// Rule ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack.notify_on_error")]
    pub r#ref: String,

    /// Pack reference
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[schema(example = "Notify on Error")]
    pub label: String,

    /// Rule description
    #[schema(example = "Send Slack notification when an error occurs")]
    pub description: Option<String>,

    /// Action reference
    #[schema(example = "slack.post_message")]
    pub action_ref: String,

    /// Trigger reference
    #[schema(example = "system.error_event")]
    pub trigger_ref: String,

    /// Parameters to pass to the action when rule is triggered
    #[schema(value_type = Object)]
    pub action_params: JsonValue,

    /// Parameters for trigger configuration and event filtering
    #[schema(value_type = Object)]
    pub trigger_params: JsonValue,

    #[schema(value_type = Object)]
    pub sensor_worker_selector: JsonValue,

    #[schema(value_type = Vec<Object>)]
    pub sensor_worker_tolerations: JsonValue,

    #[schema(value_type = Object)]
    pub sensor_worker_affinity: JsonValue,

    /// Optional template used to resolve execution trace tags for this rule.
    #[schema(example = "{{ event.trigger }}.{{ event.id }}", nullable = true)]
    pub trace_tag_template: Option<String>,

    /// Optional execution permission override. Null means inherit action default;
    /// empty array means force no execution API token.
    #[schema(example = json!(["core.agent_reader"]), nullable = true)]
    pub permission_set_refs: Option<Vec<String>>,

    /// Whether the rule is enabled
    #[schema(example = true)]
    pub enabled: bool,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Convert from Rule model to RuleResponse
impl From<attune_common::models::rule::Rule> for RuleResponse {
    fn from(rule: attune_common::models::rule::Rule) -> Self {
        Self {
            id: rule.id,
            r#ref: rule.r#ref,
            pack: rule.pack,
            pack_ref: rule.pack_ref,
            label: rule.label,
            description: rule.description,
            action: rule.action,
            action_ref: rule.action_ref,
            trigger: rule.trigger,
            trigger_ref: rule.trigger_ref,
            conditions: rule.conditions,
            action_params: rule.action_params,
            trigger_params: rule.trigger_params,
            sensor_worker_selector: rule.sensor_worker_selector,
            sensor_worker_tolerations: rule.sensor_worker_tolerations,
            sensor_worker_affinity: rule.sensor_worker_affinity,
            trace_tag_template: rule.trace_tag_template,
            permission_set_refs: rule.permission_set_refs,
            enabled: rule.enabled,
            is_adhoc: rule.is_adhoc,
            owner_identity: rule.owner_identity,
            created: rule.created,
            updated: rule.updated,
        }
    }
}

/// Convert from Rule model to RuleSummary
impl From<attune_common::models::rule::Rule> for RuleSummary {
    fn from(rule: attune_common::models::rule::Rule) -> Self {
        Self {
            id: rule.id,
            r#ref: rule.r#ref,
            pack_ref: rule.pack_ref,
            label: rule.label,
            description: rule.description,
            action_ref: rule.action_ref,
            trigger_ref: rule.trigger_ref,
            action_params: rule.action_params,
            trigger_params: rule.trigger_params,
            sensor_worker_selector: rule.sensor_worker_selector,
            sensor_worker_tolerations: rule.sensor_worker_tolerations,
            sensor_worker_affinity: rule.sensor_worker_affinity,
            trace_tag_template: rule.trace_tag_template,
            permission_set_refs: rule.permission_set_refs,
            enabled: rule.enabled,
            created: rule.created,
            updated: rule.updated,
        }
    }
}

fn default_empty_object() -> JsonValue {
    serde_json::json!({})
}

fn default_empty_array() -> JsonValue {
    serde_json::json!([])
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_rule_request_defaults() {
        let json = r#"{
            "ref": "test-rule",
            "pack_ref": "test-pack",
            "label": "Test Rule",
            "description": "Test description",
            "action_ref": "test.action",
            "trigger_ref": "test.trigger"
        }"#;

        let req: CreateRuleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.r#ref, "test-rule");
        assert_eq!(req.label, "Test Rule");
        assert_eq!(req.action_ref, "test.action");
        assert_eq!(req.trigger_ref, "test.trigger");
        assert!(req.enabled);
        assert_eq!(req.conditions, serde_json::json!({}));
    }

    #[test]
    fn test_create_rule_request_validation() {
        let req = CreateRuleRequest {
            r#ref: "".to_string(), // Invalid: empty
            pack_ref: "test-pack".to_string(),
            label: "Test Rule".to_string(),
            description: Some("Test description".to_string()),
            action_ref: "test.action".to_string(),
            trigger_ref: "test.trigger".to_string(),
            conditions: default_empty_object(),
            action_params: default_empty_object(),
            trigger_params: default_empty_object(),
            sensor_worker_selector: default_empty_object(),
            sensor_worker_tolerations: default_empty_array(),
            sensor_worker_affinity: default_empty_object(),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
        };

        assert!(req.validate().is_err());
    }

    #[test]
    fn test_create_rule_request_valid() {
        let req = CreateRuleRequest {
            r#ref: "test.rule".to_string(),
            pack_ref: "test-pack".to_string(),
            label: "Test Rule".to_string(),
            description: Some("Test description".to_string()),
            action_ref: "test.action".to_string(),
            trigger_ref: "test.trigger".to_string(),
            conditions: serde_json::json!({
                "and": [
                    {"var": "event.status", "==": "error"},
                    {"var": "event.severity", ">": 3}
                ]
            }),
            action_params: default_empty_object(),
            trigger_params: default_empty_object(),
            sensor_worker_selector: default_empty_object(),
            sensor_worker_tolerations: default_empty_array(),
            sensor_worker_affinity: default_empty_object(),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
        };

        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_update_rule_request_all_none() {
        let req = UpdateRuleRequest {
            label: None,
            description: None,
            action_ref: None,
            trigger_ref: None,
            conditions: None,
            action_params: None,
            trigger_params: None,
            sensor_worker_selector: None,
            sensor_worker_tolerations: None,
            sensor_worker_affinity: None,
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: None,
        };

        // Should be valid even with all None values
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_update_rule_request_partial() {
        let req = UpdateRuleRequest {
            label: Some("Updated Rule".to_string()),
            description: None,
            action_ref: Some("test.action.updated".to_string()),
            trigger_ref: Some("test.trigger.updated".to_string()),
            conditions: Some(serde_json::json!({"var": "status", "==": "ok"})),
            action_params: None,
            trigger_params: None,
            sensor_worker_selector: None,
            sensor_worker_tolerations: None,
            sensor_worker_affinity: None,
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: Some(false),
        };

        assert!(req.validate().is_ok());
    }
}
