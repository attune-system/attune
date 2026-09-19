//! Inquiry data transfer objects

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use attune_common::models::{enums::InquiryStatus, inquiry::Inquiry, Id, JsonDict, JsonSchema};
use serde_json::Value as JsonValue;

/// Full inquiry response with all details
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InquiryResponse {
    /// Inquiry ID
    #[schema(example = 1)]
    pub id: Id,

    /// Execution ID this inquiry belongs to
    #[schema(example = 1)]
    pub execution: Id,

    pub workflow_execution: Option<Id>,

    pub workflow_task_name: Option<String>,

    pub purpose: Option<String>,

    /// Prompt text displayed to the user
    #[schema(example = "Approve deployment to production?")]
    pub prompt: String,

    /// JSON schema for expected response
    #[schema(value_type = Object, nullable = true)]
    pub response_schema: Option<JsonSchema>,

    /// Identity ID this inquiry is assigned to
    #[schema(example = 1)]
    pub assigned_to: Option<Id>,

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
        Self {
            id: inquiry.id,
            execution: inquiry.execution,
            workflow_execution: inquiry.workflow_execution,
            workflow_task_name: inquiry.workflow_task_name,
            purpose: inquiry.purpose,
            prompt: inquiry.prompt,
            response_schema: inquiry.response_schema,
            assigned_to: inquiry.assigned_to,
            status: inquiry.status,
            response: inquiry.response,
            timeout_at: inquiry.timeout_at,
            responded_by: inquiry.responded_by,
            responded_at: inquiry.responded_at,
            created: inquiry.created,
            updated: inquiry.updated,
        }
    }
}

/// Summary inquiry response for list views
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InquirySummary {
    /// Inquiry ID
    #[schema(example = 1)]
    pub id: Id,

    /// Execution ID
    #[schema(example = 1)]
    pub execution: Id,

    /// Prompt text
    #[schema(example = "Approve deployment to production?")]
    pub prompt: String,

    /// Assigned identity ID
    #[schema(example = 1)]
    pub assigned_to: Option<Id>,

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
            execution: inquiry.execution,
            prompt: inquiry.prompt,
            assigned_to: inquiry.assigned_to,
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
    #[schema(value_type = Object, example = json!({"approved": {"type": "boolean", "description": "Whether the deployment is approved", "required": true}}))]
    pub response_schema: Option<JsonSchema>,

    /// Optional identity ID to assign this inquiry to
    #[schema(example = 1)]
    pub assigned_to: Option<Id>,

    /// Optional relative timeout in seconds.
    #[validate(range(min = 1))]
    #[schema(example = 3600)]
    pub timeout_seconds: Option<i64>,
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

    /// Filter by execution ID
    #[param(example = 1)]
    pub execution: Option<Id>,

    /// Filter by assigned identity
    #[param(example = 1)]
    pub assigned_to: Option<Id>,

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
