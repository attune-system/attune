//! Dashboard metadata and data contract routes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration as StdDuration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use validator::Validate;

use attune_common::{
    dashboard_spec::validate_dashboard_spec,
    models::{
        key::Key, Dashboard, DashboardScopeType, DashboardVisibility, ExecutionStatus, OwnerType,
        SensorProcessStatus, WorkerRole, WorkerStatus,
    },
    rbac::{Action as RbacAction, AuthorizationContext, Grant, Resource},
    repositories::dashboard::{
        CreateDashboardInput, CreateDashboardVersionInput, DashboardRepository, DashboardScopedRef,
        DashboardVersionRepository, UpdateDashboardInput,
    },
    repositories::{
        action::ActionRepository,
        analytics::AnalyticsTimeRange,
        rule::RuleRepository,
        runtime::WorkerRepository,
        sensor_process::SensorProcessRepository,
        trigger::TriggerRepository,
        work_queue::{WorkQueueItemRepository, WorkQueueRepository},
        AnalyticsRepository,
    },
    repositories::{Create, Delete, List, Patch, Update},
    schema::RefValidator,
};
use tokio::sync::{Mutex, Notify, Semaphore};
use tokio::time::timeout;
use tracing::{debug, info};

use crate::dashboard_data::contracts::{default_source_contracts, SourceContract, SourceType};
use crate::dashboard_data::watermark::{
    merge_bucket_rows_deterministic, BucketCountRow, TimeRange, WatermarkCutoverPlan,
};
use crate::dashboard_data::FreshnessMode;
use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::{AuthorizationCheck, AuthorizationService},
    dashboard_data::{ActionResultPathAllowList, SafeRef},
    dto::{
        dashboard::{
            CloneDashboardRequest, CreateDashboardRequest, DashboardAuthorizationMode,
            DashboardDataRequest, DashboardDataResponse, DashboardEffectiveTimeRange,
            DashboardFreshnessMode, DashboardListItemResponse, DashboardMetadataResponse,
            DashboardSourceCatalogResponse, DashboardSourceContractResponse, DashboardSourceError,
            DashboardSourceMeta, DashboardSourceParamSchemaResponse, DashboardSourceResult,
            DashboardSourceStatus, PreviewDashboardRequest, UpdateDashboardRequest,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

#[derive(Debug, Clone)]
struct DashboardFilterDef {
    filter_type: Option<String>,
    options: Option<Vec<JsonValue>>,
}

#[derive(Debug, Clone)]
struct DashboardSourceDef {
    source_id: String,
    source_type: String,
    source_params: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone)]
struct DashboardSpecIndex {
    defaults_time_window: Option<String>,
    defaults_timezone: Option<String>,
    filters: HashMap<String, DashboardFilterDef>,
    /// Canonical dashboard source order sorted by `source_id` ascending.
    ///
    /// This intentionally avoids relying on JSON object key iteration order from
    /// `spec.data_sources`, which is not a stable cross-language contract.
    sources_in_contract_order: Vec<DashboardSourceDef>,
    card_to_source: HashMap<String, String>,
    sources_from_cards_in_order: Vec<String>,
}

#[derive(Debug, Clone)]
struct DashboardWriteShape {
    r#ref: String,
    label: String,
    description: Option<String>,
    scope_type: DashboardScopeType,
    scope_ref: String,
    visibility: DashboardVisibility,
    enabled: bool,
    is_default_home: bool,
    spec_version: i32,
    spec: JsonValue,
    tags: Vec<String>,
    owner_identity: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
struct SourceAuthRequirement {
    resource: Resource,
    action: RbacAction,
}

#[derive(Debug, Clone)]
struct SourceRegistryEntry {
    required_auth: Option<SourceAuthRequirement>,
}

#[derive(Debug, Clone)]
struct DashboardSourceRegistry {
    entries: HashMap<&'static str, SourceRegistryEntry>,
}

#[derive(Debug, Clone, Default)]
struct RefFilterScope {
    pack_refs: Option<BTreeSet<String>>,
    action_refs: Option<BTreeSet<String>>,
    trigger_refs: Option<BTreeSet<String>>,
    rule_refs: Option<BTreeSet<String>>,
    queue_refs: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourcePrimaryRefKind {
    Action,
    Trigger,
    Rule,
    Queue,
}

impl SourcePrimaryRefKind {
    fn from_source_type(source_type: &str) -> Option<Self> {
        match source_type {
            "latest_action_result"
            | "action_result_path"
            | "execution_count"
            | "execution_timeseries"
            | "execution_status_breakdown"
            | "execution_duration_stats"
            | "last_execution" => Some(Self::Action),
            "event_count" | "event_timeseries" | "last_event" => Some(Self::Trigger),
            "enforcement_count" | "enforcement_timeseries" | "last_enforcement" => Some(Self::Rule),
            "queue_backlog" | "queue_throughput" | "queue_dispatch_stats" => Some(Self::Queue),
            _ => None,
        }
    }

    fn meta_key(self) -> &'static str {
        match self {
            Self::Action => "action_refs",
            Self::Trigger => "trigger_refs",
            Self::Rule => "rule_refs",
            Self::Queue => "queue_refs",
        }
    }
}

#[derive(Debug, Clone)]
struct SourceQueryScope {
    authorization_mode: DashboardAuthorizationMode,
    pack_refs: Option<BTreeSet<String>>,
    primary_ref_kind: Option<SourcePrimaryRefKind>,
    primary_refs: Option<BTreeSet<String>>,
}

impl SourceQueryScope {
    fn authorized_refs_json(&self) -> Option<JsonValue> {
        let mut object = serde_json::Map::new();
        if let Some(pack_refs) = &self.pack_refs {
            object.insert(
                "pack_refs".to_string(),
                JsonValue::Array(
                    pack_refs
                        .iter()
                        .map(|value| JsonValue::String(value.clone()))
                        .collect(),
                ),
            );
        }
        if let (Some(kind), Some(primary_refs)) = (self.primary_ref_kind, &self.primary_refs) {
            object.insert(
                kind.meta_key().to_string(),
                JsonValue::Array(
                    primary_refs
                        .iter()
                        .map(|value| JsonValue::String(value.clone()))
                        .collect(),
                ),
            );
        }
        if object.is_empty() {
            None
        } else {
            Some(JsonValue::Object(object))
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AuthzRefConstraints {
    unrestricted: bool,
    pack_refs: BTreeSet<String>,
    refs: BTreeSet<String>,
}

impl DashboardSourceRegistry {
    fn new() -> Self {
        let mut entries = HashMap::new();

        for source in [
            "latest_action_result",
            "action_result_path",
            "execution_count",
            "execution_timeseries",
            "execution_status_breakdown",
            "execution_duration_stats",
            "last_execution",
        ] {
            entries.insert(
                source,
                SourceRegistryEntry {
                    required_auth: Some(SourceAuthRequirement {
                        resource: Resource::Executions,
                        action: RbacAction::Read,
                    }),
                },
            );
        }

        for source in ["event_count", "event_timeseries", "last_event"] {
            entries.insert(
                source,
                SourceRegistryEntry {
                    required_auth: Some(SourceAuthRequirement {
                        resource: Resource::Events,
                        action: RbacAction::Read,
                    }),
                },
            );
        }

        for source in [
            "enforcement_count",
            "enforcement_timeseries",
            "last_enforcement",
        ] {
            entries.insert(
                source,
                SourceRegistryEntry {
                    required_auth: Some(SourceAuthRequirement {
                        resource: Resource::Enforcements,
                        action: RbacAction::Read,
                    }),
                },
            );
        }

        entries.insert(
            "key_value",
            SourceRegistryEntry {
                required_auth: Some(SourceAuthRequirement {
                    resource: Resource::Keys,
                    action: RbacAction::Read,
                }),
            },
        );
        entries.insert(
            "queue_backlog",
            SourceRegistryEntry {
                required_auth: Some(SourceAuthRequirement {
                    resource: Resource::QueueItems,
                    action: RbacAction::Read,
                }),
            },
        );
        entries.insert(
            "queue_dispatch_stats",
            SourceRegistryEntry {
                required_auth: Some(SourceAuthRequirement {
                    resource: Resource::Queues,
                    action: RbacAction::Read,
                }),
            },
        );
        entries.insert(
            "queue_throughput",
            SourceRegistryEntry {
                required_auth: Some(SourceAuthRequirement {
                    resource: Resource::QueueItems,
                    action: RbacAction::Read,
                }),
            },
        );
        entries.insert(
            "inquiry_backlog",
            SourceRegistryEntry {
                required_auth: Some(SourceAuthRequirement {
                    resource: Resource::Inquiries,
                    action: RbacAction::Read,
                }),
            },
        );
        entries.insert(
            "inquiry_sla",
            SourceRegistryEntry {
                required_auth: Some(SourceAuthRequirement {
                    resource: Resource::Inquiries,
                    action: RbacAction::Read,
                }),
            },
        );
        for source in ["worker_health", "worker_status"] {
            entries.insert(
                source,
                SourceRegistryEntry {
                    required_auth: Some(SourceAuthRequirement {
                        resource: Resource::Workers,
                        action: RbacAction::Read,
                    }),
                },
            );
        }
        entries.insert(
            "sensor_health",
            SourceRegistryEntry {
                required_auth: Some(SourceAuthRequirement {
                    resource: Resource::Workers,
                    action: RbacAction::Read,
                }),
            },
        );

        Self { entries }
    }

    fn get(&self, source_type: &str) -> Option<&SourceRegistryEntry> {
        self.entries.get(source_type)
    }
}

const SOURCE_TIMEOUT: StdDuration = StdDuration::from_secs(5);
const DASHBOARD_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(15);
const SOURCE_CONCURRENCY_LIMIT: usize = 16;
const MAX_CARDS_PER_DASHBOARD: usize = 40;
const MAX_SOURCE_DEFINITIONS_PER_DASHBOARD: usize = 60;
const MAX_SOURCES_PER_REQUEST: usize = 30;
const MAX_HIGH_COST_SOURCE_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;
const MAX_RAW_FALLBACK_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;
const SOURCE_ROW_CAP: usize = 2_000;
const SMALL_COHORT_MIN_COUNT: i64 = 2;
const SOURCE_CACHE_TTL: StdDuration = StdDuration::from_secs(30);
const SOURCE_CACHE_STALE_TTL: StdDuration = StdDuration::from_secs(120);
const SOURCE_CACHE_FAILURE_COALESCE_TTL: StdDuration = StdDuration::from_secs(2);
const SOURCE_CACHE_MAX_ENTRIES: usize = 1_000;
const SOURCE_INFLIGHT_WAIT_RECHECK: StdDuration = StdDuration::from_millis(100);
const SOURCE_INFLIGHT_WAIT_CAP: StdDuration = StdDuration::from_secs(6);
/// Dashboard contract terminal completion outcomes used by execution_count
/// and execution_status_breakdown default semantics.
const TERMINAL_EXECUTION_STATUSES: [&str; 5] =
    ["completed", "failed", "timeout", "cancelled", "abandoned"];
const TERMINAL_QUEUE_ITEM_STATUSES: [&str; 4] = ["completed", "failed", "skipped", "cancelled"];
const TERMINAL_QUEUE_DISPATCH_FALLBACK_STATUSES: [&str; 4] =
    ["completed", "failed", "released", "cancelled"];
const DEFAULT_INQUIRY_SLA_TARGET_SECONDS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceCostClass {
    HighCostRaw,
    RawFallbackBounded,
}

impl SourceCostClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::HighCostRaw => "high_cost_raw",
            Self::RawFallbackBounded => "raw_fallback_bounded",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SourceWindowBound {
    cost_class: SourceCostClass,
    max_window_seconds: i64,
    freshness_mode_hint: DashboardFreshnessMode,
}

#[derive(Debug, Clone, Copy)]
struct SourceWindowBoundViolation {
    cost_class: SourceCostClass,
    requested_window_seconds: i64,
    max_window_seconds: i64,
    freshness_mode_hint: DashboardFreshnessMode,
}

/// Canonical row contract for execution_count/execution_timeseries.
#[derive(Debug, Clone, serde::Serialize)]
struct BucketCountSourceRow {
    bucket_start: DateTime<Utc>,
    series: String,
    count: i64,
}

/// Canonical row contract for execution_status_breakdown.
#[derive(Debug, Clone, serde::Serialize)]
struct ExecutionStatusSourceRow {
    bucket_start: DateTime<Utc>,
    status: String,
    count: i64,
}

/// Canonical row contract for worker_health.
#[derive(Debug, Clone, serde::Serialize)]
struct WorkerHealthSourceRow {
    worker_id: i64,
    worker_name: String,
    worker_role: String,
    status: String,
    cordoned: bool,
}

/// Canonical row contract for queue_backlog.
#[derive(Debug, Clone, serde::Serialize)]
struct QueueBacklogSourceRow {
    queue_ref: String,
    queued: i64,
    retry: i64,
    leased: i64,
    total_backlog: i64,
}

/// Canonical row contract for queue_throughput.
#[derive(Debug, Clone, serde::Serialize)]
struct QueueThroughputSourceRow {
    bucket_start: DateTime<Utc>,
    queue_ref: String,
    completed: i64,
    failed: i64,
    skipped: i64,
    cancelled: i64,
    total_processed: i64,
}

/// Canonical row contract for queue_dispatch_stats.
#[derive(Debug, Clone, serde::Serialize)]
struct QueueDispatchStatsSourceRow {
    bucket_start: DateTime<Utc>,
    queue_ref: String,
    status: String,
    dispatch_count: i64,
    leased_item_count: i64,
    avg_duration_seconds: f64,
    max_duration_seconds: f64,
}

/// Canonical payload contract for key_value.
#[derive(Debug, Clone, serde::Serialize)]
struct KeyValueSourceData {
    r#ref: String,
    name: String,
    owner_type: String,
    owner_ref: Option<String>,
    encrypted: bool,
    decrypted: bool,
    value: JsonValue,
    updated_at: DateTime<Utc>,
}

/// Canonical row contract for latest_action_result.
#[derive(Debug, Clone, serde::Serialize)]
struct LatestActionResultSourceRow {
    action_ref: String,
    execution_id: i64,
    status: String,
    updated_at: DateTime<Utc>,
    result: Option<JsonValue>,
}

/// Canonical row contract for action_result_path.
#[derive(Debug, Clone, serde::Serialize)]
struct ActionResultPathSourceRow {
    action_ref: String,
    execution_id: i64,
    status: String,
    updated_at: DateTime<Utc>,
    path: String,
    value: JsonValue,
}

/// Canonical row contract for last_execution.
#[derive(Debug, Clone, serde::Serialize)]
struct LastExecutionSourceRow {
    action_ref: String,
    execution_id: i64,
    status: String,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    trace_tag: Option<String>,
    result: Option<JsonValue>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct LatestExecutionQueryRow {
    action_ref: String,
    execution_id: i64,
    status: ExecutionStatus,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    trace_tag: Option<String>,
    result: Option<JsonValue>,
}

/// Canonical row contract for inquiry_backlog.
#[derive(Debug, Clone, serde::Serialize)]
struct InquiryBacklogSourceRow {
    pack_ref: Option<String>,
    assigned_to: Option<i64>,
    pending_count: i64,
    overdue_count: i64,
}

/// Canonical row contract for inquiry_sla.
#[derive(Debug, Clone, serde::Serialize)]
struct InquirySlaSourceRow {
    bucket_start: DateTime<Utc>,
    pack_ref: Option<String>,
    assigned_to: Option<i64>,
    sla_target_seconds: i64,
    total_inquiries: i64,
    within_sla_count: i64,
    breached_count: i64,
    open_count: i64,
    compliance_rate: f64,
}

/// Canonical row contract for execution_duration_stats.
#[derive(Debug, Clone, serde::Serialize)]
struct ExecutionDurationStatsSourceRow {
    bucket_start: DateTime<Utc>,
    series: String,
    execution_count: i64,
    avg_duration_seconds: f64,
    p50_duration_seconds: f64,
    p95_duration_seconds: f64,
    max_duration_seconds: f64,
}

/// Canonical row contract for last_event.
#[derive(Debug, Clone, serde::Serialize)]
struct LastEventSourceRow {
    trigger_ref: String,
    event_id: i64,
    created: DateTime<Utc>,
    source_ref: Option<String>,
    rule_ref: Option<String>,
    trace_tag: Option<String>,
}

/// Canonical row contract for last_enforcement.
#[derive(Debug, Clone, serde::Serialize)]
struct LastEnforcementSourceRow {
    rule_ref: String,
    enforcement_id: i64,
    trigger_ref: String,
    status: String,
    created: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
    event_id: Option<i64>,
}

/// Canonical row contract for sensor_health.
#[derive(Debug, Clone, serde::Serialize)]
struct SensorHealthSourceRow {
    sensor_ref: String,
    worker_id: i64,
    worker_name: String,
    health: String,
    status: String,
    active_rule_count: i32,
    consecutive_failures: i32,
    pid: Option<i32>,
    last_started_at: Option<DateTime<Utc>>,
    last_stopped_at: Option<DateTime<Utc>>,
    next_restart_at: Option<DateTime<Utc>>,
    last_exit_code: Option<i32>,
    last_signal: Option<i32>,
    log_artifact_ref: Option<String>,
    updated: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct SourceCacheEntry {
    result: DashboardSourceResult,
    expires_at: Instant,
    stale_at: Instant,
    created_at: Instant,
}

#[derive(Debug)]
struct InflightEntry {
    notify: Notify,
    completed: AtomicBool,
}

impl InflightEntry {
    fn new() -> Self {
        Self {
            notify: Notify::new(),
            completed: AtomicBool::new(false),
        }
    }
}

#[derive(Debug)]
enum InflightRegistration {
    Leader,
    Waiter(Arc<InflightEntry>),
}

#[derive(Debug)]
struct DashboardSourceCache {
    entries: Mutex<HashMap<String, SourceCacheEntry>>,
    inflight: Mutex<HashMap<String, Arc<InflightEntry>>>,
}

impl DashboardSourceCache {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
        }
    }
}

static DASHBOARD_SOURCE_CACHE: OnceLock<DashboardSourceCache> = OnceLock::new();

fn source_cache() -> &'static DashboardSourceCache {
    DASHBOARD_SOURCE_CACHE.get_or_init(DashboardSourceCache::new)
}

#[utoipa::path(
    get,
    path = "/api/v1/dashboards",
    tag = "dashboards",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Visible dashboard summaries", body = ApiResponse<Vec<DashboardListItemResponse>>),
        (status = 401, description = "Unauthorized")
    )
)]
pub async fn list_dashboards(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
) -> ApiResult<impl IntoResponse> {
    let dashboards = DashboardRepository::list(&state.db).await?;
    let grants = AuthorizationService::new(state.db.clone())
        .effective_grants(&user)
        .await?;
    let identity_id = user.identity_id().ok();

    let mut visible = dashboards
        .into_iter()
        .filter(|dashboard| dashboard.enabled)
        .filter(|dashboard| {
            if dashboard.visibility != DashboardVisibility::Private {
                return true;
            }
            identity_id.is_some() && dashboard.owner_identity == identity_id
        })
        .filter(|dashboard| {
            let Some(id) = identity_id else {
                return false;
            };
            let mut ctx = AuthorizationContext::new(id);
            ctx.target_id = Some(dashboard.id);
            ctx.target_ref = Some(dashboard.r#ref.clone());
            ctx.pack_ref = dashboard
                .r#ref
                .split_once('.')
                .map(|(pack_ref, _)| pack_ref.to_string());
            ctx.owner_identity_id = dashboard.owner_identity;
            AuthorizationService::is_allowed(&grants, Resource::Dashboards, RbacAction::Read, &ctx)
        })
        .map(DashboardListItemResponse::from)
        .collect::<Vec<_>>();

    visible.sort_by(|a, b| {
        b.is_default_home
            .cmp(&a.is_default_home)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.r#ref.cmp(&b.r#ref))
    });

    Ok((StatusCode::OK, Json(ApiResponse::new(visible))))
}

#[utoipa::path(
    get,
    path = "/api/v1/dashboards/source-catalog",
    tag = "dashboards",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Dashboard source contract catalog", body = ApiResponse<DashboardSourceCatalogResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    )
)]
pub async fn get_dashboard_source_catalog(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
) -> ApiResult<impl IntoResponse> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;

    let context = AuthorizationContext::new(identity_id);
    AuthorizationService::new(state.db.clone())
        .authorize(
            &user,
            AuthorizationCheck {
                resource: Resource::Dashboards,
                action: RbacAction::Read,
                context,
            },
        )
        .await?;

    let contracts = default_source_contracts()
        .into_values()
        .map(|contract| DashboardSourceContractResponse {
            source_type: contract.source_type,
            availability: contract.availability,
            authorization_basis: contract.authorization_basis,
            default_freshness_mode: contract.default_freshness_mode,
            param_schema: DashboardSourceParamSchemaResponse {
                required: contract
                    .param_schema
                    .required
                    .into_iter()
                    .map(ToString::to_string)
                    .collect(),
                optional: contract
                    .param_schema
                    .optional
                    .into_iter()
                    .map(ToString::to_string)
                    .collect(),
            },
            ordering: contract
                .ordering
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            response_shape: contract.response_shape.to_string(),
            notes: contract.notes.map(ToString::to_string),
        })
        .collect::<Vec<_>>();

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(DashboardSourceCatalogResponse {
            source: "api".to_string(),
            contracts,
        })),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/dashboards",
    tag = "dashboards",
    request_body = CreateDashboardRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Dashboard created successfully", body = ApiResponse<DashboardMetadataResponse>),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 409, description = "Dashboard with same ref already exists in the target scope"),
        (status = 422, description = "Dashboard spec validation failed")
    )
)]
pub async fn create_dashboard(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateDashboardRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let shape = normalize_create_dashboard_request(&user, request)?;
    authorize_dashboard_create(&state, &user, &shape).await?;

    let mut tx = state.db.begin().await?;
    ensure_dashboard_scope_available(
        &mut *tx,
        &shape.r#ref,
        shape.scope_type,
        &shape.scope_ref,
        None,
    )
    .await?;
    clear_prior_default_home_if_needed(&mut tx, &shape, None, actor_identity_id(&user)?).await?;

    let dashboard = DashboardRepository::create(
        &mut *tx,
        CreateDashboardInput {
            r#ref: shape.r#ref.clone(),
            scope_type: shape.scope_type,
            scope_ref: shape.scope_ref.clone(),
            pack: None,
            owner_identity: shape.owner_identity,
            visibility: shape.visibility,
            is_adhoc: true,
            label: shape.label.clone(),
            description: shape.description.clone(),
            enabled: shape.enabled,
            is_default_home: shape.is_default_home,
            spec_version: shape.spec_version,
            spec: shape.spec.clone(),
            tags: shape.tags.clone(),
            created_by: actor_identity_id(&user)?,
        },
    )
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            DashboardMetadataResponse::from(dashboard),
            "Dashboard created successfully",
        )),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/dashboards/{ref}",
    tag = "dashboards",
    params(("ref" = String, Path, description = "Dashboard reference identifier")),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Dashboard metadata", body = ApiResponse<DashboardMetadataResponse>),
        (status = 400, description = "Invalid dashboard ref"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Dashboard not found")
    )
)]
pub async fn get_dashboard(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(dashboard_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let dashboard = resolve_dashboard_for_user(&state, &user, &dashboard_ref).await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(DashboardMetadataResponse::from(dashboard))),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/dashboards/{ref}",
    tag = "dashboards",
    params(("ref" = String, Path, description = "Dashboard reference identifier")),
    request_body = UpdateDashboardRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Dashboard updated successfully", body = ApiResponse<DashboardMetadataResponse>),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden or pack-managed dashboard"),
        (status = 404, description = "Dashboard not found"),
        (status = 409, description = "Revision mismatch or scope conflict"),
        (status = 422, description = "Dashboard spec validation failed")
    )
)]
pub async fn update_dashboard(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(dashboard_ref): Path<String>,
    Json(request): Json<UpdateDashboardRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let dashboard =
        resolve_dashboard_for_action(&state, &user, &dashboard_ref, RbacAction::Update).await?;
    if !dashboard.is_adhoc {
        return Err(ApiError::Forbidden(
            "Pack-managed dashboards must be updated in pack dashboard definition files"
                .to_string(),
        ));
    }
    if request.expected_revision != dashboard.revision {
        return Err(revision_conflict_error(
            &dashboard_ref,
            request.expected_revision,
            dashboard.revision,
        ));
    }

    let shape = normalize_update_dashboard_request(&user, &dashboard, request)?;
    if dashboard_matches_shape(&dashboard, &shape) {
        return Ok((
            StatusCode::OK,
            Json(ApiResponse::with_message(
                DashboardMetadataResponse::from(dashboard),
                "Dashboard updated successfully",
            )),
        ));
    }

    let mut tx = state.db.begin().await?;
    ensure_dashboard_scope_available(
        &mut *tx,
        &shape.r#ref,
        shape.scope_type,
        &shape.scope_ref,
        Some(dashboard.id),
    )
    .await?;
    clear_prior_default_home_if_needed(
        &mut tx,
        &shape,
        Some(dashboard.id),
        actor_identity_id(&user)?,
    )
    .await?;

    let updated = persist_dashboard_update(
        &mut tx,
        dashboard.id,
        dashboard.revision,
        &shape,
        true,
        actor_identity_id(&user)?,
        &dashboard_ref,
    )
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            DashboardMetadataResponse::from(updated),
            "Dashboard updated successfully",
        )),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/dashboards/{ref}",
    tag = "dashboards",
    params(("ref" = String, Path, description = "Dashboard reference identifier")),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Dashboard deleted successfully", body = SuccessResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden or pack-managed dashboard"),
        (status = 404, description = "Dashboard not found")
    )
)]
pub async fn delete_dashboard(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(dashboard_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let dashboard =
        resolve_dashboard_for_action(&state, &user, &dashboard_ref, RbacAction::Delete).await?;
    if !dashboard.is_adhoc {
        return Err(ApiError::Forbidden(
            "Pack-managed dashboards must be deleted from pack dashboard definition files"
                .to_string(),
        ));
    }

    let deleted = DashboardRepository::delete(&state.db, dashboard.id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Dashboard '{}' not found",
            dashboard_ref
        )));
    }

    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new(format!(
            "Dashboard '{}' deleted successfully",
            dashboard_ref
        ))),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/dashboards/{ref}/clone",
    tag = "dashboards",
    params(("ref" = String, Path, description = "Dashboard reference identifier")),
    request_body = CloneDashboardRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Dashboard cloned successfully", body = ApiResponse<DashboardMetadataResponse>),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Dashboard not found"),
        (status = 409, description = "Dashboard with same ref already exists in the target scope"),
        (status = 422, description = "Dashboard spec validation failed")
    )
)]
pub async fn clone_dashboard(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(dashboard_ref): Path<String>,
    Json(request): Json<CloneDashboardRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let source =
        resolve_dashboard_for_action(&state, &user, &dashboard_ref, RbacAction::Read).await?;
    let shape = normalize_clone_dashboard_request(&user, &source, request)?;
    authorize_dashboard_create(&state, &user, &shape).await?;

    let mut tx = state.db.begin().await?;
    ensure_dashboard_scope_available(
        &mut *tx,
        &shape.r#ref,
        shape.scope_type,
        &shape.scope_ref,
        None,
    )
    .await?;

    let dashboard = DashboardRepository::create(
        &mut *tx,
        CreateDashboardInput {
            r#ref: shape.r#ref.clone(),
            scope_type: shape.scope_type,
            scope_ref: shape.scope_ref.clone(),
            pack: None,
            owner_identity: shape.owner_identity,
            visibility: shape.visibility,
            is_adhoc: true,
            label: shape.label.clone(),
            description: shape.description.clone(),
            enabled: shape.enabled,
            is_default_home: false,
            spec_version: shape.spec_version,
            spec: shape.spec.clone(),
            tags: shape.tags.clone(),
            created_by: actor_identity_id(&user)?,
        },
    )
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            DashboardMetadataResponse::from(dashboard),
            "Dashboard cloned successfully",
        )),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/dashboards/preview",
    tag = "dashboards",
    request_body = PreviewDashboardRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Dashboard preview data envelope", body = DashboardDataResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 422, description = "Dashboard spec validation failed")
    )
)]
pub async fn preview_dashboard(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<PreviewDashboardRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    if request.data_request.time_window.is_some() && request.data_request.time_range.is_some() {
        return Err(ApiError::BadRequest(
            "time_window and time_range are mutually exclusive".to_string(),
        ));
    }

    let shape = normalize_create_dashboard_request(&user, request.dashboard)?;
    authorize_dashboard_preview(&state, &user, &shape).await?;

    let now = Utc::now();
    let preview_dashboard = Dashboard {
        id: 0,
        r#ref: shape.r#ref,
        scope_type: shape.scope_type,
        scope_ref: shape.scope_ref,
        pack: None,
        owner_identity: shape.owner_identity,
        visibility: shape.visibility,
        is_adhoc: true,
        label: shape.label,
        description: shape.description,
        enabled: shape.enabled,
        is_default_home: shape.is_default_home,
        revision: 0,
        spec_version: shape.spec_version,
        spec: shape.spec,
        tags: shape.tags,
        created: now,
        updated: now,
    };

    let response =
        execute_dashboard_data_request(&state, &user, preview_dashboard, request.data_request)
            .await?;
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    post,
    path = "/api/v1/dashboards/{ref}/data",
    tag = "dashboards",
    request_body = DashboardDataRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Dashboard source data envelope", body = DashboardDataResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Dashboard not found")
    )
)]
pub async fn get_dashboard_data(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(dashboard_ref): Path<String>,
    Json(request): Json<DashboardDataRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    if request.time_window.is_some() && request.time_range.is_some() {
        return Err(ApiError::BadRequest(
            "time_window and time_range are mutually exclusive".to_string(),
        ));
    }

    let dashboard = resolve_dashboard_for_user(&state, &user, &dashboard_ref).await?;
    let response = execute_dashboard_data_request(&state, &user, dashboard, request).await?;
    Ok((StatusCode::OK, Json(response)))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/dashboards", get(list_dashboards).post(create_dashboard))
        .route(
            "/dashboards/source-catalog",
            get(get_dashboard_source_catalog),
        )
        .route("/dashboards/preview", post(preview_dashboard))
        .route(
            "/dashboards/{ref}",
            get(get_dashboard)
                .put(update_dashboard)
                .delete(delete_dashboard),
        )
        .route("/dashboards/{ref}/clone", post(clone_dashboard))
        .route("/dashboards/{ref}/data", post(get_dashboard_data))
}

async fn execute_dashboard_data_request(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    dashboard: Dashboard,
    request: DashboardDataRequest,
) -> Result<DashboardDataResponse, ApiError> {
    let spec_index = index_dashboard_spec(&dashboard.spec)?;

    validate_request_filters(&request.filters, &spec_index.filters)?;
    let request_ref_scope = normalize_request_ref_scope(&request.filters)?;
    let effective_grants = AuthorizationService::new(state.db.clone())
        .effective_grants(user)
        .await?;

    let resolved_source_ids = resolve_requested_source_ids(&request, &spec_index)?;
    let effective_time_range = resolve_effective_time_range(&request, &spec_index)?;
    let request_deadline = tokio::time::Instant::now() + DASHBOARD_REQUEST_TIMEOUT;

    let registry = DashboardSourceRegistry::new();
    let source_semaphore = Arc::new(Semaphore::new(SOURCE_CONCURRENCY_LIMIT));
    let source_defs = spec_index.sources_in_contract_order;
    let mut source_results = vec![None; source_defs.len()];
    let mut pending_executions = FuturesUnordered::new();
    for (index, source_def) in source_defs.iter().enumerate() {
        if !resolved_source_ids.contains(&source_def.source_id) {
            continue;
        }

        let meta = default_source_meta();
        let Some(entry) = registry.get(&source_def.source_type) else {
            source_results[index] = Some(DashboardSourceResult {
                source_id: source_def.source_id.clone(),
                source_type: source_def.source_type.clone(),
                status: DashboardSourceStatus::Invalid,
                data: None,
                meta,
                error: Some(DashboardSourceError {
                    code: "unsupported".to_string(),
                    message: "No source handler registered for this source type".to_string(),
                    retryable: false,
                    details: None,
                }),
            });
            continue;
        };

        let source_scope = match resolve_source_query_scope(
            source_def,
            &request_ref_scope,
            &resolve_source_param_scope(source_def, &request.filters)?,
            &effective_grants,
            entry.required_auth,
        ) {
            Ok(scope) => scope,
            Err(ApiError::Forbidden(_) | ApiError::Unauthorized(_)) => {
                source_results[index] = Some(DashboardSourceResult {
                    source_id: source_def.source_id.clone(),
                    source_type: source_def.source_type.clone(),
                    status: DashboardSourceStatus::Forbidden,
                    data: None,
                    meta,
                    error: Some(DashboardSourceError {
                        code: "forbidden".to_string(),
                        message: "Not authorized to read this source".to_string(),
                        retryable: false,
                        details: None,
                    }),
                });
                continue;
            }
            Err(err) => return Err(err),
        };

        if let Err(violation) =
            validate_source_window_bounds(&source_def.source_type, &effective_time_range)
        {
            source_results[index] = Some(source_window_bound_violation_result(
                source_def,
                &effective_time_range,
                violation,
            ));
            continue;
        }

        let state = state.clone();
        let user = user.clone();
        let dashboard = dashboard.clone();
        let source = source_def.clone();
        let request_filters = request.filters.clone();
        let effective_time_range = effective_time_range.clone();
        let source_semaphore = source_semaphore.clone();
        let effective_grants = effective_grants.clone();
        let include_meta = request.include_meta;
        pending_executions.push(async move {
            let result = execute_source_data(
                &state,
                &user,
                &effective_grants,
                &dashboard,
                &source,
                &request_filters,
                source_scope,
                &effective_time_range,
                include_meta,
                source_semaphore,
                request_deadline,
            )
            .await;
            (index, result)
        });
    }

    while let Some((index, result)) = pending_executions.next().await {
        source_results[index] = Some(result);
    }

    let source_results = source_results.into_iter().flatten().collect::<Vec<_>>();
    let partial = source_results.iter().any(|source| {
        !matches!(
            source.status,
            DashboardSourceStatus::Ok | DashboardSourceStatus::Empty
        )
    });

    Ok(DashboardDataResponse {
        contract_version: 1,
        dashboard_ref: dashboard.r#ref,
        dashboard_revision: dashboard.revision,
        spec_version: dashboard.spec_version,
        resolved_at: Utc::now(),
        request_id: request.request_id,
        effective_time_range,
        partial,
        sources: source_results,
    })
}

fn default_source_meta() -> DashboardSourceMeta {
    DashboardSourceMeta {
        authorization_mode: DashboardAuthorizationMode::OperatorGlobal,
        freshness_mode: DashboardFreshnessMode::RawOnly,
        aggregate_watermark: None,
        cache_hit: false,
        bucket_size: None,
        truncated: false,
        unit_hints: JsonValue::Object(Default::default()),
        ordering: Vec::new(),
        authorized_refs: None,
    }
}

fn serialized_row<T: serde::Serialize>(row: T) -> JsonValue {
    serde_json::to_value(row).unwrap_or(JsonValue::Null)
}

fn json_object<const N: usize>(entries: [(&str, &str); N]) -> JsonValue {
    let mut object = serde_json::Map::new();
    for (key, value) in entries {
        object.insert(key.to_string(), JsonValue::String(value.to_string()));
    }
    JsonValue::Object(object)
}

fn worker_role_label(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Action => "action",
        WorkerRole::Sensor => "sensor",
    }
}

fn worker_status_label(status: WorkerStatus) -> &'static str {
    match status {
        WorkerStatus::Active => "active",
        WorkerStatus::Inactive => "inactive",
        WorkerStatus::Busy => "busy",
        WorkerStatus::Error => "error",
    }
}

fn execution_status_label(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Requested => "requested",
        ExecutionStatus::Scheduling => "scheduling",
        ExecutionStatus::Scheduled => "scheduled",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Canceling => "canceling",
        ExecutionStatus::Cancelled => "cancelled",
        ExecutionStatus::Timeout => "timeout",
        ExecutionStatus::Abandoned => "abandoned",
    }
}

fn owner_type_label(owner_type: OwnerType) -> &'static str {
    match owner_type {
        OwnerType::System => "system",
        OwnerType::Identity => "identity",
        OwnerType::Pack => "pack",
        OwnerType::Action => "action",
        OwnerType::Sensor => "sensor",
    }
}

fn sensor_process_status_label(status: SensorProcessStatus) -> &'static str {
    match status {
        SensorProcessStatus::Starting => "starting",
        SensorProcessStatus::Running => "running",
        SensorProcessStatus::Stopped => "stopped",
        SensorProcessStatus::Failed => "failed",
        SensorProcessStatus::Backoff => "backoff",
    }
}

fn sensor_process_health_label(status: SensorProcessStatus) -> &'static str {
    match status {
        SensorProcessStatus::Running => "healthy",
        SensorProcessStatus::Starting | SensorProcessStatus::Stopped => "degraded",
        SensorProcessStatus::Failed | SensorProcessStatus::Backoff => "unhealthy",
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_source_data(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    effective_grants: &[Grant],
    dashboard: &Dashboard,
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    source_scope: SourceQueryScope,
    effective_time_range: &DashboardEffectiveTimeRange,
    include_meta: bool,
    source_semaphore: Arc<Semaphore>,
    request_deadline: tokio::time::Instant,
) -> DashboardSourceResult {
    let started = Instant::now();
    let cache_key = build_source_cache_key(
        dashboard,
        user,
        source,
        request_filters,
        effective_time_range,
    );

    loop {
        if let Some(mut cached) = source_cache().get_fresh(&cache_key).await {
            cached.meta.cache_hit = include_meta;
            emit_source_telemetry(dashboard, &cached, started, false);
            return cached;
        }

        match source_cache().register_inflight(&cache_key).await {
            InflightRegistration::Leader => break,
            InflightRegistration::Waiter(waiter) => {
                source_cache()
                    .wait_for_inflight(&cache_key, waiter, request_deadline)
                    .await;
            }
        }
    }

    let mut result = match source_semaphore.acquire().await {
        Ok(_permit) => {
            if let Some(source_timeout_budget) = resolve_source_timeout_budget(request_deadline) {
                let execution = timeout(
                    source_timeout_budget,
                    execute_source_handler(
                        state,
                        user,
                        effective_grants,
                        source,
                        request_filters,
                        &source_scope,
                        effective_time_range,
                    ),
                )
                .await;

                match execution {
                    Ok(Ok(mut executed)) => {
                        if !include_meta {
                            executed.meta.cache_hit = false;
                        }
                        source_cache()
                            .insert(cache_key.clone(), executed.clone())
                            .await;
                        executed
                    }
                    Ok(Err(error)) => {
                        if let Some(mut stale) = source_cache().get_stale(&cache_key).await {
                            stale.status = DashboardSourceStatus::Stale;
                            stale.meta.cache_hit = include_meta;
                            stale.meta.freshness_mode = DashboardFreshnessMode::RawOnlyFallback;
                            stale.error = Some(DashboardSourceError {
                                code: "fallback_cache".to_string(),
                                message: format!("Source failed; returning stale cache: {}", error),
                                retryable: true,
                                details: None,
                            });
                            stale
                        } else {
                            DashboardSourceResult {
                                source_id: source.source_id.clone(),
                                source_type: source.source_type.clone(),
                                status: DashboardSourceStatus::Error,
                                data: None,
                                meta: default_source_meta(),
                                error: Some(DashboardSourceError {
                                    code: "source_error".to_string(),
                                    message: error.to_string(),
                                    retryable: true,
                                    details: None,
                                }),
                            }
                        }
                    }
                    Err(_) => {
                        if let Some(mut stale) = source_cache().get_stale(&cache_key).await {
                            stale.status = DashboardSourceStatus::Stale;
                            stale.meta.cache_hit = include_meta;
                            stale.meta.freshness_mode = DashboardFreshnessMode::RawOnlyFallback;
                            stale.error = Some(DashboardSourceError {
                                code: "timeout_fallback".to_string(),
                                message: "Source timed out; returning stale cache".to_string(),
                                retryable: true,
                                details: None,
                            });
                            stale
                        } else {
                            DashboardSourceResult {
                                source_id: source.source_id.clone(),
                                source_type: source.source_type.clone(),
                                status: DashboardSourceStatus::Error,
                                data: None,
                                meta: default_source_meta(),
                                error: Some(DashboardSourceError {
                                    code: "timeout".to_string(),
                                    message: "Source execution exceeded timeout budget".to_string(),
                                    retryable: true,
                                    details: None,
                                }),
                            }
                        }
                    }
                }
            } else {
                DashboardSourceResult {
                    source_id: source.source_id.clone(),
                    source_type: source.source_type.clone(),
                    status: DashboardSourceStatus::Error,
                    data: None,
                    meta: default_source_meta(),
                    error: Some(DashboardSourceError {
                        code: "request_timeout".to_string(),
                        message: "Dashboard request exceeded overall timeout budget".to_string(),
                        retryable: true,
                        details: None,
                    }),
                }
            }
        }
        Err(_) => unsupported_source_result(source, "semaphore"),
    };

    if should_coalesce_failure(&result) {
        source_cache()
            .insert_retryable_failure(cache_key.clone(), result.clone())
            .await;
    }

    source_cache().complete_inflight(&cache_key).await;
    emit_source_telemetry(dashboard, &result, started, true);
    if !include_meta {
        result.meta.cache_hit = false;
    }
    result
}

fn should_coalesce_failure(result: &DashboardSourceResult) -> bool {
    matches!(result.status, DashboardSourceStatus::Error)
        && result.error.as_ref().is_some_and(|error| error.retryable)
}

fn resolve_inflight_wait_budget(request_deadline: tokio::time::Instant) -> Option<StdDuration> {
    let now = tokio::time::Instant::now();
    if request_deadline <= now {
        return None;
    }
    Some(SOURCE_INFLIGHT_WAIT_CAP.min(request_deadline.saturating_duration_since(now)))
}

impl DashboardSourceCache {
    async fn wait_for_inflight(
        &self,
        key: &str,
        waiter: Arc<InflightEntry>,
        request_deadline: tokio::time::Instant,
    ) {
        let Some(mut remaining) = resolve_inflight_wait_budget(request_deadline) else {
            return;
        };

        loop {
            if waiter.completed.load(Ordering::Acquire) {
                return;
            }
            if !self.is_same_inflight(key, &waiter).await {
                return;
            }
            if remaining.is_zero() {
                return;
            }

            let wait_slice = SOURCE_INFLIGHT_WAIT_RECHECK.min(remaining);
            let _ = timeout(wait_slice, waiter.notify.notified()).await;
            remaining = remaining.saturating_sub(wait_slice);
        }
    }

    async fn is_same_inflight(&self, key: &str, expected: &Arc<InflightEntry>) -> bool {
        let inflight = self.inflight.lock().await;
        inflight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, expected))
    }

    async fn register_inflight(&self, key: &str) -> InflightRegistration {
        let mut inflight = self.inflight.lock().await;
        if let Some(waiter) = inflight.get(key) {
            return InflightRegistration::Waiter(waiter.clone());
        }

        inflight.insert(key.to_string(), Arc::new(InflightEntry::new()));
        InflightRegistration::Leader
    }

    async fn complete_inflight(&self, key: &str) {
        let inflight = {
            let mut inflight = self.inflight.lock().await;
            inflight.remove(key)
        };
        if let Some(inflight) = inflight {
            inflight.completed.store(true, Ordering::Release);
            inflight.notify.notify_waiters();
        }
    }

    async fn insert_retryable_failure(&self, key: String, result: DashboardSourceResult) {
        self.insert_with_ttls(
            key,
            result,
            SOURCE_CACHE_FAILURE_COALESCE_TTL,
            SOURCE_CACHE_FAILURE_COALESCE_TTL,
        )
        .await;
    }

    async fn insert_with_ttls(
        &self,
        key: String,
        result: DashboardSourceResult,
        fresh_ttl: StdDuration,
        stale_ttl: StdDuration,
    ) {
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        entries.insert(
            key,
            SourceCacheEntry {
                result,
                expires_at: now + fresh_ttl,
                stale_at: now + stale_ttl,
                created_at: now,
            },
        );

        if entries.len() > SOURCE_CACHE_MAX_ENTRIES {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.created_at)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
                debug!(cache_key = %oldest, "Evicted dashboard source cache entry");
            }
        }
    }
}

fn build_source_cache_key(
    dashboard: &Dashboard,
    user: &AuthenticatedUser,
    source: &DashboardSourceDef,
    filters: &BTreeMap<String, JsonValue>,
    range: &DashboardEffectiveTimeRange,
) -> String {
    let identity_id = user.identity_id().unwrap_or_default();
    let filters_json = serde_json::to_string(filters).unwrap_or_else(|_| "{}".to_string());
    let spec_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&dashboard.spec).unwrap_or_default(),
    ));
    format!(
        "{}|rev:{}|spec:{}|scope:{:?}:{}|owner:{}|viewer:{}|auth_iat:{}|source:{}:{}|tz:{}|{}|{}|filters:{}",
        dashboard.r#ref,
        dashboard.revision,
        spec_hash,
        dashboard.scope_type,
        dashboard.scope_ref,
        dashboard.owner_identity.unwrap_or_default(),
        identity_id,
        user.claims.iat,
        source.source_id,
        source.source_type,
        range.timezone,
        range.start.timestamp(),
        range.end.timestamp(),
        filters_json
    )
}

fn emit_source_telemetry(
    dashboard: &Dashboard,
    result: &DashboardSourceResult,
    started: Instant,
    executed: bool,
) {
    let duration_ms = started.elapsed().as_millis() as u64;
    info!(
        dashboard_ref = %dashboard.r#ref,
        source_id = %result.source_id,
        source_type = %result.source_type,
        status = ?result.status,
        cache_hit = result.meta.cache_hit,
        freshness_mode = ?result.meta.freshness_mode,
        truncated = result.meta.truncated,
        duration_ms,
        executed,
        error_code = result.error.as_ref().map(|err| err.code.as_str()).unwrap_or("none"),
        "dashboard source execution completed"
    );
}

#[derive(Debug, Clone, Copy)]
enum BucketedCutoverKind {
    ExecutionThroughput,
    ExecutionStatus,
    EventVolume,
}

#[derive(Debug, Clone)]
struct BucketedSourceExecution {
    data: Vec<BucketCountRow>,
    freshness_mode: DashboardFreshnessMode,
    aggregate_watermark: Option<DateTime<Utc>>,
}

async fn execute_execution_throughput_with_cutover(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    action_refs: Option<&BTreeSet<String>>,
) -> Result<BucketedSourceExecution, ApiError> {
    execute_bucketed_source_with_cutover(
        state,
        effective_time_range,
        BucketedCutoverKind::ExecutionThroughput,
        action_refs,
    )
    .await
}

async fn execute_execution_status_with_cutover(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    action_refs: Option<&BTreeSet<String>>,
) -> Result<BucketedSourceExecution, ApiError> {
    execute_bucketed_source_with_cutover(
        state,
        effective_time_range,
        BucketedCutoverKind::ExecutionStatus,
        action_refs,
    )
    .await
}

async fn execute_event_volume_with_cutover(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    trigger_refs: Option<&BTreeSet<String>>,
) -> Result<BucketedSourceExecution, ApiError> {
    execute_bucketed_source_with_cutover(
        state,
        effective_time_range,
        BucketedCutoverKind::EventVolume,
        trigger_refs,
    )
    .await
}

async fn execute_bucketed_source_with_cutover(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    kind: BucketedCutoverKind,
    primary_refs: Option<&BTreeSet<String>>,
) -> Result<BucketedSourceExecution, ApiError> {
    let request_range = TimeRange::new(effective_time_range.start, effective_time_range.end)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let aggregate_watermark =
        fetch_aggregate_watermark(state, kind.aggregate_watermark_view_name()).await;
    let plan = WatermarkCutoverPlan::build(request_range, aggregate_watermark)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;

    let aggregate_rows = if let Some(range) = plan.aggregate_range {
        query_aggregate_bucket_rows(state, kind, range, primary_refs).await?
    } else {
        Vec::new()
    };
    let raw_rows = if let Some(range) = plan.raw_range {
        query_raw_bucket_rows(state, kind, range, primary_refs).await?
    } else {
        Vec::new()
    };
    let merged = merge_bucket_rows_deterministic(&plan, &aggregate_rows, &raw_rows);

    Ok(BucketedSourceExecution {
        data: merged,
        freshness_mode: map_freshness_mode(plan.freshness_mode),
        aggregate_watermark: plan.aggregate_watermark,
    })
}

impl BucketedCutoverKind {
    #[cfg(test)]
    fn for_source_type(source_type: &str) -> Option<Self> {
        match source_type {
            "execution_count" | "execution_timeseries" => Some(Self::ExecutionThroughput),
            "execution_status_breakdown" => Some(Self::ExecutionStatus),
            "event_count" | "event_timeseries" => Some(Self::EventVolume),
            _ => None,
        }
    }

    fn aggregate_query_view_name(self) -> &'static str {
        match self {
            Self::ExecutionThroughput | Self::ExecutionStatus => "execution_status_hourly",
            Self::EventVolume => "event_volume_hourly",
        }
    }

    fn aggregate_watermark_view_name(self) -> &'static str {
        self.aggregate_query_view_name()
    }
}

async fn fetch_aggregate_watermark(
    state: &Arc<AppState>,
    aggregate_view_name: &str,
) -> Option<DateTime<Utc>> {
    let watermark = sqlx::query_as::<_, (Option<DateTime<Utc>>, )>(
        r#"
        SELECT _timescaledb_functions.to_timestamp(_timescaledb_functions.cagg_watermark(ht.id)) AS watermark
        FROM timescaledb_information.continuous_aggregates cagg
        INNER JOIN _timescaledb_catalog.hypertable ht
            ON ht.schema_name = cagg.materialization_hypertable_schema
           AND ht.table_name = cagg.materialization_hypertable_name
        WHERE cagg.view_schema = current_schema()
          AND cagg.view_name = $1
        LIMIT 1
        "#,
    )
    .bind(aggregate_view_name)
    .fetch_optional(&state.db)
    .await;

    match watermark {
        Ok(Some((watermark,))) => watermark,
        Ok(None) => None,
        Err(error) => {
            debug!(
                aggregate_view_name,
                error = %error,
                "dashboard source watermark unavailable; falling back to raw path"
            );
            None
        }
    }
}

async fn query_aggregate_bucket_rows(
    state: &Arc<AppState>,
    kind: BucketedCutoverKind,
    range: TimeRange,
    primary_refs: Option<&BTreeSet<String>>,
) -> Result<Vec<BucketCountRow>, ApiError> {
    let rows = match kind {
        BucketedCutoverKind::ExecutionThroughput => {
            let aggregate_view_name = kind.aggregate_query_view_name();
            if let Some(action_refs) = primary_refs {
                let action_refs: Vec<String> = action_refs.iter().cloned().collect();
                let query = format!(
                    r#"
                    SELECT
                        bucket AS bucket_start,
                        action_ref AS series,
                        SUM(transition_count)::bigint AS count
                    FROM {aggregate_view_name}
                    WHERE bucket >= $1
                      AND bucket < $2
                      AND action_ref = ANY($3::text[])
                      AND new_status = ANY($4::text[])
                    GROUP BY bucket, action_ref
                    ORDER BY bucket ASC, action_ref ASC
                    "#,
                );
                let rows = sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(&query)
                    .bind(range.start)
                    .bind(range.end)
                    .bind(action_refs)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(&state.db)
                    .await?;
                rows.into_iter()
                    .map(|(bucket_start, series, count)| BucketCountRow {
                        bucket_start,
                        series,
                        count,
                    })
                    .collect()
            } else {
                let query = format!(
                    r#"
                    SELECT
                        bucket AS bucket_start,
                        SUM(transition_count)::bigint AS count
                    FROM {aggregate_view_name}
                    WHERE bucket >= $1 AND bucket < $2
                      AND new_status = ANY($3::text[])
                    GROUP BY bucket
                    ORDER BY bucket ASC
                    "#,
                );
                let rows = sqlx::query_as::<_, (DateTime<Utc>, i64)>(&query)
                    .bind(range.start)
                    .bind(range.end)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(&state.db)
                    .await?;
                rows.into_iter()
                    .map(|(bucket_start, count)| BucketCountRow {
                        bucket_start,
                        series: "all".to_string(),
                        count,
                    })
                    .collect()
            }
        }
        BucketedCutoverKind::ExecutionStatus => {
            let aggregate_view_name = kind.aggregate_query_view_name();
            let rows = if let Some(action_refs) = primary_refs {
                let action_refs: Vec<String> = action_refs.iter().cloned().collect();
                let query = format!(
                    r#"
                    SELECT
                        bucket AS bucket_start,
                        COALESCE(new_status, 'unknown') AS series,
                        SUM(transition_count)::bigint AS count
                    FROM {aggregate_view_name}
                    WHERE bucket >= $1
                      AND bucket < $2
                      AND action_ref = ANY($3::text[])
                      AND new_status = ANY($4::text[])
                    GROUP BY bucket, new_status
                    ORDER BY bucket ASC, series ASC
                    "#,
                );
                sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(&query)
                    .bind(range.start)
                    .bind(range.end)
                    .bind(action_refs)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(&state.db)
                    .await?
            } else {
                let query = format!(
                    r#"
                    SELECT
                        bucket AS bucket_start,
                        COALESCE(new_status, 'unknown') AS series,
                        SUM(transition_count)::bigint AS count
                    FROM {aggregate_view_name}
                    WHERE bucket >= $1
                      AND bucket < $2
                      AND new_status = ANY($3::text[])
                    GROUP BY bucket, new_status
                    ORDER BY bucket ASC, series ASC
                    "#,
                );
                sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(&query)
                    .bind(range.start)
                    .bind(range.end)
                    .bind(TERMINAL_EXECUTION_STATUSES)
                    .fetch_all(&state.db)
                    .await?
            };
            rows.into_iter()
                .map(|(bucket_start, series, count)| BucketCountRow {
                    bucket_start,
                    series,
                    count,
                })
                .collect()
        }
        BucketedCutoverKind::EventVolume => {
            let aggregate_view_name = kind.aggregate_query_view_name();
            if let Some(trigger_refs) = primary_refs {
                let trigger_refs: Vec<String> = trigger_refs.iter().cloned().collect();
                let query = format!(
                    r#"
                    SELECT
                        bucket AS bucket_start,
                        trigger_ref AS series,
                        SUM(event_count)::bigint AS count
                    FROM {aggregate_view_name}
                    WHERE bucket >= $1
                      AND bucket < $2
                      AND trigger_ref = ANY($3::text[])
                    GROUP BY bucket, trigger_ref
                    ORDER BY bucket ASC, trigger_ref ASC
                    "#,
                );
                let rows = sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(&query)
                    .bind(range.start)
                    .bind(range.end)
                    .bind(trigger_refs)
                    .fetch_all(&state.db)
                    .await?;
                rows.into_iter()
                    .map(|(bucket_start, series, count)| BucketCountRow {
                        bucket_start,
                        series,
                        count,
                    })
                    .collect()
            } else {
                let query = format!(
                    r#"
                    SELECT
                        bucket AS bucket_start,
                        SUM(event_count)::bigint AS count
                    FROM {aggregate_view_name}
                    WHERE bucket >= $1 AND bucket < $2
                    GROUP BY bucket
                    ORDER BY bucket ASC
                    "#,
                );
                let rows = sqlx::query_as::<_, (DateTime<Utc>, i64)>(&query)
                    .bind(range.start)
                    .bind(range.end)
                    .fetch_all(&state.db)
                    .await?;
                rows.into_iter()
                    .map(|(bucket_start, count)| BucketCountRow {
                        bucket_start,
                        series: "all".to_string(),
                        count,
                    })
                    .collect()
            }
        }
    };
    Ok(rows)
}

async fn query_raw_bucket_rows(
    state: &Arc<AppState>,
    kind: BucketedCutoverKind,
    range: TimeRange,
    primary_refs: Option<&BTreeSet<String>>,
) -> Result<Vec<BucketCountRow>, ApiError> {
    let rows = match kind {
        BucketedCutoverKind::ExecutionThroughput => {
            if let Some(action_refs) = primary_refs {
                let action_refs: Vec<String> = action_refs.iter().cloned().collect();
                let rows = sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                    r#"
                    SELECT
                        date_trunc('hour', time) AS bucket_start,
                        entity_ref AS series,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND entity_ref = ANY($3::text[])
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($4::text[])
                    GROUP BY bucket_start, entity_ref
                    ORDER BY bucket_start ASC, entity_ref ASC
                    "#,
                )
                .bind(range.start)
                .bind(range.end)
                .bind(action_refs)
                .bind(TERMINAL_EXECUTION_STATUSES)
                .fetch_all(&state.db)
                .await?;
                rows.into_iter()
                    .map(|(bucket_start, series, count)| BucketCountRow {
                        bucket_start,
                        series,
                        count,
                    })
                    .collect()
            } else {
                let rows = sqlx::query_as::<_, (DateTime<Utc>, i64)>(
                    r#"
                    SELECT
                        date_trunc('hour', time) AS bucket_start,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($3::text[])
                    GROUP BY bucket_start
                    ORDER BY bucket_start ASC
                    "#,
                )
                .bind(range.start)
                .bind(range.end)
                .bind(TERMINAL_EXECUTION_STATUSES)
                .fetch_all(&state.db)
                .await?;
                rows.into_iter()
                    .map(|(bucket_start, count)| BucketCountRow {
                        bucket_start,
                        series: "all".to_string(),
                        count,
                    })
                    .collect()
            }
        }
        BucketedCutoverKind::ExecutionStatus => {
            let rows = if let Some(action_refs) = primary_refs {
                let action_refs: Vec<String> = action_refs.iter().cloned().collect();
                sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                    r#"
                    SELECT
                        date_trunc('hour', time) AS bucket_start,
                        COALESCE(new_values->>'status', 'unknown') AS series,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND entity_ref = ANY($3::text[])
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($4::text[])
                    GROUP BY bucket_start, COALESCE(new_values->>'status', 'unknown')
                    ORDER BY bucket_start ASC, series ASC
                    "#,
                )
                .bind(range.start)
                .bind(range.end)
                .bind(action_refs)
                .bind(TERMINAL_EXECUTION_STATUSES)
                .fetch_all(&state.db)
                .await?
            } else {
                sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                    r#"
                    SELECT
                        date_trunc('hour', time) AS bucket_start,
                        COALESCE(new_values->>'status', 'unknown') AS series,
                        COUNT(*)::bigint AS count
                    FROM execution_history
                    WHERE 'status' = ANY(changed_fields)
                      AND time >= $1
                      AND time < $2
                      AND COALESCE(new_values->>'status', 'unknown') = ANY($3::text[])
                    GROUP BY bucket_start, COALESCE(new_values->>'status', 'unknown')
                    ORDER BY bucket_start ASC, series ASC
                    "#,
                )
                .bind(range.start)
                .bind(range.end)
                .bind(TERMINAL_EXECUTION_STATUSES)
                .fetch_all(&state.db)
                .await?
            };
            rows.into_iter()
                .map(|(bucket_start, series, count)| BucketCountRow {
                    bucket_start,
                    series,
                    count,
                })
                .collect()
        }
        BucketedCutoverKind::EventVolume => {
            if let Some(trigger_refs) = primary_refs {
                let trigger_refs: Vec<String> = trigger_refs.iter().cloned().collect();
                let rows = sqlx::query_as::<_, (DateTime<Utc>, String, i64)>(
                    r#"
                    SELECT
                        date_trunc('hour', created) AS bucket_start,
                        trigger_ref AS series,
                        COUNT(*)::bigint AS count
                    FROM event
                    WHERE created >= $1
                      AND created < $2
                      AND trigger_ref = ANY($3::text[])
                    GROUP BY bucket_start, trigger_ref
                    ORDER BY bucket_start ASC, trigger_ref ASC
                    "#,
                )
                .bind(range.start)
                .bind(range.end)
                .bind(trigger_refs)
                .fetch_all(&state.db)
                .await?;
                rows.into_iter()
                    .map(|(bucket_start, series, count)| BucketCountRow {
                        bucket_start,
                        series,
                        count,
                    })
                    .collect()
            } else {
                let rows = sqlx::query_as::<_, (DateTime<Utc>, i64)>(
                    r#"
                    SELECT
                        date_trunc('hour', created) AS bucket_start,
                        COUNT(*)::bigint AS count
                    FROM event
                    WHERE created >= $1
                      AND created < $2
                    GROUP BY bucket_start
                    ORDER BY bucket_start ASC
                    "#,
                )
                .bind(range.start)
                .bind(range.end)
                .fetch_all(&state.db)
                .await?;
                rows.into_iter()
                    .map(|(bucket_start, count)| BucketCountRow {
                        bucket_start,
                        series: "all".to_string(),
                        count,
                    })
                    .collect()
            }
        }
    };
    Ok(rows)
}

fn map_freshness_mode(mode: FreshnessMode) -> DashboardFreshnessMode {
    match mode {
        FreshnessMode::RawOnly => DashboardFreshnessMode::RawOnly,
        FreshnessMode::AggregateOnly => DashboardFreshnessMode::AggregateOnly,
        FreshnessMode::AggregatePlusTail => DashboardFreshnessMode::AggregatePlusTail,
        FreshnessMode::RawOnlyFallback => DashboardFreshnessMode::RawOnlyFallback,
    }
}

fn normalize_request_ref_scope(
    request_filters: &BTreeMap<String, JsonValue>,
) -> Result<RefFilterScope, ApiError> {
    Ok(RefFilterScope {
        pack_refs: normalize_ref_filter_values(request_filters, "pack_ref", "pack_refs")?,
        action_refs: normalize_ref_filter_values(request_filters, "action_ref", "action_refs")?,
        trigger_refs: normalize_ref_filter_values(request_filters, "trigger_ref", "trigger_refs")?,
        rule_refs: normalize_ref_filter_values(request_filters, "rule_ref", "rule_refs")?,
        queue_refs: normalize_ref_filter_values(request_filters, "queue_ref", "queue_refs")?,
    })
}

fn resolve_source_param_scope(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
) -> Result<RefFilterScope, ApiError> {
    Ok(RefFilterScope {
        pack_refs: resolve_source_ref_filter_values(
            source,
            request_filters,
            "pack_ref",
            "pack_refs",
        )?,
        action_refs: resolve_source_ref_filter_values(
            source,
            request_filters,
            "action_ref",
            "action_refs",
        )?,
        trigger_refs: resolve_source_ref_filter_values(
            source,
            request_filters,
            "trigger_ref",
            "trigger_refs",
        )?,
        rule_refs: resolve_source_ref_filter_values(
            source,
            request_filters,
            "rule_ref",
            "rule_refs",
        )?,
        queue_refs: resolve_source_ref_filter_values(
            source,
            request_filters,
            "queue_ref",
            "queue_refs",
        )?,
    })
}

fn resolve_source_ref_filter_values(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    singular_key: &str,
    plural_key: &str,
) -> Result<Option<BTreeSet<String>>, ApiError> {
    let value = source
        .source_params
        .get(singular_key)
        .or_else(|| source.source_params.get(plural_key))
        .or_else(|| {
            source
                .source_params
                .get(singular_key.strip_suffix("_ref").unwrap_or(singular_key))
        });

    let Some(value) = value else {
        return Ok(None);
    };

    if let Some(template_filter_id) = parse_source_filter_template(value) {
        let Some(resolved_value) = request_filters.get(template_filter_id) else {
            return Ok(None);
        };
        return normalize_source_ref_value(
            resolved_value,
            singular_key,
            Some(template_filter_id),
            &source.source_id,
        );
    }

    normalize_source_ref_value(value, singular_key, None, &source.source_id)
}

fn normalize_source_ref_value(
    value: &JsonValue,
    singular_key: &str,
    template_filter_id: Option<&str>,
    source_id: &str,
) -> Result<Option<BTreeSet<String>>, ApiError> {
    let mut normalized = BTreeSet::new();
    match value {
        JsonValue::Null => {}
        JsonValue::String(single) => {
            normalized.insert(parse_safe_ref(single)?);
        }
        JsonValue::Array(items) => {
            for item in items {
                let candidate = item.as_str().ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "Dashboard source '{}' param '{}' must contain only string references",
                        source_id, singular_key
                    ))
                })?;
                normalized.insert(parse_safe_ref(candidate)?);
            }
        }
        _ => {
            let context = template_filter_id
                .map(|filter_id| format!(" resolved from filter '{}'", filter_id))
                .unwrap_or_default();
            return Err(ApiError::BadRequest(format!(
                "Dashboard source '{}' param '{}'{} must be a string or string array",
                source_id, singular_key, context
            )));
        }
    }

    Ok(Some(normalized))
}

fn parse_source_filter_template(value: &JsonValue) -> Option<&str> {
    let template = value.as_str()?.trim();
    let inner = template.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    inner.strip_prefix("filters.")?.split_whitespace().next()
}

fn resolve_source_param_json(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    keys: &[&str],
) -> Option<JsonValue> {
    let value = keys
        .iter()
        .find_map(|key| source.source_params.get(*key))
        .cloned()?;

    if let Some(filter_id) = parse_source_filter_template(&value) {
        return request_filters.get(filter_id).cloned();
    }

    Some(value)
}

fn resolve_source_param_i64(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<i64>, ApiError> {
    let Some(value) = resolve_source_param_json(source, request_filters, &[key]) else {
        return Ok(None);
    };

    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Number(number) => number.as_i64().map(Some).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Dashboard source '{}' param '{}' must be an integer",
                source.source_id, key
            ))
        }),
        other => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param '{}' resolved to unsupported value type '{}'",
            source.source_id,
            key,
            json_type_name(&other)
        ))),
    }
}

fn resolve_source_param_bucket_size(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
) -> Result<Option<String>, ApiError> {
    let Some(value) = resolve_source_param_json(source, request_filters, &["bucket_size"]) else {
        return Ok(None);
    };

    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(bucket_size) if bucket_size == "1h" => Ok(Some(bucket_size)),
        JsonValue::String(bucket_size) => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param 'bucket_size' currently only supports '1h' (found '{}')",
            source.source_id, bucket_size
        ))),
        other => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param 'bucket_size' resolved to unsupported value type '{}'",
            source.source_id,
            json_type_name(&other)
        ))),
    }
}

fn json_type_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn parse_source_params(value: &JsonValue) -> Result<BTreeMap<String, JsonValue>, ApiError> {
    let params = value.as_object().ok_or_else(|| {
        ApiError::UnprocessableEntity("Dashboard source 'params' must be an object".to_string())
    })?;
    Ok(params
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

fn source_type_contract_name_for_param_validation(source_type: &str) -> &str {
    if source_type == "worker_status" {
        "worker_health"
    } else {
        source_type
    }
}

fn source_contract_for_param_validation(
    source_type: &str,
) -> Result<(String, SourceContract), ApiError> {
    let contract_source_type = source_type_contract_name_for_param_validation(source_type);
    let parsed_source_type: SourceType = serde_json::from_value(JsonValue::String(
        contract_source_type.to_string(),
    ))
    .map_err(|_| {
        ApiError::UnprocessableEntity(format!(
            "Dashboard source type '{}' is not supported",
            source_type
        ))
    })?;

    let mut contracts = default_source_contracts();
    let contract = contracts.remove(&parsed_source_type).ok_or_else(|| {
        ApiError::UnprocessableEntity(format!(
            "Dashboard source type '{}' has no declared source contract",
            source_type
        ))
    })?;

    Ok((contract_source_type.to_string(), contract))
}

fn is_contract_param_key_allowed(allowed_param_keys: &BTreeSet<&str>, key: &str) -> bool {
    if allowed_param_keys.contains(key) {
        return true;
    }

    if let Some(stem) = key.strip_suffix("_refs") {
        let singular = format!("{stem}_ref");
        return allowed_param_keys.contains(singular.as_str());
    }

    if let Some(stem) = key.strip_suffix("_ref") {
        let plural = format!("{stem}_refs");
        return allowed_param_keys.contains(plural.as_str());
    }

    false
}

fn validate_source_params(
    source_id: &str,
    source_type: &str,
    source_params: &BTreeMap<String, JsonValue>,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    let (contract_source_type, contract) = source_contract_for_param_validation(source_type)?;

    for required_param in &contract.param_schema.required {
        if !source_params.contains_key(*required_param) {
            return Err(ApiError::UnprocessableEntity(format!(
                "Dashboard source '{}' of type '{}' is missing required param '{}'",
                source_id, source_type, required_param
            )));
        }
    }

    let allowed_param_keys: BTreeSet<&str> = contract
        .param_schema
        .required
        .iter()
        .chain(contract.param_schema.optional.iter())
        .copied()
        .collect();

    for (key, value) in source_params {
        if !is_contract_param_key_allowed(&allowed_param_keys, key.as_str()) {
            let allowed_keys = if allowed_param_keys.is_empty() {
                "(none)".to_string()
            } else {
                allowed_param_keys
                    .iter()
                    .map(|entry| entry.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(ApiError::UnprocessableEntity(format!(
                "Dashboard source '{}' of type '{}' has unsupported param key '{}' (contract '{}', allowed: {})",
                source_id, source_type, key, contract_source_type, allowed_keys
            )));
        }

        validate_source_param_value(source_id, key, value, declared_filters)?;
    }
    Ok(())
}

fn validate_source_param_value(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    if key == "worker_id" {
        return validate_source_worker_id_param(source_id, key, value, declared_filters);
    }
    if matches!(
        key,
        "decrypt" | "include_in_flight" | "include_cancelled" | "history"
    ) {
        return validate_source_bool_param(source_id, key, value, declared_filters);
    }
    if matches!(key, "assigned_to" | "sla_target_seconds") {
        return validate_source_integer_param(source_id, key, value, declared_filters);
    }
    if key == "bucket_size" {
        return validate_source_bucket_size_param(source_id, key, value, declared_filters);
    }
    if key == "window" {
        return validate_source_window_param(source_id, key, value, declared_filters);
    }
    if key == "owner_type" {
        return validate_source_owner_type_param(source_id, key, value, declared_filters);
    }
    if matches!(
        key,
        "owner_ref" | "path" | "status" | "worker_role" | "mode"
    ) {
        return validate_source_string_param(source_id, key, value, declared_filters);
    }
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::String(text) => {
            if let Some(filter_id) = parse_source_filter_template(value) {
                if !declared_filters.contains_key(filter_id) {
                    return Err(ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' references unknown filter '{}'",
                        source_id, key, filter_id
                    )));
                }
                return Ok(());
            }
            parse_safe_ref(text)
                .map(|_| ())
                .map_err(|_| {
                    ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' contains invalid reference '{}'",
                        source_id, key, text
                    ))
                })
        }
        JsonValue::Array(values) => values.iter().try_for_each(|entry| {
            let candidate = entry.as_str().ok_or_else(|| {
                ApiError::UnprocessableEntity(format!(
                    "Dashboard source '{}' param '{}' arrays must contain only string references",
                    source_id, key
                ))
            })?;
            if parse_source_filter_template(entry).is_some() {
                return Err(ApiError::UnprocessableEntity(format!(
                    "Dashboard source '{}' param '{}' template values are only supported as a single string",
                    source_id, key
                )));
            }
            parse_safe_ref(candidate)
                .map(|_| ())
                .map_err(|_| {
                    ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' contains invalid reference '{}'",
                        source_id, key, candidate
                    ))
                })
        }),
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be a string, string array, or null",
            source_id, key
        ))),
    }
}

fn validate_source_bool_param(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    match value {
        JsonValue::Null | JsonValue::Bool(_) => Ok(()),
        JsonValue::String(text) => {
            if let Some(filter_id) = parse_source_filter_template(value) {
                if !declared_filters.contains_key(filter_id) {
                    return Err(ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' references unknown filter '{}'",
                        source_id, key, filter_id
                    )));
                }
                return Ok(());
            }
            match text.as_str() {
                "true" | "false" => Ok(()),
                _ => Err(ApiError::UnprocessableEntity(format!(
                    "Dashboard source '{}' param '{}' must be a boolean, 'true'/'false', filter template, or null",
                    source_id, key
                ))),
            }
        }
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be a boolean, filter template, or null",
            source_id, key
        ))),
    }
}

fn validate_source_string_param(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::String(_) => {
            if let Some(filter_id) = parse_source_filter_template(value) {
                if !declared_filters.contains_key(filter_id) {
                    return Err(ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' references unknown filter '{}'",
                        source_id, key, filter_id
                    )));
                }
            }
            Ok(())
        }
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be a string, filter template, or null",
            source_id, key
        ))),
    }
}

fn validate_source_owner_type_param(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::String(text) => {
            if let Some(filter_id) = parse_source_filter_template(value) {
                if !declared_filters.contains_key(filter_id) {
                    return Err(ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' references unknown filter '{}'",
                        source_id, key, filter_id
                    )));
                }
                return Ok(());
            }
            parse_owner_type(text).map(|_| ()).map_err(|_| {
                ApiError::UnprocessableEntity(format!(
                    "Dashboard source '{}' param '{}' must be one of system, identity, pack, action, or sensor",
                    source_id, key
                ))
            })
        }
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be a string, filter template, or null",
            source_id, key
        ))),
    }
}

fn validate_source_integer_param(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::Number(number) if number.as_i64().is_some() => Ok(()),
        JsonValue::String(_) => {
            if let Some(filter_id) = parse_source_filter_template(value) {
                if !declared_filters.contains_key(filter_id) {
                    return Err(ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' references unknown filter '{}'",
                        source_id, key, filter_id
                    )));
                }
                return Ok(());
            }
            Err(ApiError::UnprocessableEntity(format!(
                "Dashboard source '{}' param '{}' must use a numeric literal or filter template",
                source_id, key
            )))
        }
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be an integer, filter template, or null",
            source_id, key
        ))),
    }
}

fn validate_source_bucket_size_param(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::String(_) => {
            if let Some(filter_id) = parse_source_filter_template(value) {
                if !declared_filters.contains_key(filter_id) {
                    return Err(ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' references unknown filter '{}'",
                        source_id, key, filter_id
                    )));
                }
            }
            Ok(())
        }
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be a string, filter template, or null",
            source_id, key
        ))),
    }
}

fn validate_source_worker_id_param(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::Number(number) if number.as_i64().is_some_and(|candidate| candidate > 0) => {
            Ok(())
        }
        JsonValue::String(text) => {
            if let Some(filter_id) = parse_source_filter_template(value) {
                if !declared_filters.contains_key(filter_id) {
                    return Err(ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' references unknown filter '{}'",
                        source_id, key, filter_id
                    )));
                }
                return Ok(());
            }
            text.parse::<i64>()
                .ok()
                .filter(|candidate| *candidate > 0)
                .map(|_| ())
                .ok_or_else(|| {
                    ApiError::UnprocessableEntity(format!(
                        "Dashboard source '{}' param '{}' must be a positive integer",
                        source_id, key
                    ))
                })
        }
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be a positive integer, string template, or null",
            source_id, key
        ))),
    }
}

fn validate_source_window_param(
    source_id: &str,
    key: &str,
    value: &JsonValue,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::String(_) if parse_source_filter_template(value).is_some() => {
            let filter_id = parse_source_filter_template(value).unwrap_or_default();
            if !declared_filters.contains_key(filter_id) {
                return Err(ApiError::UnprocessableEntity(format!(
                    "Dashboard source '{}' param '{}' references unknown filter '{}'",
                    source_id, key, filter_id
                )));
            }
            Ok(())
        }
        JsonValue::String(text) => parse_time_window(text).map(|_| ()).map_err(|_| {
            ApiError::UnprocessableEntity(format!(
                "Dashboard source '{}' param '{}' must be a valid time window like '15m' or '24h'",
                source_id, key
            ))
        }),
        _ => Err(ApiError::UnprocessableEntity(format!(
            "Dashboard source '{}' param '{}' must be a time-window string, string template, or null",
            source_id, key
        ))),
    }
}

fn resolve_source_param_value(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Option<JsonValue> {
    let value = source.source_params.get(key)?;
    if let Some(filter_id) = parse_source_filter_template(value) {
        request_filters.get(filter_id).cloned()
    } else {
        Some(value.clone())
    }
}

fn resolve_optional_source_ref_param(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<String>, ApiError> {
    let Some(value) = resolve_source_param_value(source, request_filters, key) else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(text) => parse_safe_ref(&text).map(Some),
        _ => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param '{}' must resolve to a string reference",
            source.source_id, key
        ))),
    }
}

fn resolve_optional_source_i64_param(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<i64>, ApiError> {
    let Some(value) = resolve_source_param_value(source, request_filters, key) else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Number(number) => number
            .as_i64()
            .filter(|candidate| *candidate > 0)
            .map(Some)
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "Dashboard source '{}' param '{}' must resolve to a positive integer",
                    source.source_id, key
                ))
            }),
        JsonValue::String(text) => text
            .parse::<i64>()
            .ok()
            .filter(|candidate| *candidate > 0)
            .map(Some)
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "Dashboard source '{}' param '{}' must resolve to a positive integer",
                    source.source_id, key
                ))
            }),
        _ => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param '{}' must resolve to a positive integer",
            source.source_id, key
        ))),
    }
}

fn resolve_optional_source_string_param(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<String>, ApiError> {
    let Some(value) = resolve_source_param_value(source, request_filters, key) else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(text) => Ok(Some(text)),
        _ => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param '{}' must resolve to a string",
            source.source_id, key
        ))),
    }
}

fn resolve_optional_source_bool_param(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<bool>, ApiError> {
    let Some(value) = resolve_source_param_value(source, request_filters, key) else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Bool(flag) => Ok(Some(flag)),
        JsonValue::String(text) => match text.as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => Err(ApiError::BadRequest(format!(
                "Dashboard source '{}' param '{}' must resolve to a boolean",
                source.source_id, key
            ))),
        },
        _ => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param '{}' must resolve to a boolean",
            source.source_id, key
        ))),
    }
}

fn resolve_optional_source_owner_type_param(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<OwnerType>, ApiError> {
    let Some(value) = resolve_source_param_value(source, request_filters, key) else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(text) => parse_owner_type(&text).map(Some).map_err(|_| {
            ApiError::BadRequest(format!(
                "Dashboard source '{}' param '{}' must resolve to one of system, identity, pack, action, or sensor",
                source.source_id, key
            ))
        }),
        _ => Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param '{}' must resolve to a string",
            source.source_id, key
        ))),
    }
}

fn parse_owner_type(value: &str) -> Result<OwnerType, ()> {
    match value {
        "system" => Ok(OwnerType::System),
        "identity" => Ok(OwnerType::Identity),
        "pack" => Ok(OwnerType::Pack),
        "action" => Ok(OwnerType::Action),
        "sensor" => Ok(OwnerType::Sensor),
        _ => Err(()),
    }
}

fn is_valid_result_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
}

fn resolve_required_result_path(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
) -> Result<String, ApiError> {
    let Some(path) = resolve_optional_source_string_param(source, request_filters, "path")? else {
        return Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' requires param 'path'",
            source.source_id
        )));
    };
    if !is_valid_result_path(&path) {
        return Err(ApiError::BadRequest(format!(
            "Dashboard source '{}' param 'path' must use dot-separated identifier segments",
            source.source_id
        )));
    }
    Ok(path)
}

fn resolve_optional_source_window_start(
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    effective_time_range: &DashboardEffectiveTimeRange,
) -> Result<DateTime<Utc>, ApiError> {
    let Some(value) = resolve_source_param_value(source, request_filters, "window") else {
        return Ok(effective_time_range.start);
    };
    let duration = match value {
        JsonValue::Null => return Ok(effective_time_range.start),
        JsonValue::String(text) => parse_time_window(&text)?,
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Dashboard source '{}' param 'window' must resolve to a time-window string",
                source.source_id
            )));
        }
    };
    Ok(std::cmp::max(
        effective_time_range.start,
        effective_time_range.end - duration,
    ))
}

fn normalize_ref_filter_values(
    filters: &BTreeMap<String, JsonValue>,
    singular_key: &str,
    plural_key: &str,
) -> Result<Option<BTreeSet<String>>, ApiError> {
    let value = filters
        .get(singular_key)
        .or_else(|| filters.get(plural_key))
        .or_else(|| filters.get(singular_key.strip_suffix("_ref").unwrap_or(singular_key)));

    let Some(value) = value else {
        return Ok(None);
    };

    let mut normalized = BTreeSet::new();
    match value {
        JsonValue::Null => {}
        JsonValue::String(single) => {
            normalized.insert(parse_safe_ref(single)?);
        }
        JsonValue::Array(items) => {
            for item in items {
                let candidate = item.as_str().ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "Filter '{}' must contain only string references",
                        singular_key
                    ))
                })?;
                normalized.insert(parse_safe_ref(candidate)?);
            }
        }
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Filter '{}' must be a string or string array",
                singular_key
            )));
        }
    }

    Ok(Some(normalized))
}

fn parse_safe_ref(value: &str) -> Result<String, ApiError> {
    SafeRef::parse(value)
        .map(|safe| safe.as_str().to_string())
        .map_err(|_| ApiError::BadRequest(format!("Invalid reference filter value '{}'", value)))
}

fn resolve_source_query_scope(
    source: &DashboardSourceDef,
    request_scope: &RefFilterScope,
    source_params_scope: &RefFilterScope,
    effective_grants: &[Grant],
    required_auth: Option<SourceAuthRequirement>,
) -> Result<SourceQueryScope, ApiError> {
    let primary_ref_kind = SourcePrimaryRefKind::from_source_type(&source.source_type);
    let requested_primary = primary_ref_kind.and_then(|kind| match kind {
        SourcePrimaryRefKind::Action => request_scope.action_refs.clone(),
        SourcePrimaryRefKind::Trigger => request_scope.trigger_refs.clone(),
        SourcePrimaryRefKind::Rule => request_scope.rule_refs.clone(),
        SourcePrimaryRefKind::Queue => request_scope.queue_refs.clone(),
    });

    let source_primary = primary_ref_kind.and_then(|kind| match kind {
        SourcePrimaryRefKind::Action => source_params_scope.action_refs.clone(),
        SourcePrimaryRefKind::Trigger => source_params_scope.trigger_refs.clone(),
        SourcePrimaryRefKind::Rule => source_params_scope.rule_refs.clone(),
        SourcePrimaryRefKind::Queue => source_params_scope.queue_refs.clone(),
    });

    let requested_pack_refs = intersect_ref_sets(
        request_scope.pack_refs.clone(),
        source_params_scope.pack_refs.clone(),
    );
    let requested_primary = intersect_ref_sets(requested_primary, source_primary);

    let mut mode = DashboardAuthorizationMode::OperatorGlobal;
    if source.source_type == "key_value" {
        return Ok(SourceQueryScope {
            authorization_mode: mode,
            pack_refs: requested_pack_refs,
            primary_ref_kind,
            primary_refs: requested_primary,
        });
    }
    let (auth_pack_refs, auth_primary_refs) = if let Some(required_auth) = required_auth {
        let constraints = collect_authz_ref_constraints(
            effective_grants,
            required_auth.resource,
            required_auth.action,
        )?;
        if constraints.unrestricted {
            (None, None)
        } else {
            mode = DashboardAuthorizationMode::IdentityFiltered;
            (
                if constraints.pack_refs.is_empty() {
                    None
                } else {
                    Some(constraints.pack_refs)
                },
                if constraints.refs.is_empty() {
                    None
                } else {
                    Some(constraints.refs)
                },
            )
        }
    } else {
        (None, None)
    };

    let pack_refs = intersect_ref_sets(requested_pack_refs, auth_pack_refs);
    let primary_refs = intersect_ref_sets(requested_primary, auth_primary_refs);

    Ok(SourceQueryScope {
        authorization_mode: mode,
        pack_refs,
        primary_ref_kind,
        primary_refs,
    })
}

fn collect_authz_ref_constraints(
    grants: &[Grant],
    resource: Resource,
    action: RbacAction,
) -> Result<AuthzRefConstraints, ApiError> {
    let mut constraints = AuthzRefConstraints::default();
    let mut matching_grants = 0usize;
    let mut usable_grants = 0usize;

    for grant in grants {
        if grant.resource != resource || !grant.actions.contains(&action) {
            continue;
        }
        matching_grants += 1;
        match &grant.constraints {
            None => {
                constraints.unrestricted = true;
                return Ok(constraints);
            }
            Some(c) => {
                if c.owner.is_some()
                    || c.owner_types.is_some()
                    || c.owner_refs.is_some()
                    || c.visibility.is_some()
                    || c.execution_scope.is_some()
                    || c.ids.is_some()
                    || c.encrypted.is_some()
                    || c.attributes.is_some()
                {
                    continue;
                }
                usable_grants += 1;
                if let Some(pack_refs) = &c.pack_refs {
                    constraints.pack_refs.extend(pack_refs.iter().cloned());
                }
                if let Some(refs) = &c.refs {
                    constraints.refs.extend(refs.iter().cloned());
                }
            }
        }
    }

    if matching_grants == 0 || usable_grants == 0 {
        return Err(ApiError::Forbidden(
            "Not authorized to read this source".to_string(),
        ));
    }

    Ok(constraints)
}

fn intersect_ref_sets(
    request_values: Option<BTreeSet<String>>,
    auth_values: Option<BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    match (request_values, auth_values) {
        (Some(request), Some(auth)) => Some(request.intersection(&auth).cloned().collect()),
        (Some(request), None) => Some(request),
        (None, Some(auth)) => Some(auth),
        (None, None) => None,
    }
}

async fn effective_action_refs(
    state: &Arc<AppState>,
    source_scope: &SourceQueryScope,
) -> Result<Option<BTreeSet<String>>, ApiError> {
    effective_refs_by_pack(source_scope, SourcePrimaryRefKind::Action, || async {
        Ok(ActionRepository::list(&state.db)
            .await?
            .into_iter()
            .map(|action| action.r#ref)
            .collect())
    })
    .await
}

async fn query_queue_throughput_rows(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    queue_refs: Option<&BTreeSet<String>>,
) -> Result<Vec<QueueThroughputSourceRow>, ApiError> {
    let rows = if let Some(queue_refs) = queue_refs {
        let queue_refs: Vec<String> = queue_refs.iter().cloned().collect();
        sqlx::query_as::<_, (DateTime<Utc>, String, i64, i64, i64, i64, i64)>(
            r#"
            SELECT
                date_trunc('hour', updated) AS bucket_start,
                queue_ref,
                COUNT(*) FILTER (WHERE status::text = 'completed')::bigint AS completed,
                COUNT(*) FILTER (WHERE status::text = 'failed')::bigint AS failed,
                COUNT(*) FILTER (WHERE status::text = 'skipped')::bigint AS skipped,
                COUNT(*) FILTER (WHERE status::text = 'cancelled')::bigint AS cancelled,
                COUNT(*)::bigint AS total_processed
            FROM work_queue_item
            WHERE updated >= $1
              AND updated < $2
              AND queue_ref = ANY($3::text[])
              AND status::text = ANY($4::text[])
            GROUP BY bucket_start, queue_ref
            ORDER BY bucket_start ASC, queue_ref ASC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(queue_refs)
        .bind(TERMINAL_QUEUE_ITEM_STATUSES)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, (DateTime<Utc>, String, i64, i64, i64, i64, i64)>(
            r#"
            SELECT
                date_trunc('hour', updated) AS bucket_start,
                queue_ref,
                COUNT(*) FILTER (WHERE status::text = 'completed')::bigint AS completed,
                COUNT(*) FILTER (WHERE status::text = 'failed')::bigint AS failed,
                COUNT(*) FILTER (WHERE status::text = 'skipped')::bigint AS skipped,
                COUNT(*) FILTER (WHERE status::text = 'cancelled')::bigint AS cancelled,
                COUNT(*)::bigint AS total_processed
            FROM work_queue_item
            WHERE updated >= $1
              AND updated < $2
              AND status::text = ANY($3::text[])
            GROUP BY bucket_start, queue_ref
            ORDER BY bucket_start ASC, queue_ref ASC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(TERMINAL_QUEUE_ITEM_STATUSES)
        .fetch_all(&state.db)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(
            |(bucket_start, queue_ref, completed, failed, skipped, cancelled, total_processed)| {
                QueueThroughputSourceRow {
                    bucket_start,
                    queue_ref,
                    completed,
                    failed,
                    skipped,
                    cancelled,
                    total_processed,
                }
            },
        )
        .collect())
}

async fn query_queue_dispatch_stats_rows(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    queue_refs: Option<&BTreeSet<String>>,
) -> Result<Vec<QueueDispatchStatsSourceRow>, ApiError> {
    let rows = if let Some(queue_refs) = queue_refs {
        let queue_refs: Vec<String> = queue_refs.iter().cloned().collect();
        sqlx::query_as::<_, (DateTime<Utc>, String, String, i64, i64, f64, f64)>(
            r#"
            SELECT
                date_trunc('hour', COALESCE(e.updated, d.updated)) AS bucket_start,
                d.queue_ref,
                COALESCE(e.status::text, d.status::text) AS status,
                COUNT(*)::bigint AS dispatch_count,
                COALESCE(SUM(d.leased_item_count), 0)::bigint AS leased_item_count,
                COALESCE(
                    AVG(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    MAX(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS max_duration_seconds
            FROM work_queue_dispatch d
            LEFT JOIN execution e ON e.id = d.execution
            WHERE COALESCE(e.updated, d.updated) >= $1
              AND COALESCE(e.updated, d.updated) < $2
              AND d.queue_ref = ANY($3::text[])
              AND (
                    e.status::text = ANY($4::text[])
                 OR (e.id IS NULL AND d.status::text = ANY($5::text[]))
              )
            GROUP BY bucket_start, d.queue_ref, COALESCE(e.status::text, d.status::text)
            ORDER BY bucket_start ASC, d.queue_ref ASC, status ASC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(queue_refs)
        .bind(TERMINAL_EXECUTION_STATUSES)
        .bind(TERMINAL_QUEUE_DISPATCH_FALLBACK_STATUSES)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, (DateTime<Utc>, String, String, i64, i64, f64, f64)>(
            r#"
            SELECT
                date_trunc('hour', COALESCE(e.updated, d.updated)) AS bucket_start,
                d.queue_ref,
                COALESCE(e.status::text, d.status::text) AS status,
                COUNT(*)::bigint AS dispatch_count,
                COALESCE(SUM(d.leased_item_count), 0)::bigint AS leased_item_count,
                COALESCE(
                    AVG(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    MAX(
                        EXTRACT(EPOCH FROM (
                            COALESCE(e.updated, d.updated)
                            - COALESCE(e.started_at, e.created, d.created)
                        ))
                    ),
                    0
                )::double precision AS max_duration_seconds
            FROM work_queue_dispatch d
            LEFT JOIN execution e ON e.id = d.execution
            WHERE COALESCE(e.updated, d.updated) >= $1
              AND COALESCE(e.updated, d.updated) < $2
              AND (
                    e.status::text = ANY($3::text[])
                 OR (e.id IS NULL AND d.status::text = ANY($4::text[]))
              )
            GROUP BY bucket_start, d.queue_ref, COALESCE(e.status::text, d.status::text)
            ORDER BY bucket_start ASC, d.queue_ref ASC, status ASC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(TERMINAL_EXECUTION_STATUSES)
        .bind(TERMINAL_QUEUE_DISPATCH_FALLBACK_STATUSES)
        .fetch_all(&state.db)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(
            |(
                bucket_start,
                queue_ref,
                status,
                dispatch_count,
                leased_item_count,
                avg_duration_seconds,
                max_duration_seconds,
            )| QueueDispatchStatsSourceRow {
                bucket_start,
                queue_ref,
                status,
                dispatch_count,
                leased_item_count,
                avg_duration_seconds,
                max_duration_seconds,
            },
        )
        .collect())
}

async fn query_inquiry_backlog_rows(
    state: &Arc<AppState>,
    pack_refs: Option<&BTreeSet<String>>,
    assigned_to: Option<i64>,
) -> Result<Vec<InquiryBacklogSourceRow>, ApiError> {
    let pack_ref_expr = r#"
        CASE
            WHEN e.action_ref IS NOT NULL AND position('.' in e.action_ref) > 0
                THEN split_part(e.action_ref, '.', 1)
            ELSE NULL
        END
    "#;

    let rows = match (pack_refs, assigned_to) {
        (Some(pack_refs), Some(assigned_to)) => {
            let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
            sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.status::text = 'pending'
                      AND i.assigned_to = $1
                      AND {pack_ref_expr} = ANY($2::text[])
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .bind(assigned_to)
            .bind(pack_refs)
            .fetch_all(&state.db)
            .await?
        }
        (Some(pack_refs), None) => {
            let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
            sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.status::text = 'pending'
                      AND {pack_ref_expr} = ANY($1::text[])
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .bind(pack_refs)
            .fetch_all(&state.db)
            .await?
        }
        (None, Some(assigned_to)) => {
            sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.status::text = 'pending'
                      AND i.assigned_to = $1
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .bind(assigned_to)
            .fetch_all(&state.db)
            .await?
        }
        (None, None) => {
            sqlx::query_as::<_, (Option<String>, Option<i64>, i64, i64)>(&format!(
                r#"
                    SELECT
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS pending_count,
                        COUNT(*) FILTER (
                            WHERE i.timeout_at IS NOT NULL AND i.timeout_at < NOW()
                        )::bigint AS overdue_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.status::text = 'pending'
                    GROUP BY 1, 2
                    ORDER BY pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .fetch_all(&state.db)
            .await?
        }
    };

    Ok(rows
        .into_iter()
        .map(
            |(pack_ref, assigned_to, pending_count, overdue_count)| InquiryBacklogSourceRow {
                pack_ref,
                assigned_to,
                pending_count,
                overdue_count,
            },
        )
        .collect())
}

async fn query_inquiry_sla_rows(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    pack_refs: Option<&BTreeSet<String>>,
    assigned_to: Option<i64>,
    sla_target_seconds: i64,
) -> Result<Vec<InquirySlaSourceRow>, ApiError> {
    let pack_ref_expr = r#"
        CASE
            WHEN e.action_ref IS NOT NULL AND position('.' in e.action_ref) > 0
                THEN split_part(e.action_ref, '.', 1)
            ELSE NULL
        END
    "#;
    let elapsed_expr = r#"
        EXTRACT(EPOCH FROM (
            COALESCE(
                i.responded_at,
                CASE WHEN i.status::text = 'timeout' THEN COALESCE(i.updated, i.timeout_at) END,
                NOW()
            ) - i.created
        ))
    "#;

    let rows = match (pack_refs, assigned_to) {
        (Some(pack_refs), Some(assigned_to)) => {
            let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
            sqlx::query_as::<
                _,
                (
                    DateTime<Utc>,
                    Option<String>,
                    Option<i64>,
                    i64,
                    i64,
                    i64,
                    i64,
                ),
            >(&format!(
                r#"
                    SELECT
                        date_trunc('hour', i.created) AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.created >= $2
                      AND i.created < $3
                      AND i.assigned_to = $4
                      AND {pack_ref_expr} = ANY($5::text[])
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .bind(sla_target_seconds as f64)
            .bind(effective_time_range.start)
            .bind(effective_time_range.end)
            .bind(assigned_to)
            .bind(pack_refs)
            .fetch_all(&state.db)
            .await?
        }
        (Some(pack_refs), None) => {
            let pack_refs: Vec<String> = pack_refs.iter().cloned().collect();
            sqlx::query_as::<
                _,
                (
                    DateTime<Utc>,
                    Option<String>,
                    Option<i64>,
                    i64,
                    i64,
                    i64,
                    i64,
                ),
            >(&format!(
                r#"
                    SELECT
                        date_trunc('hour', i.created) AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.created >= $2
                      AND i.created < $3
                      AND {pack_ref_expr} = ANY($4::text[])
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .bind(sla_target_seconds as f64)
            .bind(effective_time_range.start)
            .bind(effective_time_range.end)
            .bind(pack_refs)
            .fetch_all(&state.db)
            .await?
        }
        (None, Some(assigned_to)) => {
            sqlx::query_as::<
                _,
                (
                    DateTime<Utc>,
                    Option<String>,
                    Option<i64>,
                    i64,
                    i64,
                    i64,
                    i64,
                ),
            >(&format!(
                r#"
                    SELECT
                        date_trunc('hour', i.created) AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.created >= $2
                      AND i.created < $3
                      AND i.assigned_to = $4
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .bind(sla_target_seconds as f64)
            .bind(effective_time_range.start)
            .bind(effective_time_range.end)
            .bind(assigned_to)
            .fetch_all(&state.db)
            .await?
        }
        (None, None) => {
            sqlx::query_as::<
                _,
                (
                    DateTime<Utc>,
                    Option<String>,
                    Option<i64>,
                    i64,
                    i64,
                    i64,
                    i64,
                ),
            >(&format!(
                r#"
                    SELECT
                        date_trunc('hour', i.created) AS bucket_start,
                        {pack_ref_expr} AS pack_ref,
                        i.assigned_to,
                        COUNT(*)::bigint AS total_inquiries,
                        COUNT(*) FILTER (WHERE {elapsed_expr} <= $1)::bigint AS within_sla_count,
                        COUNT(*) FILTER (WHERE {elapsed_expr} > $1)::bigint AS breached_count,
                        COUNT(*) FILTER (WHERE i.status::text = 'pending')::bigint AS open_count
                    FROM inquiry i
                    LEFT JOIN execution e ON e.id = i.execution
                    WHERE i.created >= $2
                      AND i.created < $3
                    GROUP BY 1, 2, 3
                    ORDER BY bucket_start ASC, pack_ref ASC NULLS LAST, i.assigned_to ASC NULLS LAST
                    "#
            ))
            .bind(sla_target_seconds as f64)
            .bind(effective_time_range.start)
            .bind(effective_time_range.end)
            .fetch_all(&state.db)
            .await?
        }
    };

    Ok(rows
        .into_iter()
        .map(
            |(
                bucket_start,
                pack_ref,
                assigned_to,
                total_inquiries,
                within_sla_count,
                breached_count,
                open_count,
            )| InquirySlaSourceRow {
                bucket_start,
                pack_ref,
                assigned_to,
                sla_target_seconds,
                total_inquiries,
                within_sla_count,
                breached_count,
                open_count,
                compliance_rate: if total_inquiries > 0 {
                    within_sla_count as f64 / total_inquiries as f64
                } else {
                    0.0
                },
            },
        )
        .collect())
}

async fn query_execution_duration_stats_rows(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    action_refs: Option<&BTreeSet<String>>,
) -> Result<Vec<ExecutionDurationStatsSourceRow>, ApiError> {
    let rows = if let Some(action_refs) = action_refs {
        let action_refs: Vec<String> = action_refs.iter().cloned().collect();
        sqlx::query_as::<_, (DateTime<Utc>, String, i64, f64, f64, f64, f64)>(
            r#"
            SELECT
                date_trunc('hour', updated) AS bucket_start,
                COALESCE(action_ref, 'unknown') AS series,
                COUNT(*)::bigint AS execution_count,
                COALESCE(
                    AVG(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.5) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p50_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.95) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p95_duration_seconds,
                COALESCE(
                    MAX(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS max_duration_seconds
            FROM execution
            WHERE updated >= $1
              AND updated < $2
              AND started_at IS NOT NULL
              AND status::text = ANY($3::text[])
              AND action_ref = ANY($4::text[])
            GROUP BY 1, 2
            ORDER BY bucket_start ASC, series ASC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(TERMINAL_EXECUTION_STATUSES)
        .bind(action_refs)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, (DateTime<Utc>, String, i64, f64, f64, f64, f64)>(
            r#"
            SELECT
                date_trunc('hour', updated) AS bucket_start,
                COALESCE(action_ref, 'unknown') AS series,
                COUNT(*)::bigint AS execution_count,
                COALESCE(
                    AVG(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS avg_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.5) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p50_duration_seconds,
                COALESCE(
                    PERCENTILE_CONT(0.95) WITHIN GROUP (
                        ORDER BY EXTRACT(EPOCH FROM (updated - started_at))
                    ),
                    0
                )::double precision AS p95_duration_seconds,
                COALESCE(
                    MAX(EXTRACT(EPOCH FROM (updated - started_at))),
                    0
                )::double precision AS max_duration_seconds
            FROM execution
            WHERE updated >= $1
              AND updated < $2
              AND started_at IS NOT NULL
              AND status::text = ANY($3::text[])
            GROUP BY 1, 2
            ORDER BY bucket_start ASC, series ASC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(TERMINAL_EXECUTION_STATUSES)
        .fetch_all(&state.db)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(
            |(
                bucket_start,
                series,
                execution_count,
                avg_duration_seconds,
                p50_duration_seconds,
                p95_duration_seconds,
                max_duration_seconds,
            )| ExecutionDurationStatsSourceRow {
                bucket_start,
                series,
                execution_count,
                avg_duration_seconds,
                p50_duration_seconds,
                p95_duration_seconds,
                max_duration_seconds,
            },
        )
        .collect())
}

async fn effective_trigger_refs(
    state: &Arc<AppState>,
    source_scope: &SourceQueryScope,
) -> Result<Option<BTreeSet<String>>, ApiError> {
    effective_refs_by_pack(source_scope, SourcePrimaryRefKind::Trigger, || async {
        Ok(TriggerRepository::list(&state.db)
            .await?
            .into_iter()
            .map(|trigger| trigger.r#ref)
            .collect())
    })
    .await
}

async fn effective_rule_refs(
    state: &Arc<AppState>,
    source_scope: &SourceQueryScope,
) -> Result<Option<BTreeSet<String>>, ApiError> {
    effective_refs_by_pack(source_scope, SourcePrimaryRefKind::Rule, || async {
        Ok(RuleRepository::list(&state.db)
            .await?
            .into_iter()
            .map(|rule| rule.r#ref)
            .collect())
    })
    .await
}

async fn effective_queue_refs(
    state: &Arc<AppState>,
    source_scope: &SourceQueryScope,
) -> Result<Option<BTreeSet<String>>, ApiError> {
    effective_refs_by_pack(source_scope, SourcePrimaryRefKind::Queue, || async {
        Ok(WorkQueueRepository::list(&state.db)
            .await?
            .into_iter()
            .map(|queue| queue.r#ref)
            .collect())
    })
    .await
}

async fn effective_refs_by_pack<F, Fut>(
    source_scope: &SourceQueryScope,
    expected_kind: SourcePrimaryRefKind,
    list_all_refs: F,
) -> Result<Option<BTreeSet<String>>, ApiError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<BTreeSet<String>, attune_common::error::Error>>,
{
    if source_scope.primary_ref_kind != Some(expected_kind) {
        return Ok(None);
    }

    if source_scope.pack_refs.is_none() {
        return Ok(source_scope.primary_refs.clone());
    }

    let mut refs = if let Some(existing) = &source_scope.primary_refs {
        existing.clone()
    } else {
        list_all_refs().await.map_err(ApiError::from)?
    };

    if let Some(pack_refs) = &source_scope.pack_refs {
        refs.retain(|value| {
            value
                .split_once('.')
                .is_some_and(|(pack_ref, _)| pack_refs.contains(pack_ref))
        });
    }

    Ok(Some(refs))
}

async fn query_latest_execution_rows(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    action_refs: Option<&BTreeSet<String>>,
    statuses: &[&str],
) -> Result<Vec<LatestExecutionQueryRow>, ApiError> {
    let action_refs: Option<Vec<String>> =
        action_refs.map(|refs| refs.iter().cloned().collect::<Vec<_>>());
    let statuses = statuses
        .iter()
        .map(|status| (*status).to_string())
        .collect::<Vec<_>>();
    sqlx::query_as::<_, LatestExecutionQueryRow>(
        r#"
        SELECT DISTINCT ON (e.action_ref)
            e.action_ref AS action_ref,
            e.id AS execution_id,
            e.status AS status,
            e.created AS created_at,
            e.started_at AS started_at,
            e.updated AS updated_at,
            e.trace_tag AS trace_tag,
            e.result AS result
        FROM execution e
        WHERE ($1::text[] IS NULL OR e.action_ref = ANY($1))
          AND e.created >= $2
          AND e.created < $3
          AND e.status::text = ANY($4::text[])
        ORDER BY e.action_ref ASC, e.created DESC, e.id DESC
        "#,
    )
    .bind(action_refs)
    .bind(effective_time_range.start)
    .bind(effective_time_range.end)
    .bind(statuses)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)
}

fn key_owner_ref(
    owner_type: OwnerType,
    owner: Option<&str>,
    owner_pack_ref: Option<&str>,
    owner_action_ref: Option<&str>,
    owner_sensor_ref: Option<&str>,
) -> Option<String> {
    match owner_type {
        OwnerType::Pack => owner_pack_ref.map(str::to_string),
        OwnerType::Action => owner_action_ref.map(str::to_string),
        OwnerType::Sensor => owner_sensor_ref.map(str::to_string),
        _ => owner.map(str::to_string),
    }
}

fn key_authorization_context(identity_id: i64, key: &Key) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(key.id);
    ctx.target_ref = Some(key.r#ref.clone());
    ctx.owner_identity_id = key.owner_identity;
    ctx.owner_type = Some(key.owner_type);
    ctx.owner_ref = key_owner_ref(
        key.owner_type,
        key.owner.as_deref(),
        key.owner_pack_ref.as_deref(),
        key.owner_action_ref.as_deref(),
        key.owner_sensor_ref.as_deref(),
    );
    ctx.encrypted = Some(key.encrypted);
    ctx
}

fn constrained_key_grant_allows(
    grants: &[Grant],
    action: RbacAction,
    ctx: &AuthorizationContext,
) -> bool {
    grants.iter().any(|grant| {
        let Some(constraints) = &grant.constraints else {
            return false;
        };
        let owner_scoped = constraints.owner.is_some()
            || constraints.owner_types.is_some()
            || constraints.owner_refs.is_some()
            || constraints.refs.is_some()
            || constraints.ids.is_some();
        grant.resource == Resource::Keys
            && grant.actions.contains(&action)
            && owner_scoped
            && grant.allows(Resource::Keys, action, ctx)
    })
}

fn key_action_allowed(grants: &[Grant], action: RbacAction, identity_id: i64, key: &Key) -> bool {
    let ctx = key_authorization_context(identity_id, key);
    if key.owner_type == OwnerType::Identity && key.owner_identity != Some(identity_id) {
        return constrained_key_grant_allows(grants, action, &ctx);
    }

    AuthorizationService::is_allowed(grants, Resource::Keys, action, &ctx)
}

async fn build_key_value_source_data(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    effective_grants: &[Grant],
    key_ref: &str,
    expected_owner_type: Option<OwnerType>,
    expected_owner_ref: Option<&str>,
    decrypt_requested: bool,
) -> Result<Option<KeyValueSourceData>, ApiError> {
    let Some(mut key) =
        attune_common::repositories::key::KeyRepository::find_by_ref(&state.db, key_ref).await?
    else {
        return Ok(None);
    };

    if expected_owner_type.is_some_and(|owner_type| key.owner_type != owner_type) {
        return Ok(None);
    }

    let actual_owner_ref = key_owner_ref(
        key.owner_type,
        key.owner.as_deref(),
        key.owner_pack_ref.as_deref(),
        key.owner_action_ref.as_deref(),
        key.owner_sensor_ref.as_deref(),
    );
    if expected_owner_ref.is_some_and(|expected| actual_owner_ref.as_deref() != Some(expected)) {
        return Ok(None);
    }

    let identity_id = actor_identity_id(user)?.unwrap_or_default();
    if !key_action_allowed(effective_grants, RbacAction::Read, identity_id, &key) {
        return Err(ApiError::Forbidden(
            "Not authorized to read this source".to_string(),
        ));
    }

    let can_decrypt = !key.encrypted
        || (decrypt_requested
            && key_action_allowed(effective_grants, RbacAction::Decrypt, identity_id, &key));

    if key.encrypted {
        if can_decrypt {
            let encryption_key =
                state
                    .config
                    .security
                    .encryption_key
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::InternalServerError(
                            "Encryption key not configured on server".to_string(),
                        )
                    })?;
            key.value = attune_common::crypto::decrypt_json(&key.value, encryption_key).map_err(
                |error| {
                    ApiError::InternalServerError(format!(
                        "Failed to decrypt key '{}': {}",
                        key.r#ref, error
                    ))
                },
            )?;
        } else {
            key.value = JsonValue::Null;
        }
    }

    Ok(Some(KeyValueSourceData {
        r#ref: key.r#ref,
        name: key.name,
        owner_type: owner_type_label(key.owner_type).to_string(),
        owner_ref: actual_owner_ref,
        encrypted: key.encrypted,
        decrypted: key.encrypted && can_decrypt,
        value: key.value,
        updated_at: key.updated,
    }))
}

fn collect_json_paths(value: &JsonValue, prefix: Option<&str>, output: &mut BTreeSet<String>) {
    if let Some(prefix) = prefix {
        output.insert(prefix.to_string());
    } else {
        match value {
            JsonValue::String(_) => {
                output.insert("message".to_string());
                output.insert("value".to_string());
            }
            JsonValue::Object(_) => {}
            _ => {
                output.insert("value".to_string());
            }
        }
    }

    if let JsonValue::Object(object) = value {
        for (key, child) in object {
            let next = prefix
                .map(|existing| format!("{existing}.{key}"))
                .unwrap_or_else(|| key.clone());
            collect_json_paths(child, Some(&next), output);
        }
    }
}

fn seed_default_action_result_paths(output: &mut BTreeSet<String>) {
    for path in [
        "stdout",
        "stderr_log",
        "data",
        "error",
        "exit_code",
        "duration_ms",
        "succeeded",
        "queue_ack",
        "stdout_truncated",
        "stdout_bytes_truncated",
        "stderr_truncated",
        "stderr_bytes_truncated",
    ] {
        output.insert(path.to_string());
    }
}

fn include_requested_derived_action_result_paths(path: &str, output: &mut BTreeSet<String>) {
    if path.starts_with("data.") {
        output.insert("data".to_string());
        output.insert(path.to_string());
    }
}

fn build_action_result_path_not_allowed_message(
    source_id: &str,
    path: &str,
    allowed_paths: &BTreeSet<String>,
) -> String {
    if allowed_paths.is_empty() {
        return format!(
            "Dashboard source '{}' path '{}' is not available because the latest terminal action results contain no selectable result paths.",
            source_id, path
        );
    }

    let available_paths = allowed_paths.iter().cloned().collect::<Vec<_>>().join(", ");
    format!(
        "Dashboard source '{}' path '{}' is not available. Choose one of the allowed paths from latest terminal results: {}.",
        source_id, path, available_paths
    )
}

fn extract_json_path<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    if !value.is_object() {
        return match path {
            "value" => Some(value),
            "message" if value.is_string() => Some(value),
            _ => None,
        };
    }

    let mut current = value;
    for segment in path.split('.') {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

async fn query_last_event_rows(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    trigger_refs: Option<&BTreeSet<String>>,
) -> Result<Vec<LastEventSourceRow>, ApiError> {
    let rows = if let Some(trigger_refs) = trigger_refs {
        let trigger_refs: Vec<String> = trigger_refs.iter().cloned().collect();
        sqlx::query_as::<
            _,
            (
                String,
                i64,
                DateTime<Utc>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            r#"
            SELECT trigger_ref, event_id, created, source_ref, rule_ref, trace_tag
            FROM (
                SELECT DISTINCT ON (trigger_ref)
                    trigger_ref,
                    id AS event_id,
                    created,
                    source_ref,
                    rule_ref,
                    trace_tag
                FROM event
                WHERE created >= $1
                  AND created < $2
                  AND trigger_ref = ANY($3::text[])
                ORDER BY trigger_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY trigger_ref ASC, event_id DESC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(trigger_refs)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<
            _,
            (
                String,
                i64,
                DateTime<Utc>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            r#"
            SELECT trigger_ref, event_id, created, source_ref, rule_ref, trace_tag
            FROM (
                SELECT DISTINCT ON (trigger_ref)
                    trigger_ref,
                    id AS event_id,
                    created,
                    source_ref,
                    rule_ref,
                    trace_tag
                FROM event
                WHERE created >= $1
                  AND created < $2
                ORDER BY trigger_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY trigger_ref ASC, event_id DESC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .fetch_all(&state.db)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(
            |(trigger_ref, event_id, created, source_ref, rule_ref, trace_tag)| {
                LastEventSourceRow {
                    trigger_ref,
                    event_id,
                    created,
                    source_ref,
                    rule_ref,
                    trace_tag,
                }
            },
        )
        .collect())
}

async fn query_last_enforcement_rows(
    state: &Arc<AppState>,
    effective_time_range: &DashboardEffectiveTimeRange,
    rule_refs: Option<&BTreeSet<String>>,
) -> Result<Vec<LastEnforcementSourceRow>, ApiError> {
    let rows = if let Some(rule_refs) = rule_refs {
        let rule_refs: Vec<String> = rule_refs.iter().cloned().collect();
        sqlx::query_as::<
            _,
            (
                String,
                i64,
                String,
                String,
                DateTime<Utc>,
                Option<DateTime<Utc>>,
                Option<i64>,
            ),
        >(
            r#"
            SELECT rule_ref, enforcement_id, trigger_ref, status, created, resolved_at, event_id
            FROM (
                SELECT DISTINCT ON (rule_ref)
                    rule_ref,
                    id AS enforcement_id,
                    trigger_ref,
                    status::text AS status,
                    created,
                    resolved_at,
                    event AS event_id
                FROM enforcement
                WHERE created >= $1
                  AND created < $2
                  AND rule_ref = ANY($3::text[])
                ORDER BY rule_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY rule_ref ASC, enforcement_id DESC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .bind(rule_refs)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<
            _,
            (
                String,
                i64,
                String,
                String,
                DateTime<Utc>,
                Option<DateTime<Utc>>,
                Option<i64>,
            ),
        >(
            r#"
            SELECT rule_ref, enforcement_id, trigger_ref, status, created, resolved_at, event_id
            FROM (
                SELECT DISTINCT ON (rule_ref)
                    rule_ref,
                    id AS enforcement_id,
                    trigger_ref,
                    status::text AS status,
                    created,
                    resolved_at,
                    event AS event_id
                FROM enforcement
                WHERE created >= $1
                  AND created < $2
                ORDER BY rule_ref ASC, created DESC, id DESC
            ) latest
            ORDER BY rule_ref ASC, enforcement_id DESC
            "#,
        )
        .bind(effective_time_range.start)
        .bind(effective_time_range.end)
        .fetch_all(&state.db)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(
            |(rule_ref, enforcement_id, trigger_ref, status, created, resolved_at, event_id)| {
                LastEnforcementSourceRow {
                    rule_ref,
                    enforcement_id,
                    trigger_ref,
                    status,
                    created,
                    resolved_at,
                    event_id,
                }
            },
        )
        .collect())
}

async fn execute_source_handler(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    effective_grants: &[Grant],
    source: &DashboardSourceDef,
    request_filters: &BTreeMap<String, JsonValue>,
    source_scope: &SourceQueryScope,
    effective_time_range: &DashboardEffectiveTimeRange,
) -> Result<DashboardSourceResult, ApiError> {
    let mut meta = default_source_meta();
    meta.authorization_mode = source_scope.authorization_mode;
    meta.authorized_refs = source_scope.authorized_refs_json();
    let analytics_range = AnalyticsTimeRange {
        since: effective_time_range.start,
        until: effective_time_range.end,
    };

    let data = match source.source_type.as_str() {
        "key_value" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.unit_hints = json_object([("updated_at", "relative_time")]);
            meta.ordering = vec![
                "ref".to_string(),
                "name".to_string(),
                "value".to_string(),
                "owner_type".to_string(),
                "owner_ref".to_string(),
                "updated_at".to_string(),
            ];

            let key_ref = resolve_optional_source_ref_param(source, request_filters, "ref")?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "Dashboard source '{}' requires param 'ref'",
                        source.source_id
                    ))
                })?;
            let expected_owner_type =
                resolve_optional_source_owner_type_param(source, request_filters, "owner_type")?;
            let expected_owner_ref =
                resolve_optional_source_string_param(source, request_filters, "owner_ref")?;
            let decrypt_requested =
                resolve_optional_source_bool_param(source, request_filters, "decrypt")?
                    .unwrap_or(false);

            build_key_value_source_data(
                state,
                user,
                effective_grants,
                &key_ref,
                expected_owner_type,
                expected_owner_ref.as_deref(),
                decrypt_requested,
            )
            .await?
            .map(serialized_row)
            .unwrap_or_else(|| JsonValue::Object(Default::default()))
        }
        "latest_action_result" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.unit_hints = json_object([("updated_at", "relative_time")]);
            meta.ordering = vec![
                "action_ref".to_string(),
                "execution_id".to_string(),
                "status".to_string(),
                "updated_at".to_string(),
                "result".to_string(),
            ];

            let action_refs = effective_action_refs(state, source_scope).await?;
            let statuses = resolve_optional_source_string_param(source, request_filters, "status")?
                .map(|status| vec![status])
                .unwrap_or_else(|| {
                    TERMINAL_EXECUTION_STATUSES
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                });
            let status_refs = statuses.iter().map(String::as_str).collect::<Vec<_>>();
            let rows = query_latest_execution_rows(
                state,
                effective_time_range,
                action_refs.as_ref(),
                &status_refs,
            )
            .await?;
            JsonValue::Array(
                rows.into_iter()
                    .map(|row| LatestActionResultSourceRow {
                        action_ref: row.action_ref,
                        execution_id: row.execution_id,
                        status: execution_status_label(row.status).to_string(),
                        updated_at: row.updated_at,
                        result: row.result,
                    })
                    .map(serialized_row)
                    .collect(),
            )
        }
        "action_result_path" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.unit_hints = json_object([("updated_at", "relative_time")]);
            meta.ordering = vec![
                "action_ref".to_string(),
                "execution_id".to_string(),
                "status".to_string(),
                "updated_at".to_string(),
                "path".to_string(),
                "value".to_string(),
            ];

            let action_refs = effective_action_refs(state, source_scope).await?;
            let path = resolve_required_result_path(source, request_filters)?;
            let rows = query_latest_execution_rows(
                state,
                effective_time_range,
                action_refs.as_ref(),
                &TERMINAL_EXECUTION_STATUSES,
            )
            .await?;

            if rows.is_empty() {
                JsonValue::Array(Vec::new())
            } else {
                let mut allowed_paths = BTreeSet::new();
                seed_default_action_result_paths(&mut allowed_paths);
                include_requested_derived_action_result_paths(&path, &mut allowed_paths);
                for row in &rows {
                    if let Some(result) = &row.result {
                        collect_json_paths(result, None, &mut allowed_paths);
                    }
                }
                ActionResultPathAllowList::new(allowed_paths.iter().cloned())
                    .require_allowed(&path)
                    .map_err(|_| {
                        ApiError::BadRequest(build_action_result_path_not_allowed_message(
                            &source.source_id,
                            &path,
                            &allowed_paths,
                        ))
                    })?;

                JsonValue::Array(
                    rows.into_iter()
                        .map(|row| ActionResultPathSourceRow {
                            action_ref: row.action_ref,
                            execution_id: row.execution_id,
                            status: execution_status_label(row.status).to_string(),
                            updated_at: row.updated_at,
                            path: path.clone(),
                            value: row
                                .result
                                .as_ref()
                                .and_then(|result| extract_json_path(result, &path))
                                .cloned()
                                .unwrap_or(JsonValue::Null),
                        })
                        .map(serialized_row)
                        .collect(),
                )
            }
        }
        "last_execution" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.unit_hints = json_object([
                ("created_at", "relative_time"),
                ("started_at", "relative_time"),
                ("updated_at", "relative_time"),
            ]);
            meta.ordering = vec![
                "action_ref".to_string(),
                "execution_id".to_string(),
                "status".to_string(),
                "created_at".to_string(),
                "started_at".to_string(),
                "updated_at".to_string(),
                "trace_tag".to_string(),
                "result".to_string(),
            ];

            let action_refs = effective_action_refs(state, source_scope).await?;
            let include_in_flight =
                resolve_optional_source_bool_param(source, request_filters, "include_in_flight")?
                    .unwrap_or(false);
            let rows = if include_in_flight {
                query_latest_execution_rows(
                    state,
                    effective_time_range,
                    action_refs.as_ref(),
                    &[
                        "requested",
                        "scheduling",
                        "scheduled",
                        "running",
                        "completed",
                        "failed",
                        "canceling",
                        "cancelled",
                        "timeout",
                        "abandoned",
                    ],
                )
                .await?
            } else {
                query_latest_execution_rows(
                    state,
                    effective_time_range,
                    action_refs.as_ref(),
                    &TERMINAL_EXECUTION_STATUSES,
                )
                .await?
            };
            JsonValue::Array(
                rows.into_iter()
                    .map(|row| LastExecutionSourceRow {
                        action_ref: row.action_ref,
                        execution_id: row.execution_id,
                        status: execution_status_label(row.status).to_string(),
                        created_at: row.created_at,
                        started_at: row.started_at,
                        updated_at: row.updated_at,
                        trace_tag: row.trace_tag,
                        result: row.result,
                    })
                    .map(serialized_row)
                    .collect(),
            )
        }
        "execution_count" | "execution_timeseries" => {
            let action_refs = effective_action_refs(state, source_scope).await?;
            let rows = execute_execution_throughput_with_cutover(
                state,
                effective_time_range,
                action_refs.as_ref(),
            )
            .await?;
            meta.freshness_mode = rows.freshness_mode;
            meta.aggregate_watermark = rows.aggregate_watermark;
            meta.unit_hints = json_object([("count", "count")]);
            meta.ordering = vec!["bucket_start".to_string(), "series".to_string()];
            JsonValue::Array(
                rows.data
                    .into_iter()
                    .map(|row| BucketCountSourceRow {
                        bucket_start: row.bucket_start,
                        series: row.series,
                        count: row.count,
                    })
                    .map(serialized_row)
                    .collect(),
            )
        }
        "execution_status_breakdown" => {
            let action_refs = effective_action_refs(state, source_scope).await?;
            let rows = execute_execution_status_with_cutover(
                state,
                effective_time_range,
                action_refs.as_ref(),
            )
            .await?;
            meta.freshness_mode = rows.freshness_mode;
            meta.aggregate_watermark = rows.aggregate_watermark;
            meta.unit_hints = json_object([("count", "count")]);
            meta.ordering = vec!["bucket_start".to_string(), "status".to_string()];
            JsonValue::Array(
                rows.data
                    .into_iter()
                    .map(|row| ExecutionStatusSourceRow {
                        bucket_start: row.bucket_start,
                        status: row.series,
                        count: row.count,
                    })
                    .map(serialized_row)
                    .collect(),
            )
        }
        "execution_duration_stats" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.bucket_size = Some(
                resolve_source_param_bucket_size(source, request_filters)?
                    .unwrap_or_else(|| "1h".to_string()),
            );
            meta.unit_hints = json_object([
                ("execution_count", "count"),
                ("avg_duration_seconds", "seconds"),
                ("p50_duration_seconds", "seconds"),
                ("p95_duration_seconds", "seconds"),
                ("max_duration_seconds", "seconds"),
            ]);
            meta.ordering = vec!["bucket_start".to_string(), "series".to_string()];
            let action_refs = effective_action_refs(state, source_scope).await?;
            let rows = query_execution_duration_stats_rows(
                state,
                effective_time_range,
                action_refs.as_ref(),
            )
            .await?;
            JsonValue::Array(rows.into_iter().map(serialized_row).collect())
        }
        "event_count" | "event_timeseries" => {
            let trigger_refs = effective_trigger_refs(state, source_scope).await?;
            let rows = execute_event_volume_with_cutover(
                state,
                effective_time_range,
                trigger_refs.as_ref(),
            )
            .await?;
            meta.freshness_mode = rows.freshness_mode;
            meta.aggregate_watermark = rows.aggregate_watermark;
            meta.ordering = vec!["bucket_start".to_string(), "series".to_string()];
            JsonValue::Array(
                rows.data
                    .into_iter()
                    .map(|row| {
                        serde_json::json!({
                            "bucket_start": row.bucket_start,
                            "series": row.series,
                            "count": row.count
                        })
                    })
                    .collect(),
            )
        }
        "last_event" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.ordering = vec!["trigger_ref".to_string(), "event_id".to_string()];
            let trigger_refs = effective_trigger_refs(state, source_scope).await?;
            let rows =
                query_last_event_rows(state, effective_time_range, trigger_refs.as_ref()).await?;
            JsonValue::Array(rows.into_iter().map(serialized_row).collect())
        }
        "enforcement_count" | "enforcement_timeseries" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            let rows = if let Some(rule_refs) = effective_rule_refs(state, source_scope).await? {
                let mut rows = Vec::new();
                for rule_ref in rule_refs {
                    rows.extend(
                        AnalyticsRepository::enforcement_volume_hourly_by_rule(
                            &state.db,
                            &analytics_range,
                            &rule_ref,
                        )
                        .await?,
                    );
                }
                rows
            } else {
                AnalyticsRepository::enforcement_volume_hourly(&state.db, &analytics_range).await?
            };
            meta.ordering = vec!["bucket_start".to_string(), "series".to_string()];
            JsonValue::Array(
                rows.into_iter()
                    .map(|row| {
                        serde_json::json!({
                            "bucket_start": row.bucket,
                            "series": row.rule_ref.unwrap_or_else(|| "all".to_string()),
                            "count": row.enforcement_count
                        })
                    })
                    .collect(),
            )
        }
        "last_enforcement" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.ordering = vec!["rule_ref".to_string(), "enforcement_id".to_string()];
            let rule_refs = effective_rule_refs(state, source_scope).await?;
            let rows = query_last_enforcement_rows(state, effective_time_range, rule_refs.as_ref())
                .await?;
            JsonValue::Array(rows.into_iter().map(serialized_row).collect())
        }
        "inquiry_backlog" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.unit_hints = json_object([("pending_count", "count"), ("overdue_count", "count")]);
            meta.ordering = vec!["pack_ref".to_string(), "assigned_to".to_string()];
            let assigned_to = resolve_source_param_i64(source, request_filters, "assigned_to")?;
            let rows =
                query_inquiry_backlog_rows(state, source_scope.pack_refs.as_ref(), assigned_to)
                    .await?;
            JsonValue::Array(rows.into_iter().map(serialized_row).collect())
        }
        "inquiry_sla" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.bucket_size = Some(
                resolve_source_param_bucket_size(source, request_filters)?
                    .unwrap_or_else(|| "1h".to_string()),
            );
            meta.unit_hints = json_object([
                ("sla_target_seconds", "seconds"),
                ("total_inquiries", "count"),
                ("within_sla_count", "count"),
                ("breached_count", "count"),
                ("open_count", "count"),
                ("compliance_rate", "ratio"),
            ]);
            meta.ordering = vec![
                "bucket_start".to_string(),
                "pack_ref".to_string(),
                "assigned_to".to_string(),
            ];
            let assigned_to = resolve_source_param_i64(source, request_filters, "assigned_to")?;
            let sla_target_seconds =
                resolve_source_param_i64(source, request_filters, "sla_target_seconds")?
                    .unwrap_or(DEFAULT_INQUIRY_SLA_TARGET_SECONDS);
            if sla_target_seconds <= 0 {
                return Err(ApiError::BadRequest(format!(
                    "Dashboard source '{}' param 'sla_target_seconds' must be greater than zero",
                    source.source_id
                )));
            }
            let rows = query_inquiry_sla_rows(
                state,
                effective_time_range,
                source_scope.pack_refs.as_ref(),
                assigned_to,
                sla_target_seconds,
            )
            .await?;
            JsonValue::Array(rows.into_iter().map(serialized_row).collect())
        }
        "worker_health" | "worker_status" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.ordering = vec![
                "worker_role".to_string(),
                "worker_name".to_string(),
                "worker_id".to_string(),
            ];
            let requested_status =
                resolve_optional_source_string_param(source, request_filters, "status")?;
            let requested_worker_role =
                resolve_optional_source_string_param(source, request_filters, "worker_role")?;
            let _requested_history =
                resolve_optional_source_bool_param(source, request_filters, "history")?;
            let mut rows = WorkerRepository::list(&state.db).await?;
            rows.retain(|row| {
                let status = row.status.map(worker_status_label).unwrap_or("unknown");
                let role = worker_role_label(row.worker_role);
                requested_status
                    .as_ref()
                    .is_none_or(|expected| status == expected)
                    && requested_worker_role
                        .as_ref()
                        .is_none_or(|expected| role == expected)
            });
            rows.sort_by(|a, b| {
                worker_role_label(a.worker_role)
                    .cmp(worker_role_label(b.worker_role))
                    .then_with(|| a.id.cmp(&b.id))
            });
            JsonValue::Array(
                rows.into_iter()
                    .map(|row| WorkerHealthSourceRow {
                        worker_id: row.id,
                        worker_name: row.name,
                        worker_role: worker_role_label(row.worker_role).to_string(),
                        status: row
                            .status
                            .map(worker_status_label)
                            .unwrap_or("unknown")
                            .to_string(),
                        cordoned: row.cordoned,
                    })
                    .map(serialized_row)
                    .collect(),
            )
        }
        "sensor_health" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.unit_hints = json_object([
                ("active_rule_count", "count"),
                ("consecutive_failures", "count"),
            ]);
            meta.ordering = vec!["sensor_ref".to_string(), "worker_id".to_string()];
            let sensor_ref =
                resolve_optional_source_ref_param(source, request_filters, "sensor_ref")?;
            let worker_id =
                resolve_optional_source_i64_param(source, request_filters, "worker_id")?;
            let updated_since = resolve_optional_source_window_start(
                source,
                request_filters,
                effective_time_range,
            )?;
            let pack_refs = source_scope.pack_refs.clone();
            let mut rows = SensorProcessRepository::list(&state.db).await?;
            rows.retain(|row| {
                row.updated >= updated_since
                    && sensor_ref
                        .as_ref()
                        .is_none_or(|expected| row.sensor_ref == *expected)
                    && worker_id.is_none_or(|expected| row.worker == expected)
                    && pack_refs.as_ref().is_none_or(|allowed| {
                        row.sensor_ref
                            .split_once('.')
                            .is_some_and(|(pack_ref, _)| allowed.contains(pack_ref))
                    })
            });
            rows.sort_by(|a, b| {
                a.sensor_ref
                    .cmp(&b.sensor_ref)
                    .then_with(|| a.worker.cmp(&b.worker))
            });
            JsonValue::Array(
                rows.into_iter()
                    .map(|row| SensorHealthSourceRow {
                        sensor_ref: row.sensor_ref,
                        worker_id: row.worker,
                        worker_name: row.worker_name,
                        health: sensor_process_health_label(row.status).to_string(),
                        status: sensor_process_status_label(row.status).to_string(),
                        active_rule_count: row.active_rule_count,
                        consecutive_failures: row.consecutive_failures,
                        pid: row.pid,
                        last_started_at: row.last_started_at,
                        last_stopped_at: row.last_stopped_at,
                        next_restart_at: row.next_restart_at,
                        last_exit_code: row.last_exit_code,
                        last_signal: row.last_signal,
                        log_artifact_ref: row.log_artifact_ref,
                        updated: row.updated,
                    })
                    .map(serialized_row)
                    .collect(),
            )
        }
        "queue_backlog" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.unit_hints = json_object([
                ("queued", "count"),
                ("retry", "count"),
                ("leased", "count"),
                ("total_backlog", "count"),
            ]);
            meta.ordering = vec!["queue_ref".to_string()];
            let queue_refs = effective_queue_refs(state, source_scope).await?;
            let queue_ref_filter = queue_refs
                .as_ref()
                .map(|set| set.iter().cloned().collect::<Vec<String>>());
            let rows =
                WorkQueueItemRepository::backlog_summary(&state.db, queue_ref_filter.as_deref())
                    .await?;
            JsonValue::Array(
                rows.into_iter()
                    .map(|row| QueueBacklogSourceRow {
                        queue_ref: row.queue_ref,
                        queued: row.queued,
                        retry: row.retry,
                        leased: row.leased,
                        total_backlog: row.total_backlog,
                    })
                    .map(serialized_row)
                    .collect(),
            )
        }
        "queue_throughput" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.bucket_size = Some("1h".to_string());
            meta.unit_hints = json_object([
                ("completed", "count"),
                ("failed", "count"),
                ("skipped", "count"),
                ("cancelled", "count"),
                ("total_processed", "count"),
            ]);
            meta.ordering = vec!["bucket_start".to_string(), "queue_ref".to_string()];
            let queue_refs = effective_queue_refs(state, source_scope).await?;
            let rows =
                query_queue_throughput_rows(state, effective_time_range, queue_refs.as_ref())
                    .await?;
            JsonValue::Array(rows.into_iter().map(serialized_row).collect())
        }
        "queue_dispatch_stats" => {
            meta.freshness_mode = DashboardFreshnessMode::RawOnly;
            meta.bucket_size = Some("1h".to_string());
            meta.unit_hints = json_object([
                ("dispatch_count", "count"),
                ("leased_item_count", "count"),
                ("avg_duration_seconds", "seconds"),
                ("max_duration_seconds", "seconds"),
            ]);
            meta.ordering = vec![
                "bucket_start".to_string(),
                "queue_ref".to_string(),
                "status".to_string(),
            ];
            let queue_refs = effective_queue_refs(state, source_scope).await?;
            let rows =
                query_queue_dispatch_stats_rows(state, effective_time_range, queue_refs.as_ref())
                    .await?;
            JsonValue::Array(rows.into_iter().map(serialized_row).collect())
        }
        _ => return Ok(unsupported_source_result(source, "unsupported")),
    };

    let data = suppress_small_cohort_rows(
        data,
        source.source_type.as_str(),
        source_scope.authorization_mode,
    );
    let (data, truncated) = truncate_source_data(data);
    meta.truncated = truncated;
    let status = if data.as_array().is_some_and(|rows| rows.is_empty())
        || data.as_object().is_some_and(|object| object.is_empty())
    {
        DashboardSourceStatus::Empty
    } else if truncated {
        DashboardSourceStatus::Partial
    } else {
        DashboardSourceStatus::Ok
    };

    Ok(DashboardSourceResult {
        source_id: source.source_id.clone(),
        source_type: source.source_type.clone(),
        status,
        data: Some(data),
        meta,
        error: None,
    })
}

fn truncate_source_data(data: JsonValue) -> (JsonValue, bool) {
    match data {
        JsonValue::Array(mut rows) if rows.len() > SOURCE_ROW_CAP => {
            rows.truncate(SOURCE_ROW_CAP);
            (JsonValue::Array(rows), true)
        }
        other => (other, false),
    }
}

fn suppress_small_cohort_rows(
    data: JsonValue,
    source_type: &str,
    authorization_mode: DashboardAuthorizationMode,
) -> JsonValue {
    if !matches!(
        authorization_mode,
        DashboardAuthorizationMode::IdentityFiltered
    ) || !small_cohort_suppression_enabled(source_type)
    {
        return data;
    }

    match data {
        JsonValue::Array(rows) => JsonValue::Array(
            rows.into_iter()
                .filter(|row| !is_small_cohort_row(row))
                .collect(),
        ),
        other => other,
    }
}

fn small_cohort_suppression_enabled(source_type: &str) -> bool {
    matches!(
        source_type,
        "execution_count"
            | "execution_timeseries"
            | "execution_status_breakdown"
            | "execution_duration_stats"
            | "event_count"
            | "event_timeseries"
            | "enforcement_count"
            | "enforcement_timeseries"
            | "inquiry_backlog"
            | "inquiry_sla"
            | "queue_backlog"
            | "queue_throughput"
            | "queue_dispatch_stats"
    )
}

fn is_small_cohort_row(row: &JsonValue) -> bool {
    let JsonValue::Object(object) = row else {
        return false;
    };

    let mut observed_metric = false;
    let mut all_observed_metrics_small = true;
    for key in [
        "count",
        "execution_count",
        "event_count",
        "enforcement_count",
        "pending_count",
        "overdue_count",
        "queued",
        "retry",
        "leased",
        "total_backlog",
        "completed",
        "failed",
        "skipped",
        "cancelled",
        "total_processed",
        "dispatch_count",
        "leased_item_count",
        "total_inquiries",
        "within_sla_count",
        "breached_count",
        "open_count",
    ] {
        let Some(value) = object.get(key).and_then(|value| value.as_i64()) else {
            continue;
        };
        observed_metric = true;
        if value >= SMALL_COHORT_MIN_COUNT {
            all_observed_metrics_small = false;
            break;
        }
    }

    observed_metric && all_observed_metrics_small
}

fn unsupported_source_result(source: &DashboardSourceDef, reason: &str) -> DashboardSourceResult {
    DashboardSourceResult {
        source_id: source.source_id.clone(),
        source_type: source.source_type.clone(),
        status: DashboardSourceStatus::Invalid,
        data: None,
        meta: default_source_meta(),
        error: Some(DashboardSourceError {
            code: "unsupported".to_string(),
            message: format!(
                "No executable handler for source type '{}' ({})",
                source.source_type, reason
            ),
            retryable: false,
            details: None,
        }),
    }
}

impl DashboardSourceCache {
    async fn get_fresh(&self, key: &str) -> Option<DashboardSourceResult> {
        let now = Instant::now();
        let entries = self.entries.lock().await;
        entries
            .get(key)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.result.clone())
    }

    async fn get_stale(&self, key: &str) -> Option<DashboardSourceResult> {
        let now = Instant::now();
        let entries = self.entries.lock().await;
        entries
            .get(key)
            .filter(|entry| entry.stale_at > now)
            .map(|entry| entry.result.clone())
    }

    async fn insert(&self, key: String, result: DashboardSourceResult) {
        self.insert_with_ttls(key, result, SOURCE_CACHE_TTL, SOURCE_CACHE_STALE_TTL)
            .await;
    }
}

async fn resolve_dashboard_for_user(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    dashboard_ref: &str,
) -> Result<Dashboard, ApiError> {
    RefValidator::validate_component_ref(dashboard_ref)
        .map_err(|e| ApiError::BadRequest(format!("Invalid dashboard ref: {e}")))?;

    let identity_id = user.identity_id().ok();
    let dashboard = DashboardRepository::find_visible_by_ref(&state.db, dashboard_ref, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Dashboard '{}' not found", dashboard_ref)))?;

    if !dashboard.enabled {
        return Err(ApiError::NotFound(format!(
            "Dashboard '{}' not found",
            dashboard_ref
        )));
    }

    if dashboard.visibility == DashboardVisibility::Private
        && (identity_id.is_none() || dashboard.owner_identity != identity_id)
    {
        return Err(ApiError::Forbidden(
            "Not authorized to access private dashboard".to_string(),
        ));
    }

    authorize_dashboard_access(state, user, &dashboard).await?;
    Ok(dashboard)
}

async fn resolve_dashboard_for_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    dashboard_ref: &str,
    action: RbacAction,
) -> Result<Dashboard, ApiError> {
    RefValidator::validate_component_ref(dashboard_ref)
        .map_err(|e| ApiError::BadRequest(format!("Invalid dashboard ref: {e}")))?;

    let identity_id = user.identity_id().ok();
    let dashboard = DashboardRepository::find_visible_by_ref(&state.db, dashboard_ref, identity_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Dashboard '{}' not found", dashboard_ref)))?;

    if dashboard.visibility == DashboardVisibility::Private
        && (identity_id.is_none() || dashboard.owner_identity != identity_id)
    {
        return Err(ApiError::Forbidden(
            "Not authorized to access private dashboard".to_string(),
        ));
    }

    authorize_dashboard_action(state, user, action, &dashboard).await?;
    Ok(dashboard)
}

async fn authorize_dashboard_create(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    shape: &DashboardWriteShape,
) -> Result<(), ApiError> {
    let context = dashboard_authorization_context_for_shape(actor_identity_or_zero(user), shape);
    authorize_dashboard_context_action(state, user, RbacAction::Create, context).await
}

async fn authorize_dashboard_preview(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    shape: &DashboardWriteShape,
) -> Result<(), ApiError> {
    let existing =
        DashboardRepository::find_visible_by_ref(&state.db, &shape.r#ref, user.identity_id().ok())
            .await?;
    if let Some(dashboard) = existing {
        if dashboard.visibility == DashboardVisibility::Private
            && (user.identity_id().ok().is_none()
                || dashboard.owner_identity != user.identity_id().ok())
        {
            return Err(ApiError::Forbidden(
                "Not authorized to access private dashboard".to_string(),
            ));
        }
        authorize_dashboard_action(state, user, RbacAction::Read, &dashboard).await
    } else {
        authorize_dashboard_create(state, user, shape).await
    }
}

async fn authorize_dashboard_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    dashboard: &Dashboard,
) -> Result<(), ApiError> {
    let context =
        dashboard_authorization_context_for_dashboard(actor_identity_or_zero(user), dashboard);
    authorize_dashboard_context_action(state, user, action, context).await
}

async fn authorize_dashboard_context_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    context: AuthorizationContext,
) -> Result<(), ApiError> {
    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Dashboards,
                action,
                context,
            },
        )
        .await
}

fn dashboard_authorization_context_for_dashboard(
    identity_id: i64,
    dashboard: &Dashboard,
) -> AuthorizationContext {
    let mut context = AuthorizationContext::new(identity_id);
    context.target_id = Some(dashboard.id);
    context.target_ref = Some(dashboard.r#ref.clone());
    context.pack_ref = dashboard_pack_ref(&dashboard.r#ref);
    context.owner_identity_id = dashboard.owner_identity;
    context
}

fn dashboard_authorization_context_for_shape(
    identity_id: i64,
    shape: &DashboardWriteShape,
) -> AuthorizationContext {
    let mut context = AuthorizationContext::new(identity_id);
    context.target_ref = Some(shape.r#ref.clone());
    context.pack_ref = dashboard_pack_ref(&shape.r#ref);
    context.owner_identity_id = shape.owner_identity;
    context
}

fn actor_identity_id(user: &AuthenticatedUser) -> Result<Option<i64>, ApiError> {
    match user.claims.token_type {
        crate::auth::jwt::TokenType::Access | crate::auth::jwt::TokenType::Execution => user
            .identity_id()
            .map(Some)
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string())),
        _ => Ok(None),
    }
}

fn actor_identity_or_zero(user: &AuthenticatedUser) -> i64 {
    actor_identity_id(user).ok().flatten().unwrap_or_default()
}

fn dashboard_pack_ref(dashboard_ref: &str) -> Option<String> {
    dashboard_ref
        .split_once('.')
        .map(|(pack_ref, _)| pack_ref.to_string())
}

fn dashboard_scope_label(scope_type: DashboardScopeType) -> &'static str {
    match scope_type {
        DashboardScopeType::Global => "global",
        DashboardScopeType::Pack => "pack",
        DashboardScopeType::Identity => "identity",
        DashboardScopeType::Tenant => "tenant",
    }
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

fn revision_conflict_error(
    dashboard_ref: &str,
    expected_revision: i32,
    current_revision: i32,
) -> ApiError {
    ApiError::Conflict(format!(
        "Dashboard '{}' revision mismatch: expected revision {}, found {}",
        dashboard_ref, expected_revision, current_revision
    ))
}

fn normalize_create_dashboard_request(
    user: &AuthenticatedUser,
    request: CreateDashboardRequest,
) -> Result<DashboardWriteShape, ApiError> {
    let enabled = request.enabled.unwrap_or(true);
    let is_default_home = request.is_default_home.unwrap_or(false);
    let spec_version = request.spec_version.unwrap_or(1);
    let tags = normalize_tags(&request.tags);
    let (scope_type, scope_ref, owner_identity, visibility) = normalize_dashboard_scope(
        user,
        &request.r#ref,
        request.scope_type,
        request.scope_ref.as_deref(),
        request.visibility,
        None,
    )?;

    finalize_dashboard_shape(DashboardWriteShape {
        r#ref: request.r#ref,
        label: request.label,
        description: request.description,
        scope_type,
        scope_ref,
        visibility,
        enabled,
        is_default_home,
        spec_version,
        spec: request.spec,
        tags,
        owner_identity,
    })
}

fn normalize_update_dashboard_request(
    user: &AuthenticatedUser,
    dashboard: &Dashboard,
    request: UpdateDashboardRequest,
) -> Result<DashboardWriteShape, ApiError> {
    let description = match request.description {
        Some(crate::dto::dashboard::DashboardDescriptionPatch::Patch(
            crate::dto::runtime::NullableStringPatch::Set(value),
        )) => Some(value),
        Some(crate::dto::dashboard::DashboardDescriptionPatch::Patch(
            crate::dto::runtime::NullableStringPatch::Clear,
        )) => None,
        Some(crate::dto::dashboard::DashboardDescriptionPatch::Value(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        None => dashboard.description.clone(),
    };
    let tags = request
        .tags
        .as_ref()
        .map(|value| normalize_tags(value))
        .unwrap_or_else(|| dashboard.tags.clone());
    let effective_scope_type = request.scope_type.unwrap_or(dashboard.scope_type);
    let effective_visibility = match (effective_scope_type, request.visibility) {
        (DashboardScopeType::Identity, None) => DashboardVisibility::Private,
        (_, Some(visibility)) => visibility,
        (_, None) => dashboard.visibility,
    };
    let requested_scope_ref = if request.scope_ref.is_some() {
        request.scope_ref.as_deref()
    } else if request.scope_type.is_some() {
        None
    } else {
        Some(dashboard.scope_ref.as_str())
    };

    let (scope_type, scope_ref, owner_identity, visibility) = normalize_dashboard_scope(
        user,
        &dashboard.r#ref,
        effective_scope_type,
        requested_scope_ref,
        effective_visibility,
        dashboard.owner_identity,
    )?;

    finalize_dashboard_shape(DashboardWriteShape {
        r#ref: dashboard.r#ref.clone(),
        label: request.label.unwrap_or_else(|| dashboard.label.clone()),
        description,
        scope_type,
        scope_ref,
        visibility,
        enabled: request.enabled.unwrap_or(dashboard.enabled),
        is_default_home: request.is_default_home.unwrap_or(dashboard.is_default_home),
        spec_version: request.spec_version.unwrap_or(dashboard.spec_version),
        spec: request.spec.unwrap_or_else(|| dashboard.spec.clone()),
        tags,
        owner_identity,
    })
}

fn normalize_clone_dashboard_request(
    user: &AuthenticatedUser,
    dashboard: &Dashboard,
    request: CloneDashboardRequest,
) -> Result<DashboardWriteShape, ApiError> {
    let (scope_type, scope_ref, owner_identity, visibility) = normalize_dashboard_scope(
        user,
        &request.r#ref,
        dashboard.scope_type,
        Some(dashboard.scope_ref.as_str()),
        dashboard.visibility,
        dashboard.owner_identity,
    )?;

    finalize_dashboard_shape(DashboardWriteShape {
        r#ref: request.r#ref,
        label: dashboard.label.clone(),
        description: dashboard.description.clone(),
        scope_type,
        scope_ref,
        visibility,
        enabled: dashboard.enabled,
        is_default_home: false,
        spec_version: dashboard.spec_version,
        spec: dashboard.spec.clone(),
        tags: normalize_tags(&dashboard.tags),
        owner_identity,
    })
}

fn normalize_dashboard_scope(
    user: &AuthenticatedUser,
    dashboard_ref: &str,
    scope_type: DashboardScopeType,
    scope_ref: Option<&str>,
    visibility: DashboardVisibility,
    existing_owner_identity: Option<i64>,
) -> Result<(DashboardScopeType, String, Option<i64>, DashboardVisibility), ApiError> {
    match scope_type {
        DashboardScopeType::Global => {
            if let Some(scope_ref) = scope_ref.map(str::trim) {
                if !scope_ref.is_empty() && scope_ref != "global" {
                    return Err(ApiError::BadRequest(
                        "Global dashboards must use scope_ref 'global'".to_string(),
                    ));
                }
            }
            let owner_identity = if visibility == DashboardVisibility::Private {
                actor_identity_id(user)?.or(existing_owner_identity)
            } else {
                None
            };
            if visibility == DashboardVisibility::Private && owner_identity.is_none() {
                return Err(ApiError::Forbidden(
                    "Private dashboards require an access or execution identity".to_string(),
                ));
            }
            Ok((scope_type, "global".to_string(), owner_identity, visibility))
        }
        DashboardScopeType::Pack => {
            let pack_ref = dashboard_pack_ref(dashboard_ref).ok_or_else(|| {
                ApiError::BadRequest(
                    "Pack-scoped dashboards require a ref with a pack prefix".to_string(),
                )
            })?;
            if let Some(provided_scope_ref) =
                scope_ref.map(str::trim).filter(|value| !value.is_empty())
            {
                if provided_scope_ref != pack_ref {
                    return Err(ApiError::BadRequest(format!(
                        "Pack-scoped dashboards must use scope_ref matching dashboard ref pack prefix '{}'",
                        pack_ref
                    )));
                }
            }
            let owner_identity = if visibility == DashboardVisibility::Private {
                actor_identity_id(user)?.or(existing_owner_identity)
            } else {
                None
            };
            if visibility == DashboardVisibility::Private && owner_identity.is_none() {
                return Err(ApiError::Forbidden(
                    "Private dashboards require an access or execution identity".to_string(),
                ));
            }
            Ok((scope_type, pack_ref, owner_identity, visibility))
        }
        DashboardScopeType::Identity => {
            if visibility != DashboardVisibility::Private {
                return Err(ApiError::BadRequest(
                    "Identity-scoped dashboards must use private visibility".to_string(),
                ));
            }
            let identity_id = actor_identity_id(user)?.ok_or_else(|| {
                ApiError::Forbidden(
                    "Identity-scoped dashboards require an access token".to_string(),
                )
            })?;
            let expected_scope_ref = identity_id.to_string();
            if let Some(scope_ref) = scope_ref.map(str::trim) {
                if !scope_ref.is_empty() && scope_ref != expected_scope_ref {
                    return Err(ApiError::BadRequest(format!(
                        "Identity-scoped dashboards must use scope_ref matching the authenticated identity '{}'",
                        expected_scope_ref
                    )));
                }
            }
            Ok((
                scope_type,
                expected_scope_ref,
                Some(identity_id),
                DashboardVisibility::Private,
            ))
        }
        DashboardScopeType::Tenant => Err(ApiError::BadRequest(
            "Tenant-scoped dashboards are not supported by this API".to_string(),
        )),
    }
}

fn finalize_dashboard_shape(shape: DashboardWriteShape) -> Result<DashboardWriteShape, ApiError> {
    let spec = canonicalize_dashboard_spec(&shape)?;
    index_dashboard_spec(&spec)?;

    Ok(DashboardWriteShape { spec, ..shape })
}

fn canonicalize_dashboard_spec(shape: &DashboardWriteShape) -> Result<JsonValue, ApiError> {
    let mut spec_object = shape.spec.as_object().cloned().ok_or_else(|| {
        ApiError::UnprocessableEntity("Dashboard spec must be a JSON object".to_string())
    })?;

    spec_object.insert("ref".to_string(), JsonValue::String(shape.r#ref.clone()));
    spec_object.insert("label".to_string(), JsonValue::String(shape.label.clone()));
    match &shape.description {
        Some(description) => {
            spec_object.insert(
                "description".to_string(),
                JsonValue::String(description.clone()),
            );
        }
        None => {
            spec_object.remove("description");
        }
    }
    spec_object.insert(
        "scope_type".to_string(),
        serde_json::to_value(shape.scope_type).unwrap_or(JsonValue::Null),
    );
    spec_object.insert(
        "scope_ref".to_string(),
        JsonValue::String(shape.scope_ref.clone()),
    );
    spec_object.insert(
        "visibility".to_string(),
        serde_json::to_value(shape.visibility).unwrap_or(JsonValue::Null),
    );
    spec_object.insert("enabled".to_string(), JsonValue::Bool(shape.enabled));
    spec_object.insert(
        "is_default_home".to_string(),
        JsonValue::Bool(shape.is_default_home),
    );
    spec_object.insert(
        "spec_version".to_string(),
        JsonValue::Number(shape.spec_version.into()),
    );
    spec_object.insert(
        "tags".to_string(),
        JsonValue::Array(shape.tags.iter().cloned().map(JsonValue::String).collect()),
    );

    Ok(JsonValue::Object(spec_object))
}

fn dashboard_matches_shape(dashboard: &Dashboard, shape: &DashboardWriteShape) -> bool {
    dashboard.r#ref == shape.r#ref
        && dashboard.label == shape.label
        && dashboard.description == shape.description
        && dashboard.scope_type == shape.scope_type
        && dashboard.scope_ref == shape.scope_ref
        && dashboard.visibility == shape.visibility
        && dashboard.enabled == shape.enabled
        && dashboard.is_default_home == shape.is_default_home
        && dashboard.spec_version == shape.spec_version
        && dashboard.spec == shape.spec
        && dashboard.tags == shape.tags
        && dashboard.owner_identity == shape.owner_identity
}

async fn ensure_dashboard_scope_available<'e, E>(
    executor: E,
    dashboard_ref: &str,
    scope_type: DashboardScopeType,
    scope_ref: &str,
    exclude_dashboard_id: Option<i64>,
) -> Result<(), ApiError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres> + 'e,
{
    if let Some(existing) = DashboardRepository::find_by_ref_in_scope(
        executor,
        &DashboardScopedRef {
            scope_type,
            scope_ref: scope_ref.to_string(),
            r#ref: dashboard_ref.to_string(),
        },
    )
    .await?
    {
        if Some(existing.id) != exclude_dashboard_id {
            return Err(ApiError::Conflict(format!(
                "Dashboard '{}' already exists in scope '{}:{}'",
                dashboard_ref,
                dashboard_scope_label(scope_type),
                scope_ref
            )));
        }
    }
    Ok(())
}

async fn clear_prior_default_home_if_needed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    shape: &DashboardWriteShape,
    exclude_dashboard_id: Option<i64>,
    updated_by: Option<i64>,
) -> Result<(), ApiError> {
    if !shape.is_default_home {
        return Ok(());
    }

    let existing_default = DashboardRepository::find_default_home_in_scope(
        &mut **tx,
        shape.scope_type,
        &shape.scope_ref,
    )
    .await?;
    let Some(existing_default) = existing_default else {
        return Ok(());
    };
    if Some(existing_default.id) == exclude_dashboard_id {
        return Ok(());
    }

    let cleared_shape = finalize_dashboard_shape(DashboardWriteShape {
        r#ref: existing_default.r#ref.clone(),
        label: existing_default.label.clone(),
        description: existing_default.description.clone(),
        scope_type: existing_default.scope_type,
        scope_ref: existing_default.scope_ref.clone(),
        visibility: existing_default.visibility,
        enabled: existing_default.enabled,
        is_default_home: false,
        spec_version: existing_default.spec_version,
        spec: existing_default.spec.clone(),
        tags: existing_default.tags.clone(),
        owner_identity: existing_default.owner_identity,
    })?;

    let _ = persist_dashboard_update(
        tx,
        existing_default.id,
        existing_default.revision,
        &cleared_shape,
        false,
        updated_by,
        &existing_default.r#ref,
    )
    .await?;
    Ok(())
}

async fn persist_dashboard_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dashboard_id: i64,
    expected_revision: i32,
    shape: &DashboardWriteShape,
    enforce_revision: bool,
    updated_by: Option<i64>,
    dashboard_ref: &str,
) -> Result<Dashboard, ApiError> {
    let updated = DashboardRepository::update(
        &mut **tx,
        dashboard_id,
        UpdateDashboardInput {
            scope_type: Some(shape.scope_type),
            scope_ref: Some(shape.scope_ref.clone()),
            pack: None,
            owner_identity: Some(match shape.owner_identity {
                Some(identity_id) => Patch::Set(identity_id),
                None => Patch::Clear,
            }),
            visibility: Some(shape.visibility),
            is_adhoc: None,
            label: Some(shape.label.clone()),
            description: Some(match &shape.description {
                Some(description) => Patch::Set(description.clone()),
                None => Patch::Clear,
            }),
            enabled: Some(shape.enabled),
            is_default_home: Some(shape.is_default_home),
            spec_version: Some(shape.spec_version),
            spec: Some(shape.spec.clone()),
            tags: Some(shape.tags.clone()),
            expected_revision: enforce_revision.then_some(expected_revision),
            updated_by,
        },
    )
    .await
    .map_err(|err| match err {
        attune_common::error::Error::InvalidState(_) if enforce_revision => {
            ApiError::Conflict(format!(
                "Dashboard '{}' revision mismatch: expected revision {}",
                dashboard_ref, expected_revision
            ))
        }
        other => other.into(),
    })?;

    DashboardVersionRepository::create(
        &mut **tx,
        CreateDashboardVersionInput {
            dashboard: updated.id,
            revision: updated.revision,
            spec_version: updated.spec_version,
            spec: updated.spec.clone(),
            created_by: updated_by,
        },
    )
    .await?;

    Ok(updated)
}

async fn authorize_dashboard_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    dashboard: &Dashboard,
) -> Result<(), ApiError> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;

    let mut context = AuthorizationContext::new(identity_id);
    context.target_id = Some(dashboard.id);
    context.target_ref = Some(dashboard.r#ref.clone());
    context.pack_ref = dashboard
        .r#ref
        .split_once('.')
        .map(|(pack_ref, _)| pack_ref.to_string());
    context.owner_identity_id = dashboard.owner_identity;

    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Dashboards,
                action: RbacAction::Read,
                context,
            },
        )
        .await
}

fn index_dashboard_spec(spec: &JsonValue) -> Result<DashboardSpecIndex, ApiError> {
    validate_dashboard_spec(spec).map_err(ApiError::UnprocessableEntity)?;

    let spec_object = spec.as_object().ok_or_else(|| {
        ApiError::UnprocessableEntity("Dashboard spec must be a JSON object".to_string())
    })?;

    let defaults_time_window = spec_object
        .get("defaults")
        .and_then(|defaults| defaults.as_object())
        .and_then(|defaults| defaults.get("time_window"))
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);

    let defaults_timezone = spec_object
        .get("defaults")
        .and_then(|defaults| defaults.as_object())
        .and_then(|defaults| defaults.get("timezone"))
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);

    let mut filters = HashMap::new();
    if let Some(filter_values) = spec_object.get("filters") {
        let filter_array = filter_values.as_array().ok_or_else(|| {
            ApiError::UnprocessableEntity("Dashboard spec 'filters' must be an array".to_string())
        })?;

        for filter in filter_array {
            let filter_object = filter.as_object().ok_or_else(|| {
                ApiError::UnprocessableEntity(
                    "Dashboard spec filter entries must be objects".to_string(),
                )
            })?;
            let filter_id = filter_object
                .get("id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    ApiError::UnprocessableEntity(
                        "Dashboard spec filter entries require string 'id'".to_string(),
                    )
                })?
                .to_string();

            filters.insert(
                filter_id,
                DashboardFilterDef {
                    filter_type: filter_object
                        .get("type")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned),
                    options: filter_object
                        .get("options")
                        .and_then(|value| value.as_array())
                        .cloned(),
                },
            );
        }
    }

    let source_object = spec_object
        .get("data_sources")
        .and_then(|value| value.as_object())
        .ok_or_else(|| {
            ApiError::UnprocessableEntity(
                "Dashboard spec requires object 'data_sources'".to_string(),
            )
        })?;
    if source_object.len() > MAX_SOURCE_DEFINITIONS_PER_DASHBOARD {
        return Err(ApiError::UnprocessableEntity(format!(
            "Dashboard spec defines {} sources, exceeding max {}",
            source_object.len(),
            MAX_SOURCE_DEFINITIONS_PER_DASHBOARD
        )));
    }

    let mut sources_in_contract_order = Vec::new();
    let mut source_ids = BTreeSet::new();
    for (source_id, source_value) in source_object {
        let source_object = source_value.as_object().ok_or_else(|| {
            ApiError::UnprocessableEntity(format!(
                "Dashboard source '{}' must be an object",
                source_id
            ))
        })?;
        let source_type = source_object
            .get("type")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ApiError::UnprocessableEntity(format!(
                    "Dashboard source '{}' requires string 'type'",
                    source_id
                ))
            })?
            .to_string();
        let source_params = source_object
            .get("params")
            .map(parse_source_params)
            .transpose()?
            .unwrap_or_default();
        validate_source_params(source_id, &source_type, &source_params, &filters)?;

        source_ids.insert(source_id.clone());
        sources_in_contract_order.push(DashboardSourceDef {
            source_id: source_id.clone(),
            source_type,
            source_params,
        });
    }
    sources_in_contract_order.sort_by(|left, right| left.source_id.cmp(&right.source_id));

    let cards = spec_object
        .get("cards")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            ApiError::UnprocessableEntity("Dashboard spec requires array 'cards'".to_string())
        })?;
    if cards.len() > MAX_CARDS_PER_DASHBOARD {
        return Err(ApiError::UnprocessableEntity(format!(
            "Dashboard spec defines {} cards, exceeding max {}",
            cards.len(),
            MAX_CARDS_PER_DASHBOARD
        )));
    }

    let mut card_to_source = HashMap::new();
    let mut sources_from_cards_in_order = Vec::new();
    let mut seen_sources = BTreeSet::new();

    for card in cards {
        let card_object = card.as_object().ok_or_else(|| {
            ApiError::UnprocessableEntity("Dashboard card entries must be objects".to_string())
        })?;
        let card_id = card_object
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ApiError::UnprocessableEntity(
                    "Dashboard card entries require string 'id'".to_string(),
                )
            })?
            .to_string();
        let source_id = card_object
            .get("source")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ApiError::UnprocessableEntity(format!(
                    "Dashboard card '{}' requires string 'source'",
                    card_id
                ))
            })?
            .to_string();

        if !source_ids.contains(&source_id) {
            return Err(ApiError::UnprocessableEntity(format!(
                "Dashboard card '{}' references unknown source '{}'",
                card_id, source_id
            )));
        }

        if seen_sources.insert(source_id.clone()) {
            sources_from_cards_in_order.push(source_id.clone());
        }
        card_to_source.insert(card_id, source_id);
    }

    Ok(DashboardSpecIndex {
        defaults_time_window,
        defaults_timezone,
        filters,
        sources_in_contract_order,
        card_to_source,
        sources_from_cards_in_order,
    })
}

fn validate_request_filters(
    request_filters: &BTreeMap<String, JsonValue>,
    declared_filters: &HashMap<String, DashboardFilterDef>,
) -> Result<(), ApiError> {
    for (filter_id, filter_value) in request_filters {
        let Some(filter_def) = declared_filters.get(filter_id) else {
            return Err(ApiError::BadRequest(format!(
                "Unknown filter id '{}'",
                filter_id
            )));
        };

        if !is_supported_filter_value(filter_value) {
            return Err(ApiError::BadRequest(format!(
                "Filter '{}' has an unsupported value type",
                filter_id
            )));
        }

        if let Some(filter_type) = filter_def.filter_type.as_deref() {
            validate_filter_value_type(filter_id, filter_type, filter_value)?;
        }

        if let Some(options) = &filter_def.options {
            validate_filter_options(filter_id, filter_value, options)?;
        }
    }

    Ok(())
}

fn is_supported_filter_value(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => true,
        JsonValue::Array(values) => values
            .iter()
            .all(|entry| matches!(entry, JsonValue::String(_) | JsonValue::Number(_))),
        JsonValue::Object(_) => false,
    }
}

fn validate_filter_value_type(
    filter_id: &str,
    filter_type: &str,
    value: &JsonValue,
) -> Result<(), ApiError> {
    let type_ok = match filter_type {
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "time_window" | "text" => value.is_string(),
        "pack_ref" | "action_ref" | "queue_ref" | "trigger_ref" | "rule_ref" => {
            is_string_or_string_array(value)
        }
        _ => true,
    };

    if !type_ok {
        return Err(ApiError::BadRequest(format!(
            "Filter '{}' expects a '{}' value",
            filter_id, filter_type
        )));
    }

    Ok(())
}

fn is_string_or_string_array(value: &JsonValue) -> bool {
    value.is_string()
        || value
            .as_array()
            .is_some_and(|items| items.iter().all(JsonValue::is_string))
}

fn validate_filter_options(
    filter_id: &str,
    filter_value: &JsonValue,
    options: &[JsonValue],
) -> Result<(), ApiError> {
    let matches_option = |candidate: &JsonValue| options.iter().any(|option| option == candidate);

    let valid = match filter_value {
        JsonValue::Array(values) => values.iter().all(matches_option),
        value => matches_option(value),
    };

    if !valid {
        return Err(ApiError::BadRequest(format!(
            "Filter '{}' includes values outside declared options",
            filter_id
        )));
    }

    Ok(())
}

fn resolve_requested_source_ids(
    request: &DashboardDataRequest,
    spec_index: &DashboardSpecIndex,
) -> Result<BTreeSet<String>, ApiError> {
    let declared_sources: BTreeSet<String> = spec_index
        .sources_in_contract_order
        .iter()
        .map(|source| source.source_id.clone())
        .collect();

    let mut requested = Vec::new();

    if let Some(source_ids) = &request.source_ids {
        for source_id in source_ids {
            if !declared_sources.contains(source_id) {
                return Err(ApiError::BadRequest(format!(
                    "Unknown source id '{}'",
                    source_id
                )));
            }
            requested.push(source_id.clone());
        }
    }

    if let Some(card_ids) = &request.card_ids {
        for card_id in card_ids {
            let source_id = spec_index
                .card_to_source
                .get(card_id)
                .ok_or_else(|| ApiError::BadRequest(format!("Unknown card id '{}'", card_id)))?;
            requested.push(source_id.clone());
        }
    }

    if request.source_ids.is_none() && request.card_ids.is_none() {
        requested.extend(spec_index.sources_from_cards_in_order.clone());
    }

    let resolved: BTreeSet<String> = requested.into_iter().collect();
    if resolved.len() > MAX_SOURCES_PER_REQUEST {
        return Err(ApiError::BadRequest(format!(
            "Request resolves {} sources, exceeding max {}",
            resolved.len(),
            MAX_SOURCES_PER_REQUEST
        )));
    }

    Ok(resolved)
}

fn resolve_source_timeout_budget(request_deadline: tokio::time::Instant) -> Option<StdDuration> {
    let now = tokio::time::Instant::now();
    if request_deadline <= now {
        return None;
    }

    let remaining = request_deadline.duration_since(now);
    Some(std::cmp::min(SOURCE_TIMEOUT, remaining))
}

fn resolve_effective_time_range(
    request: &DashboardDataRequest,
    spec_index: &DashboardSpecIndex,
) -> Result<DashboardEffectiveTimeRange, ApiError> {
    let timezone = request
        .timezone
        .clone()
        .or_else(|| spec_index.defaults_timezone.clone())
        .unwrap_or_else(|| "UTC".to_string());

    let (start, end) = if let Some(range) = &request.time_range {
        if range.start >= range.end {
            return Err(ApiError::BadRequest(
                "time_range.start must be earlier than time_range.end".to_string(),
            ));
        }
        (range.start, range.end)
    } else {
        let window = request
            .time_window
            .clone()
            .or_else(|| spec_index.defaults_time_window.clone())
            .unwrap_or_else(|| "24h".to_string());

        let duration = parse_time_window(&window)?;
        let end = Utc::now();
        (end - duration, end)
    };

    Ok(DashboardEffectiveTimeRange {
        start,
        end,
        timezone,
    })
}

fn source_window_bound(source_type: &str) -> Option<SourceWindowBound> {
    match source_type {
        "enforcement_count"
        | "enforcement_timeseries"
        | "inquiry_sla"
        | "execution_duration_stats" => Some(SourceWindowBound {
            cost_class: SourceCostClass::HighCostRaw,
            max_window_seconds: MAX_HIGH_COST_SOURCE_WINDOW_SECONDS,
            freshness_mode_hint: DashboardFreshnessMode::RawOnly,
        }),
        "queue_throughput" | "queue_dispatch_stats" => Some(SourceWindowBound {
            cost_class: SourceCostClass::HighCostRaw,
            max_window_seconds: MAX_HIGH_COST_SOURCE_WINDOW_SECONDS,
            freshness_mode_hint: DashboardFreshnessMode::RawOnly,
        }),
        "execution_count"
        | "execution_timeseries"
        | "execution_status_breakdown"
        | "event_count"
        | "event_timeseries" => Some(SourceWindowBound {
            cost_class: SourceCostClass::RawFallbackBounded,
            max_window_seconds: MAX_RAW_FALLBACK_WINDOW_SECONDS,
            freshness_mode_hint: DashboardFreshnessMode::RawOnlyFallback,
        }),
        _ => None,
    }
}

fn validate_source_window_bounds(
    source_type: &str,
    effective_time_range: &DashboardEffectiveTimeRange,
) -> Result<(), SourceWindowBoundViolation> {
    let Some(bound) = source_window_bound(source_type) else {
        return Ok(());
    };

    let requested_window_seconds = (effective_time_range.end - effective_time_range.start)
        .num_seconds()
        .max(0);

    if requested_window_seconds > bound.max_window_seconds {
        return Err(SourceWindowBoundViolation {
            cost_class: bound.cost_class,
            requested_window_seconds,
            max_window_seconds: bound.max_window_seconds,
            freshness_mode_hint: bound.freshness_mode_hint,
        });
    }

    Ok(())
}

fn source_window_bound_violation_result(
    source: &DashboardSourceDef,
    effective_time_range: &DashboardEffectiveTimeRange,
    violation: SourceWindowBoundViolation,
) -> DashboardSourceResult {
    let mut meta = default_source_meta();
    meta.freshness_mode = violation.freshness_mode_hint;
    DashboardSourceResult {
        source_id: source.source_id.clone(),
        source_type: source.source_type.clone(),
        status: DashboardSourceStatus::Invalid,
        data: None,
        meta,
        error: Some(DashboardSourceError {
            code: "window_bounds_exceeded".to_string(),
            message: format!(
                "Requested time window exceeds max for source cost class '{}' (requested={}s, max={}s)",
                violation.cost_class.as_str(),
                violation.requested_window_seconds,
                violation.max_window_seconds
            ),
            retryable: false,
            details: Some(serde_json::json!({
                "enforcement_phase": "pre_execution",
                "cost_class": violation.cost_class.as_str(),
                "requested_window_seconds": violation.requested_window_seconds,
                "max_window_seconds": violation.max_window_seconds,
                "freshness_mode_hint": violation.freshness_mode_hint,
                "effective_time_range": {
                    "start": effective_time_range.start,
                    "end": effective_time_range.end,
                }
            })),
        }),
    }
}

fn parse_time_window(window: &str) -> Result<Duration, ApiError> {
    if window.len() < 2 {
        return Err(ApiError::BadRequest(format!(
            "Invalid time_window '{}'",
            window
        )));
    }

    let unit = window
        .chars()
        .last()
        .ok_or_else(|| ApiError::BadRequest(format!("Invalid time_window '{}'", window)))?;
    let amount: i64 = window[..window.len() - 1]
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("Invalid time_window '{}'", window)))?;

    if amount <= 0 {
        return Err(ApiError::BadRequest(
            "time_window must be greater than zero".to_string(),
        ));
    }

    let duration = match unit {
        's' => Duration::seconds(amount),
        'm' => Duration::minutes(amount),
        'h' => Duration::hours(amount),
        'd' => Duration::days(amount),
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Unsupported time_window unit '{}' (supported: s,m,h,d)",
                unit
            )));
        }
    };

    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::jwt::{Claims, TokenType};
    use attune_common::rbac::{Grant, GrantConstraints, Resource};
    use serde_json::json;

    fn valid_spec() -> JsonValue {
        json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "queue_backlog": { "type": "queue_backlog" }
            },
            "cards": [
                {
                    "id": "backlog",
                    "source": "queue_backlog",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        })
    }

    fn access_user(identity_id: i64) -> AuthenticatedUser {
        AuthenticatedUser {
            claims: Claims {
                sub: identity_id.to_string(),
                login: format!("user_{identity_id}"),
                iat: 1,
                exp: 2,
                token_type: TokenType::Access,
                scope: None,
                metadata: None,
            },
        }
    }

    fn spec_with_source_count(count: usize) -> JsonValue {
        let mut spec = valid_spec();
        let source_ids = (0..count)
            .map(|index| format!("source_{index}"))
            .collect::<Vec<_>>();

        {
            let data_sources = spec
                .get_mut("data_sources")
                .and_then(JsonValue::as_object_mut)
                .expect("valid spec data_sources object");
            data_sources.clear();
            for source_id in &source_ids {
                data_sources.insert(source_id.clone(), json!({ "type": "queue_backlog" }));
            }
        }

        {
            let cards = spec
                .get_mut("cards")
                .and_then(JsonValue::as_array_mut)
                .expect("valid spec cards array");
            cards.clear();
            for (index, source_id) in source_ids.iter().enumerate() {
                cards.push(json!({
                    "id": format!("card_{index}"),
                    "source": source_id,
                    "position": {
                        "lg": { "x": 0, "y": index, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": index, "w": 4, "h": 4 }
                    }
                }));
            }
        }

        spec
    }

    fn source_ids_in_response_contract_order(
        spec_index: &DashboardSpecIndex,
        resolved_source_ids: &BTreeSet<String>,
    ) -> Vec<String> {
        spec_index
            .sources_in_contract_order
            .iter()
            .filter(|source| resolved_source_ids.contains(&source.source_id))
            .map(|source| source.source_id.clone())
            .collect()
    }

    #[test]
    fn sensor_health_runtime_auth_is_worker_scoped() {
        let registry = DashboardSourceRegistry::new();
        let entry = registry
            .get("sensor_health")
            .expect("sensor_health source should exist");
        let required_auth = entry
            .required_auth
            .expect("sensor_health source should require auth");
        assert_eq!(required_auth.resource, Resource::Workers);
        assert_eq!(required_auth.action, RbacAction::Read);
    }

    #[test]
    fn bucketed_cutover_source_types_use_matching_watermark_and_query_views() {
        let cases = [
            ("execution_count", "execution_status_hourly"),
            ("execution_timeseries", "execution_status_hourly"),
            ("execution_status_breakdown", "execution_status_hourly"),
            ("event_count", "event_volume_hourly"),
            ("event_timeseries", "event_volume_hourly"),
        ];

        for (source_type, expected_view) in cases {
            let kind = BucketedCutoverKind::for_source_type(source_type)
                .expect("bucketed source type should map to a cutover kind");
            assert_eq!(kind.aggregate_query_view_name(), expected_view);
            assert_eq!(
                kind.aggregate_watermark_view_name(),
                kind.aggregate_query_view_name(),
                "watermark and aggregate query view must stay aligned for source_type={source_type}"
            );
        }
    }

    #[test]
    fn parse_time_window_supports_standard_units() {
        assert_eq!(parse_time_window("15m").unwrap(), Duration::minutes(15));
        assert_eq!(parse_time_window("2h").unwrap(), Duration::hours(2));
        assert_eq!(parse_time_window("7d").unwrap(), Duration::days(7));
    }

    #[test]
    fn parse_time_window_rejects_invalid_input() {
        assert!(parse_time_window("0h").is_err());
        assert!(parse_time_window("abc").is_err());
        assert!(parse_time_window("10w").is_err());
    }

    fn effective_range_for_test(duration: Duration) -> DashboardEffectiveTimeRange {
        let end = Utc::now();
        DashboardEffectiveTimeRange {
            start: end - duration,
            end,
            timezone: "UTC".to_string(),
        }
    }

    #[test]
    fn validate_source_window_bounds_rejects_high_cost_raw_windows_over_limit() {
        let range = effective_range_for_test(Duration::days(8));
        let violation = validate_source_window_bounds("enforcement_timeseries", &range)
            .expect_err("high-cost source should be bounded");
        assert_eq!(violation.cost_class, SourceCostClass::HighCostRaw);
        assert_eq!(
            violation.max_window_seconds,
            MAX_HIGH_COST_SOURCE_WINDOW_SECONDS
        );
        assert!(matches!(
            violation.freshness_mode_hint,
            DashboardFreshnessMode::RawOnly
        ));
    }

    #[test]
    fn validate_source_window_bounds_rejects_raw_fallback_windows_over_limit() {
        let range = effective_range_for_test(Duration::days(8));
        let violation = validate_source_window_bounds("event_timeseries", &range)
            .expect_err("fallback-bounded source should be bounded");
        assert_eq!(violation.cost_class, SourceCostClass::RawFallbackBounded);
        assert_eq!(
            violation.max_window_seconds,
            MAX_RAW_FALLBACK_WINDOW_SECONDS
        );
        assert!(matches!(
            violation.freshness_mode_hint,
            DashboardFreshnessMode::RawOnlyFallback
        ));
    }

    #[test]
    fn validate_source_window_bounds_allows_bounded_sources_within_limit() {
        let range = effective_range_for_test(Duration::days(7));
        assert!(validate_source_window_bounds("enforcement_count", &range).is_ok());
        assert!(validate_source_window_bounds("execution_count", &range).is_ok());
    }

    #[test]
    fn source_window_bound_violation_result_is_explicit_and_deterministic() {
        let source = DashboardSourceDef {
            source_id: "exec_source".to_string(),
            source_type: "execution_count".to_string(),
            source_params: BTreeMap::new(),
        };
        let range = effective_range_for_test(Duration::days(8));
        let violation = validate_source_window_bounds(&source.source_type, &range)
            .expect_err("range should violate fallback bound");
        let result = source_window_bound_violation_result(&source, &range, violation);

        assert_eq!(result.status, DashboardSourceStatus::Invalid);
        assert!(matches!(
            result.meta.freshness_mode,
            DashboardFreshnessMode::RawOnlyFallback
        ));
        let error = result.error.expect("error should be set");
        assert_eq!(error.code, "window_bounds_exceeded");
        assert!(!error.retryable);
        assert_eq!(
            error
                .details
                .and_then(|details| details.get("enforcement_phase").cloned()),
            Some(JsonValue::String("pre_execution".to_string()))
        );
    }

    #[test]
    fn index_dashboard_spec_rejects_duplicate_card_ids() {
        let mut spec = valid_spec();
        spec["cards"] = json!([
            {
                "id": "backlog",
                "source": "queue_backlog",
                "position": {
                    "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                    "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                }
            },
            {
                "id": "backlog",
                "source": "queue_backlog",
                "position": {
                    "lg": { "x": 0, "y": 4, "w": 6, "h": 4 },
                    "sm": { "x": 0, "y": 4, "w": 4, "h": 4 }
                }
            }
        ]);

        let err = index_dashboard_spec(&spec).unwrap_err();
        match err {
            ApiError::UnprocessableEntity(message) => {
                assert!(message.contains("duplicate card id 'backlog'"))
            }
            other => panic!("expected unprocessable entity, got {other:?}"),
        }
    }

    #[test]
    fn index_dashboard_spec_rejects_missing_breakpoint_position() {
        let mut spec = valid_spec();
        spec["cards"][0]["position"] = json!({
            "lg": { "x": 0, "y": 0, "w": 6, "h": 4 }
        });

        let err = index_dashboard_spec(&spec).unwrap_err();
        match err {
            ApiError::UnprocessableEntity(message) => {
                assert!(message.contains("missing position for breakpoint 'sm'"))
            }
            other => panic!("expected unprocessable entity, got {other:?}"),
        }
    }

    #[test]
    fn index_dashboard_spec_rejects_too_many_sources() {
        let spec = spec_with_source_count(MAX_SOURCE_DEFINITIONS_PER_DASHBOARD + 1);
        let err = index_dashboard_spec(&spec).unwrap_err();
        assert!(matches!(err, ApiError::UnprocessableEntity(_)));
    }

    #[test]
    fn index_dashboard_spec_rejects_too_many_cards() {
        let spec = spec_with_source_count(MAX_CARDS_PER_DASHBOARD + 1);
        let err = index_dashboard_spec(&spec).unwrap_err();
        assert!(matches!(err, ApiError::UnprocessableEntity(_)));
    }

    #[test]
    fn index_dashboard_spec_builds_canonical_source_id_order() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "zeta_source": { "type": "queue_backlog" },
                "alpha_source": { "type": "queue_backlog" },
                "beta_source": { "type": "queue_backlog" }
            },
            "cards": [
                {
                    "id": "zeta_card",
                    "source": "zeta_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        });

        let spec_index = index_dashboard_spec(&spec).expect("index should parse");
        let source_ids = spec_index
            .sources_in_contract_order
            .iter()
            .map(|source| source.source_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            source_ids,
            vec!["alpha_source", "beta_source", "zeta_source"],
            "data source order must follow canonical source_id sort"
        );
    }

    #[test]
    fn index_dashboard_spec_preserves_and_validates_source_params_templates() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "filters": [
                { "id": "action_ref", "type": "action_ref" }
            ],
            "data_sources": {
                "execution_source": {
                    "type": "execution_count",
                    "params": {
                        "pack_refs": ["core"],
                        "action_refs": "{{ filters.action_ref }}"
                    }
                }
            },
            "cards": [
                {
                    "id": "exec_card",
                    "source": "execution_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        });

        let spec_index = index_dashboard_spec(&spec).expect("index should parse");
        let source = spec_index
            .sources_in_contract_order
            .iter()
            .find(|source| source.source_id == "execution_source")
            .expect("source should be present");
        assert_eq!(
            source.source_params.get("pack_refs"),
            Some(&json!(["core"]))
        );
        assert_eq!(
            source.source_params.get("action_refs"),
            Some(&json!("{{ filters.action_ref }}"))
        );
    }

    #[test]
    fn index_dashboard_spec_rejects_unknown_filter_reference_in_source_params() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "filters": [
                { "id": "queue_ref", "type": "queue_ref" }
            ],
            "data_sources": {
                "execution_source": {
                    "type": "execution_count",
                    "params": {
                        "action_refs": "{{ filters.action_ref }}"
                    }
                }
            },
            "cards": [
                {
                    "id": "exec_card",
                    "source": "execution_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        });

        let err = index_dashboard_spec(&spec).unwrap_err();
        assert!(
            matches!(err, ApiError::UnprocessableEntity(message) if message.contains("unknown filter 'action_ref'"))
        );
    }

    #[test]
    fn index_dashboard_spec_enforces_required_source_params_from_contract() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "key_source": {
                    "type": "key_value",
                    "params": {}
                }
            },
            "cards": [
                {
                    "id": "key_card",
                    "source": "key_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        });

        let err = index_dashboard_spec(&spec).unwrap_err();
        assert!(matches!(
            err,
            ApiError::UnprocessableEntity(message)
                if message.contains("missing required param 'ref'")
        ));
    }

    #[test]
    fn index_dashboard_spec_rejects_source_params_not_declared_by_source_contract() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "execution_source": {
                    "type": "execution_count",
                    "params": {
                        "sensor_ref": "core.timer"
                    }
                }
            },
            "cards": [
                {
                    "id": "exec_card",
                    "source": "execution_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        });

        let err = index_dashboard_spec(&spec).unwrap_err();
        assert!(matches!(
            err,
            ApiError::UnprocessableEntity(message)
                if message.contains("unsupported param key 'sensor_ref'")
        ));
    }

    #[test]
    fn index_dashboard_spec_validates_worker_status_params_against_worker_health_contract() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "worker_source": {
                    "type": "worker_status",
                    "params": {
                        "sensor_ref": "core.timer"
                    }
                }
            },
            "cards": [
                {
                    "id": "worker_card",
                    "source": "worker_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        });

        let err = index_dashboard_spec(&spec).unwrap_err();
        assert!(matches!(
            err,
            ApiError::UnprocessableEntity(message)
                if message.contains("unsupported param key 'sensor_ref'")
                    && message.contains("contract 'worker_health'")
        ));
    }

    #[test]
    fn normalize_request_ref_scope_rejects_non_string_ref_filters() {
        let mut filters = BTreeMap::new();
        filters.insert("action_ref".to_string(), json!(42));
        let err = normalize_request_ref_scope(&filters).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn normalize_request_ref_scope_accepts_scalar_and_array_ref_filters() {
        let mut filters = BTreeMap::new();
        filters.insert("pack_ref".to_string(), json!(["zeta", "alpha", "zeta"]));
        filters.insert(
            "action_ref".to_string(),
            json!(["core.http_request", "core.echo"]),
        );
        filters.insert("rule_ref".to_string(), json!("core.rule_a"));

        let scope = normalize_request_ref_scope(&filters).expect("scope should normalize");

        assert_eq!(
            scope.pack_refs,
            Some(BTreeSet::from(["alpha".to_string(), "zeta".to_string()]))
        );
        assert_eq!(
            scope.action_refs,
            Some(BTreeSet::from([
                "core.echo".to_string(),
                "core.http_request".to_string(),
            ]))
        );
        assert_eq!(
            scope.rule_refs,
            Some(BTreeSet::from(["core.rule_a".to_string()]))
        );
    }

    #[test]
    fn normalize_request_ref_scope_rejects_non_string_array_ref_filters() {
        let mut filters = BTreeMap::new();
        filters.insert("queue_ref".to_string(), json!(["core.ingest", 42]));
        let err = normalize_request_ref_scope(&filters).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn normalize_request_ref_scope_rejects_malformed_ref_in_array_filters() {
        let mut filters = BTreeMap::new();
        filters.insert("trigger_ref".to_string(), json!(["core.timer", "bad ref"]));
        let err = normalize_request_ref_scope(&filters).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn validate_filter_value_type_accepts_string_arrays_for_ref_filter_types() {
        assert!(
            validate_filter_value_type("pack_ref", "pack_ref", &json!(["core", "infra"])).is_ok()
        );
        assert!(validate_filter_value_type(
            "action_ref",
            "action_ref",
            &json!(["core.echo", "core.http_request"])
        )
        .is_ok());
        assert!(validate_filter_value_type(
            "rule_ref",
            "rule_ref",
            &json!(["core.rule_a", "core.rule_b"])
        )
        .is_ok());
        assert!(validate_filter_value_type(
            "queue_ref",
            "queue_ref",
            &json!(["core.ingest", "core.bulk"])
        )
        .is_ok());
        assert!(validate_filter_value_type(
            "trigger_ref",
            "trigger_ref",
            &json!(["core.timer", "core.alert"])
        )
        .is_ok());
    }

    #[test]
    fn resolve_source_query_scope_intersects_request_with_auth_constraints() {
        let source = DashboardSourceDef {
            source_id: "execs".to_string(),
            source_type: "execution_count".to_string(),
            source_params: BTreeMap::new(),
        };
        let request_scope = RefFilterScope {
            pack_refs: Some(BTreeSet::from(["core".to_string()])),
            action_refs: Some(BTreeSet::from([
                "core.echo".to_string(),
                "other.pack_action".to_string(),
            ])),
            ..Default::default()
        };
        let grants = vec![Grant {
            resource: Resource::Executions,
            actions: vec![RbacAction::Read],
            constraints: Some(GrantConstraints {
                pack_refs: Some(vec!["core".to_string(), "infra".to_string()]),
                refs: Some(vec![
                    "core.echo".to_string(),
                    "core.http_request".to_string(),
                ]),
                ..Default::default()
            }),
        }];

        let scope = resolve_source_query_scope(
            &source,
            &request_scope,
            &RefFilterScope::default(),
            &grants,
            Some(SourceAuthRequirement {
                resource: Resource::Executions,
                action: RbacAction::Read,
            }),
        )
        .expect("scope should resolve");

        assert!(matches!(
            scope.authorization_mode,
            DashboardAuthorizationMode::IdentityFiltered
        ));
        assert_eq!(scope.pack_refs, Some(BTreeSet::from(["core".to_string()])));
        assert_eq!(
            scope.primary_refs,
            Some(BTreeSet::from(["core.echo".to_string()]))
        );
        assert_eq!(
            scope.authorized_refs_json(),
            Some(json!({
                "pack_refs": ["core"],
                "action_refs": ["core.echo"]
            }))
        );
    }

    #[test]
    fn resolve_source_query_scope_uses_source_params_request_and_auth_intersection() {
        let source = DashboardSourceDef {
            source_id: "execs".to_string(),
            source_type: "execution_count".to_string(),
            source_params: BTreeMap::from([
                ("pack_refs".to_string(), json!(["core", "infra"])),
                (
                    "action_refs".to_string(),
                    json!(["core.echo", "core.http_request"]),
                ),
            ]),
        };
        let request_scope = RefFilterScope {
            pack_refs: Some(BTreeSet::from(["core".to_string(), "other".to_string()])),
            action_refs: Some(BTreeSet::from([
                "core.echo".to_string(),
                "core.blocked".to_string(),
            ])),
            ..Default::default()
        };
        let source_param_scope = resolve_source_param_scope(&source, &BTreeMap::new())
            .expect("source params should resolve");
        let grants = vec![Grant {
            resource: Resource::Executions,
            actions: vec![RbacAction::Read],
            constraints: Some(GrantConstraints {
                pack_refs: Some(vec!["core".to_string(), "zeta".to_string()]),
                refs: Some(vec!["core.echo".to_string(), "core.extra".to_string()]),
                ..Default::default()
            }),
        }];

        let scope = resolve_source_query_scope(
            &source,
            &request_scope,
            &source_param_scope,
            &grants,
            Some(SourceAuthRequirement {
                resource: Resource::Executions,
                action: RbacAction::Read,
            }),
        )
        .expect("scope should resolve");

        assert_eq!(scope.pack_refs, Some(BTreeSet::from(["core".to_string()])));
        assert_eq!(
            scope.primary_refs,
            Some(BTreeSet::from(["core.echo".to_string()]))
        );
        assert_eq!(
            scope.authorized_refs_json(),
            Some(json!({
                "pack_refs": ["core"],
                "action_refs": ["core.echo"]
            }))
        );
    }

    #[test]
    fn source_scope_authorized_refs_json_is_deterministic() {
        let scope = SourceQueryScope {
            authorization_mode: DashboardAuthorizationMode::IdentityFiltered,
            pack_refs: Some(BTreeSet::from(["zeta".to_string(), "alpha".to_string()])),
            primary_ref_kind: Some(SourcePrimaryRefKind::Rule),
            primary_refs: Some(BTreeSet::from([
                "core.rule_b".to_string(),
                "core.rule_a".to_string(),
            ])),
        };

        assert_eq!(
            scope.authorized_refs_json(),
            Some(json!({
                "pack_refs": ["alpha", "zeta"],
                "rule_refs": ["core.rule_a", "core.rule_b"]
            }))
        );
    }

    #[test]
    fn resolve_requested_source_ids_rejects_too_many_resolved_sources() {
        let spec = spec_with_source_count(MAX_SOURCES_PER_REQUEST + 1);
        let spec_index = index_dashboard_spec(&spec).expect("index should parse");
        let request = DashboardDataRequest {
            filters: BTreeMap::new(),
            time_window: None,
            time_range: None,
            timezone: None,
            source_ids: Some(
                (0..=MAX_SOURCES_PER_REQUEST)
                    .map(|index| format!("source_{index}"))
                    .collect(),
            ),
            card_ids: None,
            include_meta: true,
            request_id: None,
        };

        let err = resolve_requested_source_ids(&request, &spec_index).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn resolve_requested_source_ids_and_response_order_are_deterministic() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "zeta_source": { "type": "queue_backlog" },
                "alpha_source": { "type": "queue_backlog" },
                "beta_source": { "type": "queue_backlog" }
            },
            "cards": [
                {
                    "id": "card_z",
                    "source": "zeta_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                },
                {
                    "id": "card_a",
                    "source": "alpha_source",
                    "position": {
                        "lg": { "x": 0, "y": 4, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 4, "w": 4, "h": 4 }
                    }
                }
            ]
        });
        let spec_index = index_dashboard_spec(&spec).expect("index should parse");
        let request = DashboardDataRequest {
            filters: BTreeMap::new(),
            time_window: None,
            time_range: None,
            timezone: None,
            source_ids: Some(vec!["zeta_source".to_string(), "alpha_source".to_string()]),
            card_ids: Some(vec!["card_z".to_string(), "card_a".to_string()]),
            include_meta: true,
            request_id: None,
        };

        let resolved = resolve_requested_source_ids(&request, &spec_index).expect("request valid");
        assert_eq!(
            resolved.into_iter().collect::<Vec<_>>(),
            vec!["alpha_source", "zeta_source"],
            "resolved source ids must be canonicalized regardless of request order"
        );

        let resolved = resolve_requested_source_ids(&request, &spec_index).expect("request valid");
        let source_order = source_ids_in_response_contract_order(&spec_index, &resolved);
        assert_eq!(
            source_order,
            vec!["alpha_source".to_string(), "zeta_source".to_string()],
            "response sources must follow canonical source_id order"
        );
    }

    #[test]
    fn resolve_requested_source_ids_is_order_insensitive_for_card_ids() {
        let spec = json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "data_sources": {
                "zeta_source": { "type": "queue_backlog" },
                "alpha_source": { "type": "queue_backlog" },
                "beta_source": { "type": "queue_backlog" }
            },
            "cards": [
                {
                    "id": "card_z",
                    "source": "zeta_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                },
                {
                    "id": "card_a",
                    "source": "alpha_source",
                    "position": {
                        "lg": { "x": 0, "y": 4, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 4, "w": 4, "h": 4 }
                    }
                }
            ]
        });
        let spec_index = index_dashboard_spec(&spec).expect("index should parse");

        let request_one = DashboardDataRequest {
            filters: BTreeMap::new(),
            time_window: None,
            time_range: None,
            timezone: None,
            source_ids: None,
            card_ids: Some(vec!["card_z".to_string(), "card_a".to_string()]),
            include_meta: true,
            request_id: None,
        };
        let request_two = DashboardDataRequest {
            filters: BTreeMap::new(),
            time_window: None,
            time_range: None,
            timezone: None,
            source_ids: None,
            card_ids: Some(vec![
                "card_a".to_string(),
                "card_z".to_string(),
                "card_a".to_string(),
            ]),
            include_meta: true,
            request_id: None,
        };

        let first =
            resolve_requested_source_ids(&request_one, &spec_index).expect("first request valid");
        let second =
            resolve_requested_source_ids(&request_two, &spec_index).expect("second request valid");

        assert_eq!(
            first, second,
            "card selector ordering and duplicates must not change resolved source set"
        );
        let source_order = source_ids_in_response_contract_order(&spec_index, &first);
        assert_eq!(
            source_order,
            vec!["alpha_source".to_string(), "zeta_source".to_string()],
            "response order remains canonical source_id order"
        );
    }

    #[test]
    fn resolve_source_timeout_budget_honors_request_deadline() {
        let long_deadline = tokio::time::Instant::now() + StdDuration::from_secs(30);
        assert_eq!(
            resolve_source_timeout_budget(long_deadline),
            Some(SOURCE_TIMEOUT)
        );

        let short_deadline = tokio::time::Instant::now() + StdDuration::from_millis(200);
        let short_budget =
            resolve_source_timeout_budget(short_deadline).expect("budget should be present");
        assert!(short_budget <= StdDuration::from_millis(200));
        assert!(short_budget < SOURCE_TIMEOUT);

        let expired_deadline = tokio::time::Instant::now() - StdDuration::from_millis(1);
        assert!(resolve_source_timeout_budget(expired_deadline).is_none());
    }

    #[tokio::test]
    async fn inflight_waiters_do_not_hang_when_completion_happens_before_wait() {
        let cache = DashboardSourceCache::new();
        let key = "dashboard-cache-race";
        assert!(matches!(
            cache.register_inflight(key).await,
            InflightRegistration::Leader
        ));

        let waiter = match cache.register_inflight(key).await {
            InflightRegistration::Waiter(waiter) => waiter,
            InflightRegistration::Leader => panic!("expected waiter registration"),
        };

        cache.complete_inflight(key).await;
        timeout(
            StdDuration::from_millis(300),
            cache.wait_for_inflight(
                key,
                waiter,
                tokio::time::Instant::now() + StdDuration::from_secs(1),
            ),
        )
        .await
        .expect("waiter should not hang on lost notify race");
    }

    #[tokio::test]
    async fn inflight_waiters_recheck_with_bounded_wait_budget() {
        let cache = DashboardSourceCache::new();
        let key = "dashboard-cache-bounded-wait";
        assert!(matches!(
            cache.register_inflight(key).await,
            InflightRegistration::Leader
        ));
        let waiter = match cache.register_inflight(key).await {
            InflightRegistration::Waiter(waiter) => waiter,
            InflightRegistration::Leader => panic!("expected waiter registration"),
        };

        timeout(
            SOURCE_INFLIGHT_WAIT_CAP + StdDuration::from_millis(300),
            cache.wait_for_inflight(
                key,
                waiter,
                tokio::time::Instant::now() + StdDuration::from_secs(60),
            ),
        )
        .await
        .expect("waiter should stop waiting after bounded budget");
    }

    #[tokio::test]
    async fn retryable_failures_get_short_coalescing_ttl() {
        let cache = DashboardSourceCache::new();
        let key = "dashboard-cache-failure-coalesce".to_string();
        let result = DashboardSourceResult {
            source_id: "source".to_string(),
            source_type: "queue_backlog".to_string(),
            status: DashboardSourceStatus::Error,
            data: None,
            meta: default_source_meta(),
            error: Some(DashboardSourceError {
                code: "timeout".to_string(),
                message: "timed out".to_string(),
                retryable: true,
                details: None,
            }),
        };

        cache
            .insert_retryable_failure(key.clone(), result.clone())
            .await;

        let fresh = cache
            .get_fresh(&key)
            .await
            .expect("retryable failure should be coalesced briefly");
        assert_eq!(fresh.status, DashboardSourceStatus::Error);

        let entries = cache.entries.lock().await;
        let entry = entries.get(&key).expect("entry should exist");
        assert!(entry.expires_at <= entry.created_at + SOURCE_CACHE_FAILURE_COALESCE_TTL);
        assert!(entry.stale_at <= entry.created_at + SOURCE_CACHE_FAILURE_COALESCE_TTL);
    }

    #[test]
    fn execution_sources_use_terminal_outcome_semantics() {
        assert_eq!(
            TERMINAL_EXECUTION_STATUSES,
            ["completed", "failed", "timeout", "cancelled", "abandoned"]
        );
    }

    #[test]
    fn queue_backlog_row_contract_shape_is_stable() {
        let row = QueueBacklogSourceRow {
            queue_ref: "core.ingest".to_string(),
            queued: 12,
            retry: 2,
            leased: 1,
            total_backlog: 15,
        };
        assert_eq!(
            serialized_row(row),
            json!({
                "queue_ref": "core.ingest",
                "queued": 12,
                "retry": 2,
                "leased": 1,
                "total_backlog": 15
            })
        );
    }

    #[test]
    fn queue_dashboard_row_contract_shapes_are_stable() {
        let bucket_start = DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let throughput_row = QueueThroughputSourceRow {
            bucket_start,
            queue_ref: "core.ingest".to_string(),
            completed: 3,
            failed: 1,
            skipped: 2,
            cancelled: 0,
            total_processed: 6,
        };
        assert_eq!(
            serialized_row(throughput_row),
            json!({
                "bucket_start": "2026-06-25T12:00:00Z",
                "queue_ref": "core.ingest",
                "completed": 3,
                "failed": 1,
                "skipped": 2,
                "cancelled": 0,
                "total_processed": 6
            })
        );

        let dispatch_row = QueueDispatchStatsSourceRow {
            bucket_start,
            queue_ref: "core.ingest".to_string(),
            status: "timeout".to_string(),
            dispatch_count: 2,
            leased_item_count: 5,
            avg_duration_seconds: 42.5,
            max_duration_seconds: 60.0,
        };
        assert_eq!(
            serialized_row(dispatch_row),
            json!({
                "bucket_start": "2026-06-25T12:00:00Z",
                "queue_ref": "core.ingest",
                "status": "timeout",
                "dispatch_count": 2,
                "leased_item_count": 5,
                "avg_duration_seconds": 42.5,
                "max_duration_seconds": 60.0
            })
        );
    }

    #[test]
    fn worker_health_row_contract_shape_is_stable() {
        let row = WorkerHealthSourceRow {
            worker_id: 42,
            worker_name: "worker-action-01".to_string(),
            worker_role: "action".to_string(),
            status: "active".to_string(),
            cordoned: false,
        };

        assert_eq!(
            serialized_row(row),
            json!({
                "worker_id": 42,
                "worker_name": "worker-action-01",
                "worker_role": "action",
                "status": "active",
                "cordoned": false
            })
        );
    }

    #[test]
    fn execution_source_row_contract_shapes_are_stable() {
        let bucket_start = DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let count_row = BucketCountSourceRow {
            bucket_start,
            series: "core.echo".to_string(),
            count: 9,
        };
        assert_eq!(
            serialized_row(count_row),
            json!({
                "bucket_start": "2026-06-25T12:00:00Z",
                "series": "core.echo",
                "count": 9
            })
        );

        let status_row = ExecutionStatusSourceRow {
            bucket_start,
            status: "failed".to_string(),
            count: 3,
        };
        assert_eq!(
            serialized_row(status_row),
            json!({
                "bucket_start": "2026-06-25T12:00:00Z",
                "status": "failed",
                "count": 3
            })
        );

        let latest_result_row = LatestActionResultSourceRow {
            action_ref: "core.echo".to_string(),
            execution_id: 42,
            status: "completed".to_string(),
            updated_at: bucket_start,
            result: Some(json!({"data": {"message": "ok"}})),
        };
        assert_eq!(
            serialized_row(latest_result_row),
            json!({
                "action_ref": "core.echo",
                "execution_id": 42,
                "status": "completed",
                "updated_at": "2026-06-25T12:00:00Z",
                "result": {"data": {"message": "ok"}}
            })
        );

        let result_path_row = ActionResultPathSourceRow {
            action_ref: "core.echo".to_string(),
            execution_id: 43,
            status: "completed".to_string(),
            updated_at: bucket_start,
            path: "data.message".to_string(),
            value: json!("ok"),
        };
        assert_eq!(
            serialized_row(result_path_row),
            json!({
                "action_ref": "core.echo",
                "execution_id": 43,
                "status": "completed",
                "updated_at": "2026-06-25T12:00:00Z",
                "path": "data.message",
                "value": "ok"
            })
        );

        let last_execution_row = LastExecutionSourceRow {
            action_ref: "core.echo".to_string(),
            execution_id: 44,
            status: "running".to_string(),
            created_at: bucket_start,
            started_at: Some(bucket_start),
            updated_at: bucket_start,
            trace_tag: Some("trace-1".to_string()),
            result: Some(json!({"data": {"progress": 50}})),
        };
        assert_eq!(
            serialized_row(last_execution_row),
            json!({
                "action_ref": "core.echo",
                "execution_id": 44,
                "status": "running",
                "created_at": "2026-06-25T12:00:00Z",
                "started_at": "2026-06-25T12:00:00Z",
                "updated_at": "2026-06-25T12:00:00Z",
                "trace_tag": "trace-1",
                "result": {"data": {"progress": 50}}
            })
        );
    }

    #[test]
    fn scalar_result_paths_include_message_and_value_aliases() {
        let mut paths = BTreeSet::new();
        collect_json_paths(&json!("hello"), None, &mut paths);
        assert!(paths.contains("message"));
        assert!(paths.contains("value"));

        let mut numeric_paths = BTreeSet::new();
        collect_json_paths(&json!(123), None, &mut numeric_paths);
        assert!(!numeric_paths.contains("message"));
        assert!(numeric_paths.contains("value"));
    }

    #[test]
    fn extract_json_path_supports_scalar_aliases() {
        let text = json!("hello");
        assert_eq!(extract_json_path(&text, "message"), Some(&json!("hello")));
        assert_eq!(extract_json_path(&text, "value"), Some(&json!("hello")));
        assert_eq!(extract_json_path(&text, "data.message"), None);

        let number = json!(123);
        assert_eq!(extract_json_path(&number, "value"), Some(&json!(123)));
        assert_eq!(extract_json_path(&number, "message"), None);
    }

    #[test]
    fn action_result_path_not_allowed_message_includes_paths_and_guidance() {
        let allowed_paths = BTreeSet::from([
            "stdout".to_string(),
            "value".to_string(),
            "status".to_string(),
        ]);
        let message = build_action_result_path_not_allowed_message(
            "core_echo_last_run",
            "data.message",
            &allowed_paths,
        );

        assert!(message.contains("core_echo_last_run"));
        assert!(message.contains("data.message"));
        assert!(message.contains("stdout"));
        assert!(message.contains("Choose one of the allowed paths"));
    }

    #[test]
    fn action_result_default_paths_include_execution_output_fields() {
        let mut paths = BTreeSet::new();
        seed_default_action_result_paths(&mut paths);

        assert!(paths.contains("stdout"));
        assert!(paths.contains("data"));
        assert!(paths.contains("error"));
        assert!(paths.contains("exit_code"));
    }

    #[test]
    fn requested_nested_data_path_is_allowed_even_without_recent_data_shape() {
        let mut paths = BTreeSet::new();
        seed_default_action_result_paths(&mut paths);
        include_requested_derived_action_result_paths("data.stdout", &mut paths);

        assert!(paths.contains("data"));
        assert!(paths.contains("data.stdout"));
        assert!(ActionResultPathAllowList::new(paths.iter().cloned())
            .require_allowed("data.stdout")
            .is_ok());
    }

    #[test]
    fn key_value_source_contract_shape_is_stable() {
        let updated_at = DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let payload = KeyValueSourceData {
            r#ref: "core.api_token".to_string(),
            name: "API Token".to_string(),
            owner_type: "pack".to_string(),
            owner_ref: Some("core".to_string()),
            encrypted: true,
            decrypted: false,
            value: JsonValue::Null,
            updated_at,
        };
        assert_eq!(
            serialized_row(payload),
            json!({
                "ref": "core.api_token",
                "name": "API Token",
                "owner_type": "pack",
                "owner_ref": "core",
                "encrypted": true,
                "decrypted": false,
                "value": null,
                "updated_at": "2026-06-25T12:00:00Z"
            })
        );
    }

    #[test]
    fn execution_duration_stats_row_contract_shape_is_stable() {
        let bucket_start = DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let row = ExecutionDurationStatsSourceRow {
            bucket_start,
            series: "core.echo".to_string(),
            execution_count: 4,
            avg_duration_seconds: 2.5,
            p50_duration_seconds: 2.0,
            p95_duration_seconds: 4.8,
            max_duration_seconds: 5.0,
        };
        assert_eq!(
            serialized_row(row),
            json!({
                "bucket_start": "2026-06-25T12:00:00Z",
                "series": "core.echo",
                "execution_count": 4,
                "avg_duration_seconds": 2.5,
                "p50_duration_seconds": 2.0,
                "p95_duration_seconds": 4.8,
                "max_duration_seconds": 5.0
            })
        );
    }

    #[test]
    fn inquiry_dashboard_row_contract_shapes_are_stable() {
        let bucket_start = DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let backlog_row = InquiryBacklogSourceRow {
            pack_ref: Some("core".to_string()),
            assigned_to: Some(42),
            pending_count: 3,
            overdue_count: 1,
        };
        assert_eq!(
            serialized_row(backlog_row),
            json!({
                "pack_ref": "core",
                "assigned_to": 42,
                "pending_count": 3,
                "overdue_count": 1
            })
        );

        let sla_row = InquirySlaSourceRow {
            bucket_start,
            pack_ref: Some("core".to_string()),
            assigned_to: Some(42),
            sla_target_seconds: 3600,
            total_inquiries: 5,
            within_sla_count: 4,
            breached_count: 1,
            open_count: 1,
            compliance_rate: 0.8,
        };
        assert_eq!(
            serialized_row(sla_row),
            json!({
                "bucket_start": "2026-06-25T12:00:00Z",
                "pack_ref": "core",
                "assigned_to": 42,
                "sla_target_seconds": 3600,
                "total_inquiries": 5,
                "within_sla_count": 4,
                "breached_count": 1,
                "open_count": 1,
                "compliance_rate": 0.8
            })
        );
    }

    #[test]
    fn normalize_create_dashboard_request_canonicalizes_metadata_into_spec() {
        let request = CreateDashboardRequest {
            r#ref: "core.authoring".to_string(),
            label: "Authoring".to_string(),
            description: Some("Dashboard".to_string()),
            scope_type: DashboardScopeType::Pack,
            scope_ref: None,
            visibility: DashboardVisibility::Pack,
            enabled: Some(true),
            is_default_home: Some(false),
            spec_version: Some(2),
            spec: valid_spec(),
            tags: vec![
                " ops ".to_string(),
                "dashboard".to_string(),
                "ops".to_string(),
            ],
        };

        let normalized =
            normalize_create_dashboard_request(&access_user(42), request).expect("request valid");

        assert_eq!(normalized.scope_ref, "core");
        assert_eq!(
            normalized.tags,
            vec!["dashboard".to_string(), "ops".to_string()]
        );
        assert_eq!(normalized.spec["ref"], "core.authoring");
        assert_eq!(normalized.spec["label"], "Authoring");
        assert_eq!(normalized.spec["scope_type"], "pack");
        assert_eq!(normalized.spec["scope_ref"], "core");
        assert_eq!(normalized.spec["visibility"], "pack");
        assert_eq!(normalized.spec["spec_version"], 2);
        assert_eq!(normalized.spec["tags"], json!(["dashboard", "ops"]));
    }

    #[test]
    fn normalize_update_dashboard_request_defaults_identity_scope_and_private_visibility() {
        let dashboard = Dashboard {
            id: 7,
            r#ref: "core.authoring".to_string(),
            scope_type: DashboardScopeType::Global,
            scope_ref: "global".to_string(),
            pack: None,
            owner_identity: None,
            visibility: DashboardVisibility::Public,
            is_adhoc: true,
            label: "Authoring".to_string(),
            description: None,
            enabled: true,
            is_default_home: false,
            revision: 3,
            spec_version: 1,
            spec: valid_spec(),
            tags: vec!["dashboard".to_string()],
            created: Utc::now(),
            updated: Utc::now(),
        };

        let request = UpdateDashboardRequest {
            label: None,
            description: None,
            scope_type: Some(DashboardScopeType::Identity),
            scope_ref: None,
            visibility: None,
            enabled: None,
            is_default_home: None,
            spec_version: None,
            spec: None,
            tags: None,
            expected_revision: dashboard.revision,
        };

        let normalized = normalize_update_dashboard_request(&access_user(42), &dashboard, request)
            .expect("request valid");

        assert_eq!(normalized.scope_type, DashboardScopeType::Identity);
        assert_eq!(normalized.scope_ref, "42");
        assert_eq!(normalized.visibility, DashboardVisibility::Private);
        assert_eq!(normalized.owner_identity, Some(42));
        assert_eq!(normalized.spec["scope_type"], "identity");
        assert_eq!(normalized.spec["visibility"], "private");
    }

    #[test]
    fn source_cache_key_changes_when_unsaved_spec_changes() {
        let user = access_user(42);
        let range = DashboardEffectiveTimeRange {
            start: Utc::now(),
            end: Utc::now(),
            timezone: "UTC".to_string(),
        };
        let source = DashboardSourceDef {
            source_id: "queue_source".to_string(),
            source_type: "queue_backlog".to_string(),
            source_params: BTreeMap::new(),
        };
        let dashboard_a = Dashboard {
            id: 1,
            r#ref: "core.authoring".to_string(),
            scope_type: DashboardScopeType::Global,
            scope_ref: "global".to_string(),
            pack: None,
            owner_identity: None,
            visibility: DashboardVisibility::Public,
            is_adhoc: true,
            label: "A".to_string(),
            description: None,
            enabled: true,
            is_default_home: false,
            revision: 0,
            spec_version: 1,
            spec: valid_spec(),
            tags: vec![],
            created: Utc::now(),
            updated: Utc::now(),
        };
        let mut dashboard_b = dashboard_a.clone();
        dashboard_b.spec["cards"][0]["id"] = json!("different_card");

        let key_a = build_source_cache_key(&dashboard_a, &user, &source, &BTreeMap::new(), &range);
        let key_b = build_source_cache_key(&dashboard_b, &user, &source, &BTreeMap::new(), &range);

        assert_ne!(key_a, key_b, "preview cache keys must include spec content");
    }
}
