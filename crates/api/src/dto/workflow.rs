//! Workflow DTOs for API requests and responses

use std::borrow::Cow;

use attune_common::{models::enums::ActionReferenceVisibility, schema::RefValidator};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationError};

fn validation_error(code: &'static str, message: String) -> ValidationError {
    let mut error = ValidationError::new(code);
    error.message = Some(Cow::Owned(message));
    error
}

fn validate_workflow_local_ref(value: &str) -> Result<(), ValidationError> {
    RefValidator::validate_component_ref(&format!("pack.{value}"))
        .map_err(|e| validation_error("workflow_name", e.to_string()))
}

fn validate_workflow_ref(value: &str) -> Result<(), ValidationError> {
    RefValidator::validate_component_ref(value)
        .map_err(|e| validation_error("workflow_ref", e.to_string()))
}

fn validate_pack_ref_field(value: &str) -> Result<(), ValidationError> {
    RefValidator::validate_pack_ref(value).map_err(|e| validation_error("pack_ref", e.to_string()))
}

/// Request DTO for saving a workflow file to disk and syncing to DB
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct SaveWorkflowFileRequest {
    /// Workflow name (becomes filename: {name}.workflow.yaml)
    #[validate(length(min = 1, max = 255))]
    #[validate(custom(function = "validate_workflow_local_ref"))]
    #[schema(example = "deploy_app")]
    pub name: String,

    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Deploy Application")]
    pub label: String,

    /// Workflow description
    #[schema(example = "Deploys an application to the target environment")]
    pub description: Option<String>,

    /// Workflow version (semantic versioning recommended)
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Pack reference this workflow belongs to
    #[validate(length(min = 1, max = 255))]
    #[validate(custom(function = "validate_pack_ref_field"))]
    #[schema(example = "core")]
    pub pack_ref: String,

    /// Whether the companion workflow action is enabled. Omitted defaults to true.
    #[schema(example = true, default = true, nullable = true)]
    pub enabled: Option<bool>,

    /// Pack-level visibility for references to the companion workflow action. Omitted defaults to public.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "public", default = "public", nullable = true)]
    pub reference_visibility: Option<ActionReferenceVisibility>,

    /// Pack refs allowed to reference the companion workflow action when visibility is restricted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["incident_response", "deployments"]), default = json!([]))]
    pub reference_allowed_pack_refs: Vec<String>,

    /// The full workflow definition as JSON (will be serialized to YAML on disk)
    #[schema(value_type = Object)]
    pub definition: JsonValue,

    /// Parameter schema (flat format with inline required/secret)
    #[schema(value_type = Object, nullable = true)]
    pub param_schema: Option<JsonValue>,

    /// Output schema (flat format)
    #[schema(value_type = Object, nullable = true)]
    pub out_schema: Option<JsonValue>,

    /// Tags for categorization
    #[schema(example = json!(["deployment", "automation"]))]
    pub tags: Option<Vec<String>>,
}

/// Request DTO for creating a new workflow
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateWorkflowRequest {
    /// Unique reference identifier (e.g., "core.notify_on_failure", "slack.incident_workflow")
    #[validate(length(min = 1, max = 255))]
    #[validate(custom(function = "validate_workflow_ref"))]
    #[schema(example = "slack.incident_workflow")]
    pub r#ref: String,

    /// Pack reference this workflow belongs to
    #[validate(length(min = 1, max = 255))]
    #[validate(custom(function = "validate_pack_ref_field"))]
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Incident Response Workflow")]
    pub label: String,

    /// Workflow description
    #[schema(example = "Automated incident response workflow with notifications and approvals")]
    pub description: Option<String>,

    /// Workflow version (semantic versioning recommended)
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Parameter schema (StackStorm-style) defining expected inputs with inline required/secret
    #[schema(value_type = Object, example = json!({"severity": {"type": "string", "description": "Incident severity", "required": true}, "channel": {"type": "string", "description": "Notification channel"}}))]
    pub param_schema: Option<JsonValue>,

    /// Output schema (flat format) defining expected outputs with inline required/secret
    #[schema(value_type = Object, example = json!({"incident_id": {"type": "string", "description": "Unique incident identifier", "required": true}}))]
    pub out_schema: Option<JsonValue>,

    /// Workflow definition (complete workflow YAML structure as JSON)
    #[schema(value_type = Object)]
    pub definition: JsonValue,

    /// Tags for categorization and search
    #[schema(example = json!(["incident", "slack", "approval"]))]
    pub tags: Option<Vec<String>>,
}

/// Request DTO for updating a workflow
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdateWorkflowRequest {
    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Incident Response Workflow (Updated)")]
    pub label: Option<String>,

    /// Workflow description
    #[schema(example = "Enhanced incident response workflow with additional automation")]
    pub description: Option<String>,

    /// Workflow version
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "1.1.0")]
    pub version: Option<String>,

    /// Parameter schema (StackStorm-style with inline required/secret)
    #[schema(value_type = Object, nullable = true)]
    pub param_schema: Option<JsonValue>,

    /// Output schema
    #[schema(value_type = Object, nullable = true)]
    pub out_schema: Option<JsonValue>,

    /// Workflow definition
    #[schema(value_type = Object, nullable = true)]
    pub definition: Option<JsonValue>,

    /// Tags
    #[schema(example = json!(["incident", "slack", "approval", "automation"]))]
    pub tags: Option<Vec<String>>,
}

/// Response DTO for workflow information
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowResponse {
    /// Workflow ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack.incident_workflow")]
    pub r#ref: String,

    /// Pack ID
    #[schema(example = 1)]
    pub pack: i64,

    /// Pack reference
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[schema(example = "Incident Response Workflow")]
    pub label: String,

    /// Workflow description
    #[schema(example = "Automated incident response workflow with notifications and approvals")]
    pub description: Option<String>,

    /// Workflow version
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Parameter schema (StackStorm-style with inline required/secret)
    #[schema(value_type = Object, nullable = true)]
    pub param_schema: Option<JsonValue>,

    /// Output schema
    #[schema(value_type = Object, nullable = true)]
    pub out_schema: Option<JsonValue>,

    /// Workflow definition
    #[schema(value_type = Object)]
    pub definition: JsonValue,

    /// Tags
    #[schema(example = json!(["incident", "slack", "approval"]))]
    pub tags: Vec<String>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Simplified workflow response (for list endpoints)
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowSummary {
    /// Workflow ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack.incident_workflow")]
    pub r#ref: String,

    /// Pack reference
    #[schema(example = "slack")]
    pub pack_ref: String,

    /// Human-readable label
    #[schema(example = "Incident Response Workflow")]
    pub label: String,

    /// Workflow description
    #[schema(example = "Automated incident response workflow with notifications and approvals")]
    pub description: Option<String>,

    /// Workflow version
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Tags
    #[schema(example = json!(["incident", "slack", "approval"]))]
    pub tags: Vec<String>,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Convert from WorkflowDefinition model to WorkflowResponse
impl From<attune_common::models::workflow::WorkflowDefinition> for WorkflowResponse {
    fn from(workflow: attune_common::models::workflow::WorkflowDefinition) -> Self {
        Self {
            id: workflow.id,
            r#ref: workflow.r#ref,
            pack: workflow.pack,
            pack_ref: workflow.pack_ref,
            label: workflow.label,
            description: workflow.description,
            version: workflow.version,
            param_schema: workflow.param_schema,
            out_schema: workflow.out_schema,
            definition: workflow.definition,
            tags: workflow.tags,
            created: workflow.created,
            updated: workflow.updated,
        }
    }
}

/// Convert from WorkflowDefinition model to WorkflowSummary
impl From<attune_common::models::workflow::WorkflowDefinition> for WorkflowSummary {
    fn from(workflow: attune_common::models::workflow::WorkflowDefinition) -> Self {
        Self {
            id: workflow.id,
            r#ref: workflow.r#ref,
            pack_ref: workflow.pack_ref,
            label: workflow.label,
            description: workflow.description,
            version: workflow.version,
            tags: workflow.tags,
            created: workflow.created,
            updated: workflow.updated,
        }
    }
}

/// Query parameters for workflow search and filtering
#[derive(Debug, Clone, Deserialize, Validate, IntoParams)]
pub struct WorkflowSearchParams {
    /// Filter by tag(s) - comma-separated list
    #[param(example = "incident,approval")]
    pub tags: Option<String>,

    /// Search term for label/description (case-insensitive)
    #[param(example = "incident")]
    pub search: Option<String>,

    /// Filter by pack reference
    #[param(example = "core")]
    pub pack_ref: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_workflow_request_validation() {
        let req = CreateWorkflowRequest {
            r#ref: "".to_string(), // Invalid: empty
            pack_ref: "test-pack".to_string(),
            label: "Test Workflow".to_string(),
            description: Some("Test description".to_string()),
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: serde_json::json!({"tasks": []}),
            tags: None,
        };

        assert!(req.validate().is_err());
    }

    #[test]
    fn test_create_workflow_request_valid() {
        let req = CreateWorkflowRequest {
            r#ref: "test.workflow".to_string(),
            pack_ref: "test-pack".to_string(),
            label: "Test Workflow".to_string(),
            description: Some("Test description".to_string()),
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: serde_json::json!({"tasks": []}),
            tags: Some(vec!["test".to_string()]),
        };

        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_update_workflow_request_all_none() {
        let req = UpdateWorkflowRequest {
            label: None,
            description: None,
            version: None,
            param_schema: None,
            out_schema: None,
            definition: None,
            tags: None,
        };

        // Should be valid even with all None values
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_workflow_search_params() {
        let params = WorkflowSearchParams {
            tags: Some("incident,approval".to_string()),
            search: Some("response".to_string()),
            pack_ref: Some("core".to_string()),
        };

        assert!(params.validate().is_ok());
    }
}
