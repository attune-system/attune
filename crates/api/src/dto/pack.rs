//! Pack DTOs for API requests and responses

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

/// Query parameters for `GET /api/v1/packs`.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PackListParams {
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    /// Keyword query. Tokens are AND-matched across ref, label, and description.
    #[param(example = "slack integration")]
    pub q: Option<String>,
}

impl PackListParams {
    pub fn pagination(&self) -> PaginationParams {
        PaginationParams {
            page: self.page,
            page_size: self.page_size,
        }
    }
}

/// Request DTO for creating a new pack
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreatePackRequest {
    /// Unique reference identifier (e.g., "core", "aws", "slack")
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "slack")]
    pub r#ref: String,

    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Slack Integration")]
    pub label: String,

    /// Pack description
    #[schema(example = "Integration with Slack for messaging and notifications")]
    pub description: Option<String>,

    /// Pack version (semver format recommended)
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Configuration schema (flat format with inline required/secret per parameter)
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object, example = json!({"api_token": {"type": "string", "description": "API authentication key", "required": true, "secret": true}}))]
    pub conf_schema: JsonValue,

    /// Pack configuration values
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object, example = json!({"api_token": "xoxb-..."}))]
    pub config: JsonValue,

    /// Pack metadata
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object, example = json!({"author": "Attune Team"}))]
    pub meta: JsonValue,

    /// Tags for categorization
    #[serde(default)]
    #[schema(example = json!(["messaging", "collaboration"]))]
    pub tags: Vec<String>,

    /// Runtime dependencies (e.g., shell, python, nodejs)
    #[serde(default)]
    #[schema(example = json!(["shell", "python"]))]
    pub runtime_deps: Vec<String>,

    /// Pack dependencies (refs of required packs)
    #[serde(default)]
    #[schema(example = json!(["core"]))]
    pub dependencies: Vec<String>,

    /// Whether this is a standard/built-in pack
    #[serde(default)]
    #[schema(example = false)]
    pub is_standard: bool,
}

/// Request DTO for registering a pack from local filesystem
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct RegisterPackRequest {
    /// Local filesystem path to the pack directory
    #[validate(length(min = 1))]
    #[schema(example = "/path/to/packs/mypack")]
    pub path: String,

    /// Skip running pack tests during registration
    #[serde(default)]
    #[schema(example = false)]
    pub skip_tests: bool,

    /// Force registration even if tests fail
    #[serde(default)]
    #[schema(example = false)]
    pub force: bool,
}

/// Request DTO for installing a pack from remote source
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct InstallPackRequest {
    /// Repository URL or source location
    #[validate(length(min = 1))]
    #[schema(example = "https://github.com/attune/pack-slack.git")]
    pub source: String,

    /// Git branch, tag, or commit reference
    #[schema(example = "main")]
    pub ref_spec: Option<String>,

    /// Replace an existing pack with the same ref
    #[serde(default)]
    #[schema(example = false)]
    pub force: bool,

    /// Skip running pack tests during installation
    #[serde(default)]
    #[schema(example = false)]
    pub skip_tests: bool,

    /// Skip dependency validation (not recommended)
    #[serde(default)]
    #[schema(example = false)]
    pub skip_deps: bool,

    /// Treat the source as explicit and do not resolve it through pack registries
    #[serde(default)]
    #[schema(example = false)]
    pub no_registry: bool,
}

/// API-managed pack registry index configuration.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackRegistryIndexResponse {
    pub id: i64,
    pub name: Option<String>,
    pub url: String,
    pub position: i32,
    pub enabled: bool,
    #[schema(value_type = Object)]
    pub headers: JsonValue,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

impl From<attune_common::models::PackRegistryIndex> for PackRegistryIndexResponse {
    fn from(index: attune_common::models::PackRegistryIndex) -> Self {
        let headers = index.headers.clone();
        Self::from_index_and_headers(index, headers)
    }
}

impl PackRegistryIndexResponse {
    pub fn from_index_and_headers(
        index: attune_common::models::PackRegistryIndex,
        headers: JsonValue,
    ) -> Self {
        Self {
            id: index.id,
            name: index.name,
            url: index.url,
            position: index.position,
            enabled: index.enabled,
            headers: redact_header_values(headers),
            created: index.created,
            updated: index.updated,
        }
    }
}

fn redact_header_values(headers: JsonValue) -> JsonValue {
    match headers {
        JsonValue::Object(headers) => JsonValue::Object(
            headers
                .into_iter()
                .map(|(name, _)| (name, JsonValue::String("[REDACTED]".to_string())))
                .collect(),
        ),
        _ => serde_json::json!({}),
    }
}

/// Request to add a configured pack registry index.
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct CreatePackRegistryIndexRequest {
    #[schema(example = "Attune Community")]
    pub name: Option<String>,
    #[validate(length(min = 1))]
    #[schema(example = "https://registry.example.com/attune/index.json")]
    pub url: String,
    /// Optional explicit search order position. Omit to append to the end.
    #[schema(example = 0)]
    pub position: Option<i32>,
    #[serde(default = "default_true")]
    #[schema(example = true)]
    pub enabled: bool,
    #[serde(default = "default_empty_object")]
    #[schema(value_type = Object)]
    pub headers: JsonValue,
}

/// Request to update a configured pack registry index.
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdatePackRegistryIndexRequest {
    #[schema(nullable = true)]
    pub name: Option<Option<String>>,
    #[validate(length(min = 1))]
    pub url: Option<String>,
    pub position: Option<i32>,
    pub enabled: Option<bool>,
    #[schema(value_type = Object, nullable = true)]
    pub headers: Option<JsonValue>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct BrowsePackIndexQuery {
    pub q: Option<String>,
    pub registry_id: Option<i64>,
    #[serde(default)]
    pub include_disabled: bool,
}

/// Indexed pack summary with the registry it was resolved from.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct IndexedPackResponse {
    pub pack: attune_common::pack_registry::PackIndexEntry,
    pub registry: PackRegistryIndexSummary,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackRegistryIndexSummary {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub url: String,
    pub position: i32,
}

/// Response for pack install/register operations with test results
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackInstallResponse {
    /// The installed/registered pack
    pub pack: PackResponse,

    /// Test execution result (if tests were run)
    pub test_result: Option<attune_common::models::pack_test::PackTestResult>,

    /// Whether tests were skipped
    pub tests_skipped: bool,
}

/// Request DTO for updating a pack
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct UpdatePackRequest {
    /// Human-readable label
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Slack Integration v2")]
    pub label: Option<String>,

    /// Pack description
    #[schema(example = "Enhanced Slack integration with new features")]
    pub description: Option<PackDescriptionPatch>,

    /// Pack version
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "2.0.0")]
    pub version: Option<String>,

    /// Configuration schema
    #[schema(value_type = Object, nullable = true)]
    pub conf_schema: Option<JsonValue>,

    /// Pack configuration values
    #[schema(value_type = Object, nullable = true)]
    pub config: Option<JsonValue>,

    /// Pack metadata
    #[schema(value_type = Object, nullable = true)]
    pub meta: Option<JsonValue>,

    /// Tags for categorization
    #[schema(example = json!(["messaging", "collaboration", "webhooks"]))]
    pub tags: Option<Vec<String>>,

    /// Runtime dependencies (e.g., shell, python, nodejs)
    #[schema(example = json!(["shell", "python"]))]
    pub runtime_deps: Option<Vec<String>>,

    /// Pack dependencies (refs of required packs)
    #[schema(example = json!(["core", "http"]))]
    pub dependencies: Option<Vec<String>>,

    /// Whether this is a standard pack
    #[schema(example = false)]
    pub is_standard: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum PackDescriptionPatch {
    Set(String),
    Clear,
}

/// Response DTO for pack information
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackResponse {
    /// Pack ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack")]
    pub r#ref: String,

    /// Human-readable label
    #[schema(example = "Slack Integration")]
    pub label: String,

    /// Pack description
    #[schema(example = "Integration with Slack for messaging and notifications")]
    pub description: Option<String>,

    /// Pack version
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Configuration schema
    #[schema(value_type = Object)]
    pub conf_schema: JsonValue,

    /// Pack configuration
    #[schema(value_type = Object)]
    pub config: JsonValue,

    /// Pack metadata
    #[schema(value_type = Object)]
    pub meta: JsonValue,

    /// Tags
    #[schema(example = json!(["messaging", "collaboration"]))]
    pub tags: Vec<String>,

    /// Runtime dependencies (e.g., shell, python, nodejs)
    #[schema(example = json!(["shell", "python"]))]
    pub runtime_deps: Vec<String>,

    /// Pack dependencies (refs of required packs)
    #[schema(example = json!(["core"]))]
    pub dependencies: Vec<String>,

    /// Is standard pack
    #[schema(example = false)]
    pub is_standard: bool,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Simplified pack response (for list endpoints)
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackSummary {
    /// Pack ID
    #[schema(example = 1)]
    pub id: i64,

    /// Unique reference identifier
    #[schema(example = "slack")]
    pub r#ref: String,

    /// Human-readable label
    #[schema(example = "Slack Integration")]
    pub label: String,

    /// Pack description
    #[schema(example = "Integration with Slack for messaging and notifications")]
    pub description: Option<String>,

    /// Pack version
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Tags
    #[schema(example = json!(["messaging", "collaboration"]))]
    pub tags: Vec<String>,

    /// Is standard pack
    #[schema(example = false)]
    pub is_standard: bool,

    /// Creation timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,

    /// Last update timestamp
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub updated: DateTime<Utc>,
}

/// Convert from Pack model to PackResponse
impl From<attune_common::models::Pack> for PackResponse {
    fn from(pack: attune_common::models::Pack) -> Self {
        Self {
            id: pack.id,
            r#ref: pack.r#ref,
            label: pack.label,
            description: pack.description,
            version: pack.version,
            conf_schema: pack.conf_schema,
            config: pack.config,
            meta: pack.meta,
            tags: pack.tags,
            runtime_deps: pack.runtime_deps,
            dependencies: pack.dependencies,
            is_standard: pack.is_standard,
            created: pack.created,
            updated: pack.updated,
        }
    }
}

/// Convert from Pack model to PackSummary
impl From<attune_common::models::Pack> for PackSummary {
    fn from(pack: attune_common::models::Pack) -> Self {
        Self {
            id: pack.id,
            r#ref: pack.r#ref,
            label: pack.label,
            description: pack.description,
            version: pack.version,
            tags: pack.tags,
            is_standard: pack.is_standard,
            created: pack.created,
            updated: pack.updated,
        }
    }
}

/// Response for pack workflow sync operation
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackWorkflowSyncResponse {
    /// Pack reference
    pub pack_ref: String,
    /// Number of workflows loaded from filesystem
    pub loaded_count: usize,
    /// Number of workflows registered/updated in database
    pub registered_count: usize,
    /// Individual workflow registration results
    pub workflows: Vec<WorkflowSyncResult>,
    /// Any errors encountered during sync
    pub errors: Vec<String>,
}

/// Individual workflow sync result
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkflowSyncResult {
    /// Workflow reference name
    pub ref_name: String,
    /// Whether the workflow was created (false = updated)
    pub created: bool,
    /// Workflow definition ID
    pub workflow_def_id: i64,
    /// Any warnings during registration
    pub warnings: Vec<String>,
}

/// Response for pack workflow validation operation
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackWorkflowValidationResponse {
    /// Pack reference
    pub pack_ref: String,
    /// Number of workflows validated
    pub validated_count: usize,
    /// Number of workflows with errors
    pub error_count: usize,
    /// Validation errors by workflow reference
    pub errors: std::collections::HashMap<String, Vec<String>>,
}

/// Request DTO for downloading packs
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct DownloadPacksRequest {
    /// List of pack sources (git URLs, HTTP URLs, or registry refs)
    #[validate(length(min = 1))]
    #[schema(example = json!(["https://github.com/attune/pack-slack.git", "aws@2.0.0"]))]
    pub packs: Vec<String>,

    /// Git reference (branch, tag, or commit) for git sources
    #[schema(example = "v1.0.0")]
    pub ref_spec: Option<String>,
}

/// Response DTO for download packs operation
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DownloadPacksResponse {
    /// Successfully downloaded packs
    pub downloaded_packs: Vec<DownloadedPack>,
    /// Failed pack downloads
    pub failed_packs: Vec<FailedPack>,
    /// Total number of packs requested
    pub total_count: usize,
    /// Number of successful downloads
    pub success_count: usize,
    /// Number of failed downloads
    pub failure_count: usize,
}

/// Information about a downloaded pack
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DownloadedPack {
    /// Original source
    pub source: String,
    /// Source type (git, http, registry)
    pub source_type: String,
    /// Local path to downloaded pack
    pub pack_path: String,
    /// Pack reference from pack.yaml
    pub pack_ref: String,
    /// Pack version from pack.yaml
    pub pack_version: String,
    /// Git commit hash (for git sources)
    pub git_commit: Option<String>,
    /// Directory checksum
    pub checksum: Option<String>,
}

/// Information about a failed pack download
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FailedPack {
    /// Pack source that failed
    pub source: String,
    /// Error message
    pub error: String,
}

/// Request DTO for getting pack dependencies
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct GetPackDependenciesRequest {
    /// List of pack directory paths to analyze
    #[validate(length(min = 1))]
    #[schema(example = json!(["/tmp/attune-packs/slack"]))]
    pub pack_paths: Vec<String>,

    /// Skip pack.yaml validation
    #[serde(default)]
    #[schema(example = false)]
    pub skip_validation: bool,
}

/// Response DTO for get pack dependencies operation
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GetPackDependenciesResponse {
    /// All dependencies found
    pub dependencies: Vec<PackDependency>,
    /// Runtime requirements by pack
    pub runtime_requirements: std::collections::HashMap<String, RuntimeRequirements>,
    /// Dependencies not yet installed
    pub missing_dependencies: Vec<PackDependency>,
    /// Packs that were analyzed
    pub analyzed_packs: Vec<AnalyzedPack>,
    /// Errors encountered during analysis
    pub errors: Vec<DependencyError>,
}

/// Pack dependency information
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PackDependency {
    /// Pack reference
    pub pack_ref: String,
    /// Version specification
    pub version_spec: String,
    /// Pack that requires this dependency
    pub required_by: String,
    /// Whether dependency is already installed
    pub already_installed: bool,
}

/// Runtime requirements for a pack
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RuntimeRequirements {
    /// Pack reference
    pub pack_ref: String,
    /// Python requirements
    pub python: Option<PythonRequirements>,
    /// Node.js requirements
    pub nodejs: Option<NodeJsRequirements>,
}

/// Python runtime requirements
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PythonRequirements {
    /// Python version requirement
    pub version: Option<String>,
    /// Path to requirements.txt
    pub requirements_file: Option<String>,
}

/// Node.js runtime requirements
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NodeJsRequirements {
    /// Node.js version requirement
    pub version: Option<String>,
    /// Path to package.json
    pub package_file: Option<String>,
}

/// Information about an analyzed pack
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnalyzedPack {
    /// Pack reference
    pub pack_ref: String,
    /// Pack directory path
    pub pack_path: String,
    /// Whether pack has dependencies
    pub has_dependencies: bool,
    /// Number of dependencies
    pub dependency_count: usize,
}

/// Dependency analysis error
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DependencyError {
    /// Pack path where error occurred
    pub pack_path: String,
    /// Error message
    pub error: String,
}

/// Request DTO for building pack environments
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct BuildPackEnvsRequest {
    /// List of pack directory paths
    #[validate(length(min = 1))]
    #[schema(example = json!(["/tmp/attune-packs/slack"]))]
    pub pack_paths: Vec<String>,

    /// Base directory for permanent pack storage
    #[schema(example = "/opt/attune/packs")]
    pub packs_base_dir: Option<String>,

    /// Python version to use
    #[serde(default = "default_python_version")]
    #[schema(example = "3.11")]
    pub python_version: String,

    /// Node.js version to use
    #[serde(default = "default_nodejs_version")]
    #[schema(example = "20")]
    pub nodejs_version: String,

    /// Skip building Python environments
    #[serde(default)]
    #[schema(example = false)]
    pub skip_python: bool,

    /// Skip building Node.js environments
    #[serde(default)]
    #[schema(example = false)]
    pub skip_nodejs: bool,

    /// Force rebuild of existing environments
    #[serde(default)]
    #[schema(example = false)]
    pub force_rebuild: bool,

    /// Timeout in seconds for building each environment
    #[serde(default = "default_build_timeout")]
    #[schema(example = 600)]
    pub timeout: u64,
}

/// Response DTO for build pack environments operation
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BuildPackEnvsResponse {
    /// Successfully built environments
    pub built_environments: Vec<BuiltEnvironment>,
    /// Failed environment builds
    pub failed_environments: Vec<FailedEnvironment>,
    /// Summary statistics
    pub summary: BuildSummary,
}

/// Information about a built environment
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BuiltEnvironment {
    /// Pack reference
    pub pack_ref: String,
    /// Pack directory path
    pub pack_path: String,
    /// Built environments
    pub environments: Environments,
    /// Build duration in milliseconds
    pub duration_ms: u64,
}

/// Environment details
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Environments {
    /// Python environment
    pub python: Option<PythonEnvironment>,
    /// Node.js environment
    pub nodejs: Option<NodeJsEnvironment>,
}

/// Python environment details
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PythonEnvironment {
    /// Path to virtualenv
    pub virtualenv_path: String,
    /// Whether requirements were installed
    pub requirements_installed: bool,
    /// Number of packages installed
    pub package_count: usize,
    /// Python version used
    pub python_version: String,
}

/// Node.js environment details
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NodeJsEnvironment {
    /// Path to node_modules
    pub node_modules_path: String,
    /// Whether dependencies were installed
    pub dependencies_installed: bool,
    /// Number of packages installed
    pub package_count: usize,
    /// Node.js version used
    pub nodejs_version: String,
}

/// Failed environment build
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FailedEnvironment {
    /// Pack reference
    pub pack_ref: String,
    /// Pack directory path
    pub pack_path: String,
    /// Runtime that failed
    pub runtime: String,
    /// Error message
    pub error: String,
}

/// Build summary statistics
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BuildSummary {
    /// Total packs processed
    pub total_packs: usize,
    /// Successfully built
    pub success_count: usize,
    /// Failed builds
    pub failure_count: usize,
    /// Python environments built
    pub python_envs_built: usize,
    /// Node.js environments built
    pub nodejs_envs_built: usize,
    /// Total duration in milliseconds
    pub total_duration_ms: u64,
}

/// Request DTO for registering multiple packs
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct RegisterPacksRequest {
    /// List of pack directory paths to register
    #[validate(length(min = 1))]
    #[schema(example = json!(["/tmp/attune-packs/slack"]))]
    pub pack_paths: Vec<String>,

    /// Base directory for permanent storage
    #[schema(example = "/opt/attune/packs")]
    pub packs_base_dir: Option<String>,

    /// Skip schema validation
    #[serde(default)]
    #[schema(example = false)]
    pub skip_validation: bool,

    /// Skip running pack tests
    #[serde(default)]
    #[schema(example = false)]
    pub skip_tests: bool,

    /// Force registration (replace if exists)
    #[serde(default)]
    #[schema(example = false)]
    pub force: bool,
}

/// Response DTO for register packs operation
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RegisterPacksResponse {
    /// Successfully registered packs
    pub registered_packs: Vec<RegisteredPack>,
    /// Failed pack registrations
    pub failed_packs: Vec<FailedPackRegistration>,
    /// Summary statistics
    pub summary: RegistrationSummary,
}

/// Information about a registered pack
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RegisteredPack {
    /// Pack reference
    pub pack_ref: String,
    /// Pack database ID
    pub pack_id: i64,
    /// Pack version
    pub pack_version: String,
    /// Permanent storage path
    pub storage_path: String,
    /// Registered components by type
    pub components_registered: ComponentCounts,
    /// Test results
    pub test_result: Option<TestResult>,
    /// Validation results
    pub validation_results: ValidationResults,
}

/// Component counts
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ComponentCounts {
    /// Number of actions
    pub actions: usize,
    /// Number of sensors
    pub sensors: usize,
    /// Number of triggers
    pub triggers: usize,
    /// Number of rules
    pub rules: usize,
    /// Number of workflows
    pub workflows: usize,
    /// Number of policies
    pub policies: usize,
}

/// Test result
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TestResult {
    /// Test status
    pub status: String,
    /// Total number of tests
    pub total_tests: usize,
    /// Number passed
    pub passed: usize,
    /// Number failed
    pub failed: usize,
}

/// Validation results
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ValidationResults {
    /// Whether validation passed
    pub valid: bool,
    /// Validation errors
    pub errors: Vec<String>,
}

/// Failed pack registration
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FailedPackRegistration {
    /// Pack reference
    pub pack_ref: String,
    /// Pack path
    pub pack_path: String,
    /// Error message
    pub error: String,
    /// Error stage
    pub error_stage: String,
}

/// Registration summary
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RegistrationSummary {
    /// Total packs processed
    pub total_packs: usize,
    /// Successfully registered
    pub success_count: usize,
    /// Failed registrations
    pub failure_count: usize,
    /// Total components registered
    pub total_components: usize,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

fn default_empty_object() -> JsonValue {
    serde_json::json!({})
}

fn default_build_timeout() -> u64 {
    600
}

fn default_python_version() -> String {
    "3.11".to_string()
}

fn default_nodejs_version() -> String {
    "20".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_pack_request_defaults() {
        let json = r#"{
            "ref": "test-pack",
            "label": "Test Pack",
            "version": "1.0.0"
        }"#;

        let req: CreatePackRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.r#ref, "test-pack");
        assert_eq!(req.label, "Test Pack");
        assert_eq!(req.version, "1.0.0");
        assert!(req.tags.is_empty());
        assert!(req.runtime_deps.is_empty());
        assert!(req.dependencies.is_empty());
        assert!(!req.is_standard);
    }

    #[test]
    fn test_create_pack_request_validation() {
        let req = CreatePackRequest {
            r#ref: "".to_string(), // Invalid: empty
            label: "Test".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            conf_schema: default_empty_object(),
            config: default_empty_object(),
            meta: default_empty_object(),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
        };

        assert!(req.validate().is_err());
    }

    #[test]
    fn registry_response_redacts_header_values() {
        let redacted = redact_header_values(serde_json::json!({
            "Authorization": "Bearer secret",
            "X-Api-Key": "secret"
        }));
        assert_eq!(redacted["Authorization"], "[REDACTED]");
        assert_eq!(redacted["X-Api-Key"], "[REDACTED]");
        assert!(!redacted.to_string().contains("secret"));
    }
}
