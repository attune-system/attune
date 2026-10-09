//! History DTOs for API requests and responses
//!
//! These types represent the API-facing view of entity history records
//! stored in ordinary PostgreSQL history tables.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};

use attune_common::models::entity_history::HistoryEntityType;

/// Response DTO for a single entity history record.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HistoryRecordResponse {
    /// When the change occurred
    #[schema(example = "2026-02-26T10:30:00Z")]
    pub time: DateTime<Utc>,

    /// The operation: `INSERT`, `UPDATE`, or `DELETE`
    #[schema(example = "UPDATE")]
    pub operation: String,

    /// The primary key of the changed entity
    #[schema(example = 42)]
    pub entity_id: i64,

    /// Denormalized human-readable identifier (e.g., action_ref, worker name)
    #[schema(example = "core.http_request")]
    pub entity_ref: Option<String>,

    /// Names of fields that changed (empty for INSERT/DELETE)
    #[schema(example = json!(["status", "result"]))]
    pub changed_fields: Vec<String>,

    /// Previous values of changed fields (null for INSERT)
    #[schema(value_type = Object, example = json!({"status": "requested"}))]
    pub old_values: Option<JsonValue>,

    /// New values of changed fields (null for DELETE)
    #[schema(value_type = Object, example = json!({"status": "running"}))]
    pub new_values: Option<JsonValue>,
}

impl From<attune_common::models::entity_history::EntityHistoryRecord> for HistoryRecordResponse {
    fn from(record: attune_common::models::entity_history::EntityHistoryRecord) -> Self {
        Self {
            time: record.time,
            operation: record.operation,
            entity_id: record.entity_id,
            entity_ref: record.entity_ref,
            changed_fields: record.changed_fields,
            old_values: record.old_values,
            new_values: record.new_values,
        }
    }
}

/// Query parameters for filtering history records.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct HistoryQueryParams {
    /// Filter by entity ID
    #[param(example = 42)]
    pub entity_id: Option<i64>,

    /// Filter by entity ref (e.g., action_ref, worker name)
    #[param(example = "core.http_request")]
    pub entity_ref: Option<String>,

    /// Filter by operation type: `INSERT`, `UPDATE`, or `DELETE`
    #[param(example = "UPDATE")]
    pub operation: Option<String>,

    /// Only include records where this field was changed
    #[param(example = "status")]
    pub changed_field: Option<String>,

    /// Only include records at or after this time (ISO 8601)
    #[param(example = "2026-02-01T00:00:00Z")]
    pub since: Option<DateTime<Utc>>,

    /// Only include records at or before this time (ISO 8601)
    #[param(example = "2026-02-28T23:59:59Z")]
    pub until: Option<DateTime<Utc>>,

    /// Page number (1-based)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Number of items per page
    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 1000)]
    pub page_size: u32,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

impl HistoryQueryParams {
    /// Convert to the repository-level query params.
    pub fn to_repo_params(
        &self,
    ) -> attune_common::repositories::entity_history::HistoryQueryParams {
        let limit = (self.page_size.clamp(1, 1000)) as i64;
        let offset = ((self.page.saturating_sub(1)) as i64) * limit;

        attune_common::repositories::entity_history::HistoryQueryParams {
            entity_id: self.entity_id,
            entity_ref: self.entity_ref.clone(),
            operation: self.operation.clone(),
            changed_field: self.changed_field.clone(),
            since: self.since,
            until: self.until,
            limit: Some(limit),
            offset: Some(offset),
        }
    }
}

/// Path parameter for the entity type segment.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct HistoryEntityTypePath {
    /// Entity type: `execution` or `worker`
    pub entity_type: String,
}

impl HistoryEntityTypePath {
    /// Parse the entity type string, returning a typed enum or an error message.
    pub fn parse(&self) -> Result<HistoryEntityType, String> {
        self.entity_type.parse::<HistoryEntityType>()
    }
}

/// Path parameters for entity-specific history (e.g., `/executions/42/history`).
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct EntityIdPath {
    /// The entity's primary key
    pub id: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_params_defaults() {
        let json = r#"{}"#;
        let params: HistoryQueryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.page, 1);
        assert_eq!(params.page_size, 50);
        assert!(params.entity_id.is_none());
        assert!(params.operation.is_none());
    }

    #[test]
    fn test_query_params_to_repo_params() {
        let params = HistoryQueryParams {
            entity_id: Some(42),
            entity_ref: None,
            operation: Some("UPDATE".to_string()),
            changed_field: Some("status".to_string()),
            since: None,
            until: None,
            page: 3,
            page_size: 20,
        };

        let repo = params.to_repo_params();
        assert_eq!(repo.entity_id, Some(42));
        assert_eq!(repo.operation, Some("UPDATE".to_string()));
        assert_eq!(repo.changed_field, Some("status".to_string()));
        assert_eq!(repo.limit, Some(20));
        assert_eq!(repo.offset, Some(40)); // (3-1) * 20
    }

    #[test]
    fn test_query_params_page_size_cap() {
        let params = HistoryQueryParams {
            entity_id: None,
            entity_ref: None,
            operation: None,
            changed_field: None,
            since: None,
            until: None,
            page: 1,
            page_size: 5000,
        };

        let repo = params.to_repo_params();
        assert_eq!(repo.limit, Some(1000));
    }

    #[test]
    fn test_entity_type_path_parse() {
        let path = HistoryEntityTypePath {
            entity_type: "execution".to_string(),
        };
        assert_eq!(path.parse().unwrap(), HistoryEntityType::Execution);

        let path = HistoryEntityTypePath {
            entity_type: "unknown".to_string(),
        };
        assert!(path.parse().is_err());
    }
}
