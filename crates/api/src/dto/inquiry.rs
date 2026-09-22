//! Inquiry data transfer objects

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use attune_common::models::{
    enums::InquiryStatus,
    inquiry::{Inquiry, InquiryResponseOption, InquiryResponseOptionStyle},
    Id, JsonDict, JsonSchema,
};
use attune_common::secret_values::redact_secret_parameters;
use serde_json::Value as JsonValue;

/// Full inquiry response with all details
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InquiryResponse {
    /// Inquiry ID
    #[schema(example = 1)]
    pub id: Id,

    /// Execution ID that created this inquiry
    #[schema(example = 1)]
    pub created_by_execution: Id,

    pub created_by_action_ref: Option<String>,

    pub created_by_pack_ref: Option<String>,

    pub workflow_execution: Option<Id>,

    /// Root execution ID for the containing workflow
    pub workflow_root_execution: Option<Id>,

    pub workflow_action_ref: Option<String>,

    pub workflow_pack_ref: Option<String>,

    pub workflow_task_name: Option<String>,

    pub purpose: Option<String>,

    /// Prompt text displayed to the user
    #[schema(example = "Approve deployment to production?")]
    pub prompt: String,

    /// Attune flat schema for expected response fields
    #[schema(value_type = Object, nullable = true)]
    pub response_schema: Option<JsonSchema>,

    /// Fixed responses that provider controls may select.
    pub response_options: Vec<InquiryResponseOption>,

    /// Identity ID this inquiry is assigned to
    #[schema(example = 1)]
    pub assigned_to: Option<Id>,

    pub assigned_to_login: Option<String>,

    pub assigned_to_display_name: Option<String>,

    /// Current status of the inquiry
    #[schema(example = "pending")]
    pub status: InquiryStatus,

    /// Response data provided by the user
    #[schema(value_type = Object, nullable = true)]
    pub response: Option<JsonDict>,

    /// When the inquiry expires
    #[schema(example = "2024-01-13T11:30:00Z")]
    pub timeout_at: Option<DateTime<Utc>>,

    pub responded_by: Option<Id>,

    pub responded_by_login: Option<String>,

    pub responded_by_display_name: Option<String>,

    /// When the inquiry was responded to
    #[schema(example = "2024-01-13T10:45:00Z")]
    pub responded_at: Option<DateTime<Utc>>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:45:00Z")]
    pub updated: DateTime<Utc>,
}

impl From<Inquiry> for InquiryResponse {
    fn from(inquiry: Inquiry) -> Self {
        let response = redact_response(inquiry.response, inquiry.response_schema.as_ref());
        Self {
            id: inquiry.id,
            created_by_execution: inquiry.created_by_execution,
            created_by_action_ref: None,
            created_by_pack_ref: None,
            workflow_execution: inquiry.workflow_execution,
            workflow_root_execution: None,
            workflow_action_ref: None,
            workflow_pack_ref: None,
            workflow_task_name: inquiry.workflow_task_name,
            purpose: inquiry.purpose,
            prompt: inquiry.prompt,
            response_schema: inquiry.response_schema,
            response_options: inquiry.response_options,
            assigned_to: inquiry.assigned_to,
            assigned_to_login: None,
            assigned_to_display_name: None,
            status: inquiry.status,
            response,
            timeout_at: inquiry.timeout_at,
            responded_by: inquiry.responded_by,
            responded_by_login: None,
            responded_by_display_name: None,
            responded_at: inquiry.responded_at,
            created: inquiry.created,
            updated: inquiry.updated,
        }
    }
}

fn redact_response(
    response: Option<JsonDict>,
    response_schema: Option<&JsonSchema>,
) -> Option<JsonDict> {
    response.map(|value| redact_secret_parameters(value, response_schema).0)
}

/// Summary inquiry response for list views
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InquirySummary {
    /// Inquiry ID
    #[schema(example = 1)]
    pub id: Id,

    /// Execution ID that created this inquiry
    #[schema(example = 1)]
    pub created_by_execution: Id,

    pub created_by_action_ref: Option<String>,

    pub created_by_pack_ref: Option<String>,

    pub workflow_execution: Option<Id>,

    pub workflow_root_execution: Option<Id>,

    pub workflow_action_ref: Option<String>,

    pub workflow_pack_ref: Option<String>,

    pub workflow_task_name: Option<String>,

    /// Prompt text
    #[schema(example = "Approve deployment to production?")]
    pub prompt: String,

    /// Assigned identity ID
    #[schema(example = 1)]
    pub assigned_to: Option<Id>,

    pub assigned_to_login: Option<String>,

    pub assigned_to_display_name: Option<String>,

    /// Inquiry status
    #[schema(example = "pending")]
    pub status: InquiryStatus,

    /// Whether a response has been provided
    #[schema(example = false)]
    pub has_response: bool,

    /// Timeout timestamp
    #[schema(example = "2024-01-13T11:30:00Z")]
    pub timeout_at: Option<DateTime<Utc>>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,
}

impl From<Inquiry> for InquirySummary {
    fn from(inquiry: Inquiry) -> Self {
        Self {
            id: inquiry.id,
            created_by_execution: inquiry.created_by_execution,
            created_by_action_ref: None,
            created_by_pack_ref: None,
            workflow_execution: inquiry.workflow_execution,
            workflow_root_execution: None,
            workflow_action_ref: None,
            workflow_pack_ref: None,
            workflow_task_name: inquiry.workflow_task_name,
            prompt: inquiry.prompt,
            assigned_to: inquiry.assigned_to,
            assigned_to_login: None,
            assigned_to_display_name: None,
            status: inquiry.status,
            has_response: inquiry.response.is_some(),
            timeout_at: inquiry.timeout_at,
            created: inquiry.created,
        }
    }
}

/// Request to create a new inquiry
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateInquiryRequest {
    /// Stable purpose used to make creation idempotent within this workflow task attempt.
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "approval")]
    pub purpose: String,

    /// Prompt text to display to the user
    #[validate(length(min = 1, max = 10000))]
    #[schema(example = "Approve deployment to production?")]
    pub prompt: String,

    /// Optional schema for the expected response format (flat format with inline required/secret)
    #[schema(value_type = Option<Object>, example = json!({"approved": {"type": "boolean", "description": "Whether the deployment is approved", "required": true}}))]
    pub response_schema: Option<JsonSchema>,

    /// Fixed response choices rendered by provider actions.
    #[validate(length(min = 1, max = 25))]
    pub response_options: Vec<InquiryResponseOption>,

    /// Optional identity ID to assign this inquiry to
    #[schema(example = 1)]
    pub assigned_to: Option<Id>,

    /// Optional relative timeout in seconds.
    #[validate(range(min = 1))]
    #[schema(example = 3600)]
    pub timeout_seconds: Option<i64>,
}

/// Provider rendering metadata for one fixed response option.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InquiryResponseOptionHandle {
    pub r#ref: String,
    pub label: String,
    pub style: InquiryResponseOptionStyle,

    #[schema(example = "attune_irh_REDACTED", min_length = 12, max_length = 96)]
    pub response_handle: String,
}

/// Creation result containing the inquiry and one opaque handle per response option.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CreateInquiryResponse {
    pub inquiry: InquiryResponse,
    pub response_options: Vec<InquiryResponseOptionHandle>,
}

/// Request to respond to an inquiry (user-facing endpoint)
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct InquiryRespondRequest {
    /// Response data conforming to the inquiry's response_schema
    #[schema(value_type = Object)]
    pub response: JsonValue,
}

/// Query parameters for filtering inquiries
#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
pub struct InquiryQueryParams {
    /// Filter by status
    #[param(example = "pending")]
    pub status: Option<InquiryStatus>,

    /// Filter by creator execution ID
    #[param(example = 1)]
    pub created_by_execution: Option<Id>,

    /// Filter by assigned identity
    #[param(example = 1)]
    pub assigned_to: Option<Id>,

    /// Filter by the containing workflow action reference
    #[param(example = "core.deploy_workflow")]
    pub workflow_action_ref: Option<String>,

    /// Filter by the containing workflow pack reference
    #[param(example = "core")]
    pub workflow_pack_ref: Option<String>,

    /// Pagination offset
    #[param(example = 0)]
    pub offset: Option<usize>,

    /// Pagination limit
    #[param(example = 50)]
    pub limit: Option<usize>,
}

/// Paginated list response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ListResponse<T> {
    /// List of items
    pub data: Vec<T>,

    /// Total count of items (before pagination)
    pub total: usize,

    /// Offset used for this page
    pub offset: usize,

    /// Limit used for this page
    pub limit: usize,
}

#[cfg(test)]
mod tests {
    use super::InquiryResponse;
    use attune_common::models::{enums::InquiryStatus, inquiry::Inquiry};
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn inquiry_response_redacts_secret_schema_fields() {
        let schema = json!({
            "decision": {"type": "string"},
            "note": {"type": "string", "secret": true}
        });

        let now = Utc::now();
        let response = InquiryResponse::from(Inquiry {
            id: 1,
            created_by_execution: 2,
            workflow_execution: Some(3),
            workflow_task_name: Some("approval".to_string()),
            action_attempt_family: Some(4),
            purpose: Some("deployment".to_string()),
            prompt: "Approve deployment?".to_string(),
            response_schema: Some(schema),
            response_options: Vec::new(),
            assigned_to: Some(5),
            status: InquiryStatus::Responded,
            response: Some(json!({"decision": "approve", "note": "private reason"})),
            timeout_at: None,
            timeout_seconds: None,
            responded_by: Some(5),
            external_actor: None,
            responded_at: Some(now),
            created: now,
            updated: now,
        });
        let redacted = response.response.unwrap();

        assert_eq!(redacted["decision"], "approve");
        assert_eq!(redacted["note"]["redacted"], true);
        assert!(!redacted.to_string().contains("private reason"));
    }
}
