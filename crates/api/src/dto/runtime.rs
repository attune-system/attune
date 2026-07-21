//! Runtime DTOs for API requests and responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::dto::common::PaginationParams;

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

/// Query parameters for `GET /api/v1/runtimes`.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct RuntimeListParams {
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    /// Keyword query. Tokens are AND-matched across ref, name, description,
    /// and pack_ref.
    #[param(example = "python core")]
    pub q: Option<String>,
}

impl RuntimeListParams {
    pub fn pagination(&self) -> PaginationParams {
        PaginationParams {
            page: self.page,
            page_size: self.page_size,
        }
    }
}

/// Request DTO for creating a runtime.
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreateRuntimeRequest {
    /// Unique reference identifier (e.g. "core.python", "core.nodejs")
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "core.python")]
    pub r#ref: String,

    /// Optional pack reference this runtime belongs to
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,

    /// Optional human-readable description
    #[validate(length(min = 1))]
    #[schema(example = "Python runtime with virtualenv support", nullable = true)]
    pub description: Option<String>,

    /// Display name
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Python")]
    pub name: String,

    /// Distribution metadata used for verification and platform support
    #[serde(default)]
    #[schema(value_type = Object, example = json!({"linux": {"supported": true}}))]
    pub distributions: JsonValue,

    /// Optional installation metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object, nullable = true, example = json!({"method": "system"}))]
    pub installation: Option<JsonValue>,

    /// Runtime execution configuration
    #[serde(default)]
    #[schema(value_type = Object, example = json!({"interpreter": {"command": "python3"}}))]
    pub execution_config: JsonValue,
}

/// Request DTO for updating a runtime.
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdateRuntimeRequest {
    /// Optional human-readable description patch.
    pub description: Option<NullableStringPatch>,

    /// Display name
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Python 3")]
    pub name: Option<String>,

    /// Distribution metadata used for verification and platform support
    #[schema(value_type = Object, nullable = true)]
    pub distributions: Option<JsonValue>,

    /// Optional installation metadata patch.
    pub installation: Option<NullableJsonPatch>,

    /// Runtime execution configuration
    #[schema(value_type = Object, nullable = true)]
    pub execution_config: Option<JsonValue>,
}

/// Explicit patch operation for nullable string fields.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum NullableStringPatch {
    #[schema(title = "SetString")]
    Set(String),
    Clear,
}

/// Explicit patch operation for nullable JSON fields.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum NullableJsonPatch {
    #[schema(title = "SetJson")]
    Set(JsonValue),
    Clear,
}

/// Full runtime response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RuntimeResponse {
    #[schema(example = 1)]
    pub id: i64,

    #[schema(example = "core.python")]
    pub r#ref: String,

    #[schema(example = 1, nullable = true)]
    pub pack: Option<i64>,

    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,

    #[schema(example = "Python runtime with virtualenv support", nullable = true)]
    pub description: Option<String>,

    #[schema(example = "Python")]
    pub name: String,

    #[schema(value_type = Object)]
    pub distributions: JsonValue,

    #[schema(value_type = Object, nullable = true)]
    pub installation: Option<JsonValue>,

    #[schema(value_type = Object)]
    pub execution_config: JsonValue,

    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Runtime summary for list views.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RuntimeSummary {
    #[schema(example = 1)]
    pub id: i64,

    #[schema(example = "core.python")]
    pub r#ref: String,

    #[schema(example = "core", nullable = true)]
    pub pack_ref: Option<String>,

    #[schema(example = "Python runtime with virtualenv support", nullable = true)]
    pub description: Option<String>,

    #[schema(example = "Python")]
    pub name: String,

    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

impl From<attune_common::models::runtime::Runtime> for RuntimeResponse {
    fn from(runtime: attune_common::models::runtime::Runtime) -> Self {
        Self {
            id: runtime.id,
            r#ref: runtime.r#ref,
            pack: runtime.pack,
            pack_ref: runtime.pack_ref,
            description: runtime.description,
            name: runtime.name,
            distributions: runtime.distributions,
            installation: runtime.installation,
            execution_config: runtime.execution_config,
            created: runtime.created,
            updated: runtime.updated,
        }
    }
}

impl From<attune_common::models::runtime::Runtime> for RuntimeSummary {
    fn from(runtime: attune_common::models::runtime::Runtime) -> Self {
        Self {
            id: runtime.id,
            r#ref: runtime.r#ref,
            pack_ref: runtime.pack_ref,
            description: runtime.description,
            name: runtime.name,
            created: runtime.created,
            updated: runtime.updated,
        }
    }
}
