//! Artifact DTOs for API requests and responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};

use attune_common::models::enums::{
    ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
};

// ============================================================================
// Artifact DTOs
// ============================================================================

/// Request DTO for creating a new artifact
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateArtifactRequest {
    /// Artifact reference (unique identifier, e.g. "build.log", "test.results")
    #[schema(example = "mypack.build_log")]
    pub r#ref: String,

    /// Owner scope type
    #[schema(example = "action")]
    pub scope: OwnerType,

    /// Owner identifier (ref string of the owning entity)
    #[schema(example = "mypack.deploy")]
    pub owner: String,

    /// Artifact type
    #[schema(example = "file_text")]
    pub r#type: ArtifactType,

    /// Visibility level (public = all users, private = scope/owner restricted).
    /// If omitted, defaults to `public` for progress artifacts and `private` for all others.
    pub visibility: Option<ArtifactVisibility>,

    /// Retention policy type. If omitted, execution/action/sensor defaults may apply.
    #[schema(example = "versions", nullable = true)]
    pub retention_policy: Option<RetentionPolicyType>,

    /// Retention limit (number of versions, days, hours, or minutes depending on policy).
    /// If omitted, execution/action/sensor defaults may apply.
    #[schema(example = 5, nullable = true)]
    pub retention_limit: Option<i32>,

    /// Human-readable name
    #[schema(example = "Build Log")]
    pub name: Option<String>,

    /// Optional description
    #[schema(example = "Output log from the build action")]
    pub description: Option<String>,

    /// MIME content type (e.g. "text/plain", "application/json")
    #[schema(example = "text/plain")]
    pub content_type: Option<String>,

    /// Initial structured data (for progress-type artifacts or metadata)
    #[schema(value_type = Option<Object>)]
    pub data: Option<JsonValue>,
}

/// Request DTO for updating an existing artifact
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UpdateArtifactRequest {
    /// Updated owner scope
    pub scope: Option<OwnerType>,

    /// Updated owner identifier
    pub owner: Option<String>,

    /// Updated artifact type
    pub r#type: Option<ArtifactType>,

    /// Updated visibility
    pub visibility: Option<ArtifactVisibility>,

    /// Updated retention policy
    pub retention_policy: Option<RetentionPolicyType>,

    /// Updated retention limit
    pub retention_limit: Option<i32>,

    /// Updated name
    pub name: Option<ArtifactStringPatch>,

    /// Updated description
    pub description: Option<ArtifactStringPatch>,

    /// Updated content type
    pub content_type: Option<ArtifactStringPatch>,

    /// Updated structured data (replaces existing data entirely)
    pub data: Option<ArtifactJsonPatch>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum ArtifactStringPatch {
    Set(String),
    Clear,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum ArtifactJsonPatch {
    Set(JsonValue),
    Clear,
}

/// Request DTO for appending to a progress-type artifact
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AppendProgressRequest {
    /// The entry to append to the progress data array.
    /// Can be any JSON value (string, object, number, etc.)
    #[schema(value_type = Object, example = json!({"step": "compile", "status": "done", "duration_ms": 1234}))]
    pub entry: JsonValue,
}

/// Request DTO for setting the full data payload on an artifact
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SetDataRequest {
    /// The data to set (replaces existing data entirely)
    #[schema(value_type = Object)]
    pub data: JsonValue,
}

/// Response DTO for artifact information
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ArtifactResponse {
    /// Artifact ID
    #[schema(example = 1)]
    pub id: i64,

    /// Artifact reference
    #[schema(example = "mypack.build_log")]
    pub r#ref: String,

    /// Owner scope type
    pub scope: OwnerType,

    /// Owner identifier
    #[schema(example = "mypack.deploy")]
    pub owner: String,

    /// Artifact type
    pub r#type: ArtifactType,

    /// Visibility level
    pub visibility: ArtifactVisibility,

    /// Classification used to distinguish runtime log artifacts from general artifacts.
    pub classification: ArtifactClassification,

    /// Retention policy
    pub retention_policy: RetentionPolicyType,

    /// Retention limit
    #[schema(example = 5)]
    pub retention_limit: i32,

    /// Human-readable name
    #[schema(example = "Build Log")]
    pub name: Option<String>,

    /// Description
    pub description: Option<String>,

    /// MIME content type
    #[schema(example = "text/plain")]
    pub content_type: Option<String>,

    /// Size of the latest version in bytes
    pub size_bytes: Option<i64>,

    /// Structured data (progress entries, metadata, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,

    /// Creation timestamp
    pub created: DateTime<Utc>,

    /// Last update timestamp
    pub updated: DateTime<Utc>,
}

/// Simplified artifact for list endpoints
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ArtifactSummary {
    /// Artifact ID
    pub id: i64,

    /// Artifact reference
    pub r#ref: String,

    /// Artifact type
    pub r#type: ArtifactType,

    /// Visibility level
    pub visibility: ArtifactVisibility,

    /// Classification used to distinguish runtime log artifacts from general artifacts.
    pub classification: ArtifactClassification,

    /// Human-readable name
    pub name: Option<String>,

    /// MIME content type
    pub content_type: Option<String>,

    /// Size of latest version in bytes
    pub size_bytes: Option<i64>,

    /// Owner scope
    pub scope: OwnerType,

    /// Owner identifier
    pub owner: String,

    /// Creation timestamp
    pub created: DateTime<Utc>,

    /// Last update timestamp
    pub updated: DateTime<Utc>,
}

/// Query parameters for filtering artifacts
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct ArtifactQueryParams {
    /// Filter by owner scope type
    pub scope: Option<OwnerType>,

    /// Filter by owner identifier
    pub owner: Option<String>,

    /// Filter by artifact type
    pub r#type: Option<ArtifactType>,

    /// Filter by visibility
    pub visibility: Option<ArtifactVisibility>,

    /// Filter by classification
    pub classification: Option<ArtifactClassification>,

    /// Filter to artifacts that have at least one version produced by this execution
    pub execution: Option<i64>,

    /// Search by name (case-insensitive substring match)
    pub name: Option<String>,

    /// Page number (1-based)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Items per page
    #[serde(default = "default_per_page")]
    #[param(example = 20, minimum = 1, maximum = 100)]
    pub per_page: u32,
}

impl ArtifactQueryParams {
    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)) * self.per_page
    }

    pub fn limit(&self) -> u32 {
        self.per_page.min(100)
    }
}

fn default_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    20
}

// ============================================================================
// ArtifactVersion DTOs
// ============================================================================

/// Request DTO for creating a new artifact version with JSON content
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateVersionJsonRequest {
    /// Structured JSON content for this version
    #[schema(value_type = Object)]
    pub content: JsonValue,

    /// MIME content type override (defaults to "application/json")
    pub content_type: Option<String>,

    /// Execution that produced this version (optional)
    #[schema(example = 42)]
    pub execution: Option<i64>,

    /// Free-form metadata about this version
    #[schema(value_type = Option<Object>)]
    pub meta: Option<JsonValue>,

    /// Who created this version (e.g. action ref, identity, "system")
    pub created_by: Option<String>,
}

/// Request DTO for creating a new file-backed artifact version.
/// No file content is included — the caller writes the file directly to
/// `$ATTUNE_ARTIFACTS_DIR/{file_path}` after receiving the response.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateFileVersionRequest {
    /// MIME content type (e.g. "text/plain", "application/octet-stream")
    #[schema(example = "text/plain")]
    pub content_type: Option<String>,

    /// Execution that produced this version (optional)
    #[schema(example = 42)]
    pub execution: Option<i64>,

    /// Free-form metadata about this version
    #[schema(value_type = Option<Object>)]
    pub meta: Option<JsonValue>,

    /// Who created this version (e.g. action ref, identity, "system")
    pub created_by: Option<String>,
}

/// Request DTO for the upsert-and-allocate endpoint.
///
/// Looks up an artifact by ref (creating it if it doesn't exist), then
/// allocates a new file-backed version and returns the `file_path` where
/// the caller should write the file on the shared artifact volume.
///
/// This replaces the multi-step create → 409-handling → allocate dance
/// with a single API call.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AllocateFileVersionByRefRequest {
    // -- Artifact metadata (used only when creating a new artifact) ----------
    /// Owner scope type (default: action)
    #[schema(example = "action")]
    pub scope: Option<OwnerType>,

    /// Owner identifier (ref string of the owning entity)
    #[schema(example = "python_example.artifact_demo")]
    pub owner: Option<String>,

    /// Artifact type (must be a file-backed type; default: file_text)
    #[schema(example = "file_text")]
    pub r#type: Option<ArtifactType>,

    /// Visibility level. If omitted, uses type-aware default.
    pub visibility: Option<ArtifactVisibility>,

    /// Retention policy type (default: versions)
    pub retention_policy: Option<RetentionPolicyType>,

    /// Retention limit (default: 10)
    pub retention_limit: Option<i32>,

    /// Human-readable name
    #[schema(example = "Demo Log")]
    pub name: Option<String>,

    /// Optional description
    pub description: Option<String>,

    /// Execution ID to link this artifact to
    #[schema(example = 42)]
    pub execution: Option<i64>,

    // -- Version metadata ----------------------------------------------------
    /// MIME content type for this version (e.g. "text/plain")
    #[schema(example = "text/plain")]
    pub content_type: Option<String>,

    /// Free-form metadata about this version
    #[schema(value_type = Option<Object>)]
    pub meta: Option<JsonValue>,

    /// Who created this version (e.g. action ref, identity, "system")
    pub created_by: Option<String>,
}

/// Response DTO for an artifact version (without binary content)
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ArtifactVersionResponse {
    /// Version ID
    pub id: i64,

    /// Parent artifact ID
    pub artifact: i64,

    /// Version number (1-based)
    pub version: i32,

    /// Execution that produced this version (e.g., the execution that wrote
    /// this log version). Per-version association — the parent artifact may
    /// be linked to many executions across versions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<i64>,

    /// MIME content type
    pub content_type: Option<String>,

    /// Size of content in bytes
    pub size_bytes: Option<i64>,

    /// Structured JSON content (if this version has JSON data)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_json: Option<JsonValue>,

    /// Relative file path for disk-backed versions (from artifacts_dir root).
    /// When present, the file content lives on the shared volume, not in the DB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,

    /// Free-form metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,

    /// Who created this version
    pub created_by: Option<String>,

    /// Creation timestamp
    pub created: DateTime<Utc>,
}

/// Simplified version for list endpoints
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ArtifactVersionSummary {
    /// Version ID
    pub id: i64,

    /// Version number
    pub version: i32,

    /// Execution that produced this version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<i64>,

    /// MIME content type
    pub content_type: Option<String>,

    /// Size of content in bytes
    pub size_bytes: Option<i64>,

    /// Relative file path for disk-backed versions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,

    /// Who created this version
    pub created_by: Option<String>,

    /// Creation timestamp
    pub created: DateTime<Utc>,
}

// ============================================================================
// Conversions
// ============================================================================

impl From<attune_common::models::artifact::Artifact> for ArtifactResponse {
    fn from(a: attune_common::models::artifact::Artifact) -> Self {
        Self {
            id: a.id,
            r#ref: a.r#ref,
            scope: a.scope,
            owner: a.owner,
            r#type: a.r#type,
            visibility: a.visibility,
            classification: a.classification,
            retention_policy: a.retention_policy,
            retention_limit: a.retention_limit,
            name: a.name,
            description: a.description,
            content_type: a.content_type,
            size_bytes: a.size_bytes,
            data: a.data,
            created: a.created,
            updated: a.updated,
        }
    }
}

impl From<attune_common::models::artifact::Artifact> for ArtifactSummary {
    fn from(a: attune_common::models::artifact::Artifact) -> Self {
        Self {
            id: a.id,
            r#ref: a.r#ref,
            r#type: a.r#type,
            visibility: a.visibility,
            classification: a.classification,
            name: a.name,
            content_type: a.content_type,
            size_bytes: a.size_bytes,
            scope: a.scope,
            owner: a.owner,
            created: a.created,
            updated: a.updated,
        }
    }
}

impl From<attune_common::models::artifact_version::ArtifactVersion> for ArtifactVersionResponse {
    fn from(v: attune_common::models::artifact_version::ArtifactVersion) -> Self {
        Self {
            id: v.id,
            artifact: v.artifact,
            version: v.version,
            execution: v.execution,
            content_type: v.content_type,
            size_bytes: v.size_bytes,
            content_json: v.content_json,
            file_path: v.file_path,
            meta: v.meta,
            created_by: v.created_by,
            created: v.created,
        }
    }
}

impl From<attune_common::models::artifact_version::ArtifactVersion> for ArtifactVersionSummary {
    fn from(v: attune_common::models::artifact_version::ArtifactVersion) -> Self {
        Self {
            id: v.id,
            version: v.version,
            execution: v.execution,
            content_type: v.content_type,
            size_bytes: v.size_bytes,
            file_path: v.file_path,
            created_by: v.created_by,
            created: v.created,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_params_defaults() {
        let json = r#"{}"#;
        let params: ArtifactQueryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.page, 1);
        assert_eq!(params.per_page, 20);
        assert!(params.scope.is_none());
        assert!(params.r#type.is_none());
        assert!(params.visibility.is_none());
        assert!(params.classification.is_none());
    }

    #[test]
    fn test_query_params_with_classification() {
        let json = r#"{"classification":"runtime_log"}"#;
        let params: ArtifactQueryParams = serde_json::from_str(json).unwrap();
        assert_eq!(
            params.classification,
            Some(ArtifactClassification::RuntimeLog)
        );
    }

    #[test]
    fn test_query_params_offset() {
        let params = ArtifactQueryParams {
            scope: None,
            owner: None,
            r#type: None,
            visibility: None,
            classification: None,
            execution: None,
            name: None,
            page: 3,
            per_page: 20,
        };
        assert_eq!(params.offset(), 40);
    }

    #[test]
    fn test_query_params_limit_cap() {
        let params = ArtifactQueryParams {
            scope: None,
            owner: None,
            r#type: None,
            visibility: None,
            classification: None,
            execution: None,
            name: None,
            page: 1,
            per_page: 200,
        };
        assert_eq!(params.limit(), 100);
    }

    #[test]
    fn test_create_request_defaults() {
        let json = r#"{
            "ref": "test.artifact",
            "scope": "system",
            "owner": "",
            "type": "file_text"
        }"#;
        let req: CreateArtifactRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.retention_policy, None);
        assert_eq!(req.retention_limit, None);
        assert!(
            req.visibility.is_none(),
            "Omitting visibility should deserialize as None (server applies type-aware default)"
        );
    }

    #[test]
    fn test_append_progress_request() {
        let json = r#"{"entry": {"step": "build", "status": "done"}}"#;
        let req: AppendProgressRequest = serde_json::from_str(json).unwrap();
        assert!(req.entry.is_object());
    }
}
