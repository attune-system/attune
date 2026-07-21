use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as JsonValue;
use utoipa::{IntoParams, ToSchema};

use attune_common::models::{WorkerRole, WorkerStatus, WorkerType};

use crate::dto::common::PaginationParams;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkerRuntimeSupport {
    #[schema(example = "python")]
    pub name: String,

    #[schema(example = json!(["3.12.1", "3.11.9"]))]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkerLoadSnapshot {
    #[schema(example = 0)]
    pub requested: u64,

    #[schema(example = 0)]
    pub scheduling: u64,

    #[schema(example = 1)]
    pub scheduled: u64,

    #[schema(example = 2)]
    pub running: u64,

    #[schema(example = 0)]
    pub canceling: u64,

    #[schema(example = 3)]
    pub total_active: u64,

    #[schema(example = 10, nullable = true)]
    pub max_concurrent_executions: Option<u32>,

    #[schema(example = 30, nullable = true)]
    pub utilization_percent: Option<u32>,

    #[schema(example = 1, nullable = true)]
    pub queue_depth: Option<i32>,

    #[schema(example = 2, nullable = true)]
    pub sensor_processes_running: Option<u64>,

    #[schema(example = 3, nullable = true)]
    pub sensor_processes_monitored: Option<u64>,

    #[schema(example = 4, nullable = true)]
    pub active_rules: Option<u64>,

    #[schema(example = 10, nullable = true)]
    pub max_concurrent_sensors: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CordonWorkerRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerHealthState {
    Active,
    Busy,
    Cordoned,
    Offline,
    Error,
    Inactive,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkerSummary {
    #[schema(example = 1)]
    pub id: i64,

    #[schema(example = "worker-build-01")]
    pub name: String,

    pub worker_type: WorkerType,

    pub worker_role: WorkerRole,

    #[schema(example = "worker-build-01", nullable = true)]
    pub host: Option<String>,

    #[schema(example = 8082, nullable = true)]
    pub port: Option<i32>,

    #[schema(nullable = true)]
    pub status: Option<WorkerStatus>,

    #[schema(example = "2026-04-11T13:26:37Z", nullable = true)]
    pub last_heartbeat: Option<DateTime<Utc>>,

    #[schema(example = 42, nullable = true)]
    pub heartbeat_age_seconds: Option<i64>,

    #[schema(example = false)]
    pub heartbeat_stale: bool,

    #[schema(example = false)]
    pub cordoned: bool,

    #[schema(nullable = true)]
    pub cordon_reason: Option<String>,

    #[schema(example = 1, nullable = true)]
    pub cordoned_by: Option<i64>,

    #[schema(example = "2026-04-11T13:26:37Z", nullable = true)]
    pub cordoned_at: Option<DateTime<Utc>>,

    pub health_state: WorkerHealthState,

    pub supported_runtimes: Vec<WorkerRuntimeSupport>,

    pub load: WorkerLoadSnapshot,

    #[schema(example = "2026-04-11T13:26:37Z")]
    pub created: DateTime<Utc>,

    #[schema(example = "2026-04-11T13:26:37Z")]
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct WorkerQueryParams {
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    /// Keyword query. Tokens are AND-matched across name, host, status,
    /// worker type, and worker role.
    #[param(example = "builder active")]
    pub q: Option<String>,

    #[serde(default)]
    pub role: Option<WorkerRole>,

    #[serde(default)]
    pub status: Option<WorkerStatus>,

    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub cordoned: Option<bool>,

    #[serde(default)]
    pub health_state: Option<WorkerHealthState>,
}

fn deserialize_optional_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|value| match value.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(serde::de::Error::custom(
                "provided string was not `true` or `false`",
            )),
        })
        .transpose()
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

impl WorkerQueryParams {
    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)) * self.page_size
    }

    pub fn limit(&self) -> u32 {
        self.page_size.min(100)
    }

    pub fn pagination(&self) -> PaginationParams {
        PaginationParams {
            page: self.page,
            page_size: self.page_size,
        }
    }
}

pub fn runtime_support_from_capabilities(
    capabilities: Option<&JsonValue>,
) -> Vec<WorkerRuntimeSupport> {
    let Some(capabilities) = capabilities.and_then(JsonValue::as_object) else {
        return Vec::new();
    };

    let mut versions_by_runtime = std::collections::BTreeMap::<String, Vec<String>>::new();

    if let Some(runtime_versions) = capabilities
        .get("runtime_versions")
        .and_then(JsonValue::as_object)
    {
        for (runtime_name, versions) in runtime_versions {
            let mut parsed_versions = versions
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>();
            parsed_versions.sort();
            parsed_versions.dedup();
            versions_by_runtime.insert(runtime_name.to_string(), parsed_versions);
        }
    }

    for runtime_name in capabilities
        .get("runtimes")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
    {
        versions_by_runtime
            .entry(runtime_name.to_string())
            .or_default();
    }

    versions_by_runtime
        .into_iter()
        .map(|(name, versions)| WorkerRuntimeSupport { name, versions })
        .collect()
}
