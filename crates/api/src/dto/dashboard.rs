//! Dashboard DTOs for metadata and data contract endpoints.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::ToSchema;
use validator::Validate;

use attune_common::models::{dashboard::Dashboard, DashboardScopeType, DashboardVisibility};

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
