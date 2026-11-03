//! Artifact DTOs for API requests and responses

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};

use attune_common::models::enums::{
    ArtifactBodyState, ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
    RetentionPolicyType,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// MIME content type
    #[schema(example = "text/plain")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    /// Size of the latest version in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
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

    /// Display size: execution-selected when scoped; omitted for global runtime logs.
    pub size_bytes: Option<i64>,

    /// Highest version for the requested execution; null for global browsing.
    pub selected_version: Option<ArtifactVersionSummary>,

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
#[serde(deny_unknown_fields)]
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

    /// Opaque continuation bound to this endpoint and its filters.
    pub cursor: Option<String>,

    /// Items per page
    #[serde(default = "default_per_page")]
    #[param(example = 20, minimum = 1, maximum = 100)]
    pub per_page: u32,
}

impl ArtifactQueryParams {
    pub fn validate(&self) -> Result<(), String> {
        validate_artifact_page_size(self.per_page)
    }
}

/// Bounded version-history pages, with the artifact list's v1 parameter names.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPageParams {
    pub cursor: Option<String>,
    #[serde(default = "default_per_page")]
    #[param(example = 20, minimum = 1, maximum = 100)]
    pub per_page: u32,
}

impl ArtifactPageParams {
    pub fn validate(&self) -> Result<(), String> {
        validate_artifact_page_size(self.per_page)
    }
}

/// Execution-scoped artifact pages. The execution identity comes from the path.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ExecutionArtifactQueryParams {
    pub cursor: Option<String>,
    #[serde(default = "default_per_page")]
    #[param(example = 20, minimum = 1, maximum = 100)]
    pub per_page: u32,
    /// Optional type filter, including bounded progress-artifact discovery.
    pub r#type: Option<ArtifactType>,
}

impl ExecutionArtifactQueryParams {
    pub fn validate(&self) -> Result<(), String> {
        validate_artifact_page_size(self.per_page)
    }
}

fn validate_artifact_page_size(per_page: u32) -> Result<(), String> {
    if !(1..=100).contains(&per_page) {
        return Err("Use per_page between 1 and 100".into());
    }
    Ok(())
}

/// Count-free v1 artifact collection response. Pages are not historical snapshots.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ArtifactCursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
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

    /// Object-backed body lifecycle. NULL identifies an unmigrated row.
    pub body_state: Option<ArtifactBodyState>,

    /// Immutable object-store locator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_key: Option<String>,

    /// Opaque provider generation, version ID, or ETag pinned for reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,

    /// SHA-256 digest of the ready body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

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

    /// Reserved; arbitrary creator metadata is never exposed by summaries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Creation timestamp
    pub created: DateTime<Utc>,
}

// ============================================================================
// Conversions
// ============================================================================

impl From<attune_common::models::artifact::Artifact> for ArtifactResponse {
    fn from(a: attune_common::models::artifact::Artifact) -> Self {
        let file_body = attune_common::repositories::artifact::is_file_backed_type(a.r#type);
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
            data: if file_body { None } else { a.data },
            created: a.created,
            updated: a.updated,
        }
    }
}

impl ArtifactResponse {
    /// Explicit human runtime-log header allowlist. No aggregate latest fields,
    /// descriptions, arbitrary data, or body/provenance records are disclosed.
    pub fn runtime_log_header(
        header: attune_common::repositories::artifact::ArtifactMetadataRow,
    ) -> Self {
        Self {
            id: header.id,
            r#ref: header.r#ref,
            scope: header.scope,
            owner: header.owner,
            r#type: header.r#type,
            visibility: header.visibility,
            classification: header.classification,
            retention_policy: header.retention_policy,
            retention_limit: header.retention_limit,
            name: header.name,
            description: None,
            content_type: None,
            size_bytes: None,
            data: None,
            created: header.created,
            updated: header.updated,
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
            content_type: if a.classification == ArtifactClassification::RuntimeLog {
                None
            } else {
                a.content_type
            },
            size_bytes: if a.classification == ArtifactClassification::RuntimeLog {
                None
            } else {
                a.size_bytes
            },
            selected_version: None,
            scope: a.scope,
            owner: a.owner,
            created: a.created,
            updated: a.updated,
        }
    }
}

impl From<attune_common::repositories::artifact::ArtifactSummaryRow> for ArtifactSummary {
    fn from(a: attune_common::repositories::artifact::ArtifactSummaryRow) -> Self {
        Self {
            id: a.id,
            r#ref: a.r#ref,
            r#type: a.r#type,
            visibility: a.visibility,
            classification: a.classification,
            name: a.name,
            content_type: if a.classification == ArtifactClassification::RuntimeLog {
                None
            } else {
                a.content_type
            },
            size_bytes: if a.classification == ArtifactClassification::RuntimeLog {
                None
            } else {
                a.size_bytes
            },
            selected_version: None,
            scope: a.scope,
            owner: a.owner,
            created: a.created,
            updated: a.updated,
        }
    }
}

impl From<attune_common::repositories::artifact::ExecutionArtifactSummaryRow> for ArtifactSummary {
    fn from(row: attune_common::repositories::artifact::ExecutionArtifactSummaryRow) -> Self {
        let mut summary = Self::from(row.artifact);
        summary.content_type = row.selected_content_type.clone();
        summary.size_bytes = row.selected_size_bytes;
        summary.selected_version = Some(ArtifactVersionSummary {
            id: row.selected_id,
            version: row.selected_version,
            execution: row.selected_execution,
            content_type: row.selected_content_type,
            size_bytes: row.selected_size_bytes,
            created_by: None,
            created: row.selected_created,
        });
        summary
    }
}

impl From<attune_common::models::artifact_version::ArtifactVersion> for ArtifactVersionResponse {
    fn from(v: attune_common::models::artifact_version::ArtifactVersion) -> Self {
        let file_body = v.file_path.is_some() || v.object_key.is_some();
        Self {
            id: v.id,
            artifact: v.artifact,
            version: v.version,
            execution: v.execution,
            content_type: v.content_type,
            size_bytes: v.size_bytes,
            content_json: if file_body { None } else { v.content_json },
            file_path: v.file_path,
            body_state: v.body_state,
            object_key: v.object_key,
            provider_version: v.provider_version,
            sha256: v.sha256,
            meta: if file_body { None } else { v.meta },
            created_by: if file_body { None } else { v.created_by },
            created: v.created,
        }
    }
}

impl ArtifactVersionResponse {
    /// File content is available only through the guarded byte reader. This
    /// also covers old inline-file rows without a physical-storage marker.
    pub fn projection(v: attune_common::models::ArtifactVersion, file_body: bool) -> Self {
        let mut response = Self::from(v);
        if file_body {
            response.content_json = None;
            response.meta = None;
            response.created_by = None;
        }
        response
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
            created_by: None,
            created: v.created,
        }
    }
}

impl From<attune_common::repositories::artifact::ArtifactVersionSummaryRow>
    for ArtifactVersionSummary
{
    fn from(v: attune_common::repositories::artifact::ArtifactVersionSummaryRow) -> Self {
        Self {
            id: v.id,
            version: v.version,
            execution: v.execution,
            content_type: v.content_type,
            size_bytes: v.size_bytes,
            created_by: None,
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
        assert!(params.cursor.is_none());
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
    fn test_query_params_accepts_cursor() {
        let params = ArtifactQueryParams {
            scope: None,
            owner: None,
            r#type: None,
            visibility: None,
            classification: None,
            execution: None,
            name: None,
            cursor: Some("opaque".into()),
            per_page: 20,
        };
        assert!(params.validate().is_ok());
        assert_eq!(params.cursor.as_deref(), Some("opaque"));
    }

    #[test]
    fn test_query_params_rejects_unbounded_limit() {
        let params = ArtifactQueryParams {
            scope: None,
            owner: None,
            r#type: None,
            visibility: None,
            classification: None,
            execution: None,
            name: None,
            cursor: None,
            per_page: 200,
        };
        assert!(params.validate().is_err());
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
