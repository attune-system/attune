//! Dashboard DTOs for metadata, authoring, and data contract endpoints.

use std::{borrow::Cow, collections::BTreeMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

use attune_common::models::{dashboard::Dashboard, DashboardScopeType, DashboardVisibility};
use attune_common::schema::RefValidator;

use crate::dashboard_data::{AuthorizationBasis, FreshnessMode, SourceAvailability, SourceType};
use crate::dto::runtime::NullableStringPatch;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardMetadataResponse {
    pub id: i64,
    pub r#ref: String,
    pub scope_type: DashboardScopeType,
    pub scope_ref: String,
    pub pack: Option<i64>,
    pub owner_identity: Option<i64>,
    pub visibility: DashboardVisibility,
    pub is_adhoc: bool,
    pub label: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub is_default_home: bool,
    pub revision: i32,
    pub spec_version: i32,
    #[schema(value_type = Object)]
    pub spec: JsonValue,
    pub tags: Vec<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

impl From<Dashboard> for DashboardMetadataResponse {
    fn from(value: Dashboard) -> Self {
        Self {
            id: value.id,
            r#ref: value.r#ref,
            scope_type: value.scope_type,
            scope_ref: value.scope_ref,
            pack: value.pack,
            owner_identity: value.owner_identity,
            visibility: value.visibility,
            is_adhoc: value.is_adhoc,
            label: value.label,
            description: value.description,
            enabled: value.enabled,
            is_default_home: value.is_default_home,
            revision: value.revision,
            spec_version: value.spec_version,
            spec: value.spec,
            tags: value.tags,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardListItemResponse {
    pub id: i64,
    pub r#ref: String,
    pub label: String,
    pub description: Option<String>,
    pub scope_type: DashboardScopeType,
    pub scope_ref: String,
    pub visibility: DashboardVisibility,
    pub is_default_home: bool,
    pub revision: i32,
    pub tags: Vec<String>,
    pub updated: DateTime<Utc>,
}

impl From<Dashboard> for DashboardListItemResponse {
    fn from(value: Dashboard) -> Self {
        Self {
            id: value.id,
            r#ref: value.r#ref,
            label: value.label,
            description: value.description,
            scope_type: value.scope_type,
            scope_ref: value.scope_ref,
            visibility: value.visibility,
            is_default_home: value.is_default_home,
            revision: value.revision,
            tags: value.tags,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardSourceParamSchemaResponse {
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardSourceContractResponse {
    pub source_type: SourceType,
    pub availability: SourceAvailability,
    pub authorization_basis: AuthorizationBasis,
    pub default_freshness_mode: FreshnessMode,
    pub param_schema: DashboardSourceParamSchemaResponse,
    pub ordering: Vec<String>,
    pub response_shape: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardSourceCatalogResponse {
    pub source: String,
    pub contracts: Vec<DashboardSourceContractResponse>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(untagged)]
pub enum DashboardDescriptionPatch {
    Patch(NullableStringPatch),
    Value(String),
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDashboardRequest {
    #[validate(custom(function = "validate_dashboard_ref_field"))]
    #[schema(example = "core.operations_home")]
    pub r#ref: String,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Operations Home")]
    pub label: String,

    #[validate(length(min = 1))]
    #[schema(example = "Operational overview for the platform", nullable = true)]
    pub description: Option<String>,

    #[schema(example = "global", default = "global")]
    pub scope_type: DashboardScopeType,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "global", nullable = true)]
    pub scope_ref: Option<String>,

    #[schema(example = "public")]
    pub visibility: DashboardVisibility,

    #[schema(example = true, default = true, nullable = true)]
    pub enabled: Option<bool>,

    #[schema(example = false, default = false, nullable = true)]
    pub is_default_home: Option<bool>,

    #[validate(range(min = 1))]
    #[schema(example = 1, default = 1, nullable = true)]
    pub spec_version: Option<i32>,

    #[schema(value_type = Object)]
    pub spec: JsonValue,

    #[validate(custom(function = "validate_dashboard_tags"))]
    #[serde(default)]
    #[schema(example = json!(["operations", "overview"]), default = json!([]))]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDashboardRequest {
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "Operations Home (Updated)", nullable = true)]
    pub label: Option<String>,

    #[schema(nullable = true)]
    pub description: Option<DashboardDescriptionPatch>,

    #[schema(example = "pack", nullable = true)]
    pub scope_type: Option<DashboardScopeType>,

    #[validate(length(min = 1, max = 255))]
    #[schema(example = "core", nullable = true)]
    pub scope_ref: Option<String>,

    #[schema(example = "pack", nullable = true)]
    pub visibility: Option<DashboardVisibility>,

    #[schema(example = true, nullable = true)]
    pub enabled: Option<bool>,

    #[schema(example = false, nullable = true)]
    pub is_default_home: Option<bool>,

    #[validate(range(min = 1))]
    #[schema(example = 2, nullable = true)]
    pub spec_version: Option<i32>,

    #[schema(value_type = Object, nullable = true)]
    pub spec: Option<JsonValue>,

    #[validate(custom(function = "validate_dashboard_tags"))]
    #[schema(example = json!(["operations", "home"]), nullable = true)]
    pub tags: Option<Vec<String>>,

    #[validate(range(min = 1))]
    #[schema(example = 3)]
    pub expected_revision: i32,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneDashboardRequest {
    #[validate(custom(function = "validate_dashboard_ref_field"))]
    #[schema(example = "core.operations_home_copy")]
    pub r#ref: String,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewDashboardRequest {
    #[validate(nested)]
    pub dashboard: CreateDashboardRequest,

    #[validate(nested)]
    pub data_request: DashboardDataRequest,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DashboardDataRequest {
    #[serde(default)]
    #[schema(value_type = Object)]
    pub filters: BTreeMap<String, JsonValue>,

    #[validate(length(min = 2, max = 16))]
    #[schema(example = "24h")]
    pub time_window: Option<String>,

    pub time_range: Option<DashboardTimeRangeRequest>,

    #[validate(length(min = 1, max = 128))]
    #[schema(example = "America/Chicago")]
    pub timezone: Option<String>,

    #[schema(example = json!(["queue_backlog", "event_count"]))]
    /// Optional source selector.
    ///
    /// Membership only: request order is ignored. The response emits `sources[]`
    /// in canonical `source_id` ascending order.
    pub source_ids: Option<Vec<String>>,

    #[schema(example = json!(["overview_backlog", "event_rate"]))]
    pub card_ids: Option<Vec<String>>,

    #[serde(default = "default_include_meta")]
    pub include_meta: bool,

    #[validate(length(min = 1, max = 255))]
    pub request_id: Option<String>,
}

fn default_include_meta() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DashboardTimeRangeRequest {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardDataResponse {
    pub contract_version: i32,
    pub dashboard_ref: String,
    pub dashboard_revision: i32,
    pub spec_version: i32,
    pub resolved_at: DateTime<Utc>,
    pub request_id: Option<String>,
    pub effective_time_range: DashboardEffectiveTimeRange,
    pub partial: bool,
    /// Source results in canonical `source_id` ascending order.
    pub sources: Vec<DashboardSourceResult>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardEffectiveTimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardSourceResult {
    pub source_id: String,
    pub source_type: String,
    pub status: DashboardSourceStatus,
    #[schema(value_type = Object, nullable = true)]
    pub data: Option<JsonValue>,
    pub meta: DashboardSourceMeta,
    pub error: Option<DashboardSourceError>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardSourceStatus {
    Ok,
    Empty,
    Partial,
    Stale,
    Forbidden,
    Invalid,
    Error,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardSourceMeta {
    pub authorization_mode: DashboardAuthorizationMode,
    pub freshness_mode: DashboardFreshnessMode,
    pub aggregate_watermark: Option<DateTime<Utc>>,
    pub cache_hit: bool,
    pub bucket_size: Option<String>,
    pub truncated: bool,
    #[schema(value_type = Object)]
    pub unit_hints: JsonValue,
    pub ordering: Vec<String>,
    #[schema(value_type = Object, nullable = true)]
    pub authorized_refs: Option<JsonValue>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardAuthorizationMode {
    OperatorGlobal,
    IdentityFiltered,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DashboardFreshnessMode {
    RawOnly,
    AggregateOnly,
    AggregatePlusTail,
    RawOnlyFallback,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardSourceError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[schema(value_type = Object, nullable = true)]
    pub details: Option<JsonValue>,
}

fn validation_error(code: &'static str, message: String) -> ValidationError {
    let mut error = ValidationError::new(code);
    error.message = Some(Cow::Owned(message));
    error
}

fn validate_dashboard_ref_field(value: &str) -> Result<(), ValidationError> {
    RefValidator::validate_component_ref(value)
        .map_err(|e| validation_error("dashboard_ref", e.to_string()))
}

fn validate_dashboard_tags(values: &[String]) -> Result<(), ValidationError> {
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(validation_error(
                "dashboard_tag",
                "Dashboard tags cannot be empty".to_string(),
            ));
        }
        if trimmed.len() > 64 {
            return Err(validation_error(
                "dashboard_tag",
                "Dashboard tags must be at most 64 characters".to_string(),
            ));
        }
    }
    Ok(())
}
