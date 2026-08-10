//! Analytics API routes
//!
//! Provides read-only access to TimescaleDB continuous aggregates for dashboard
//! widgets and time-series analytics. All data is pre-computed by TimescaleDB
//! continuous aggregate policies — these endpoints simply query the materialized views.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::sync::Arc;

use attune_common::rbac::{Action, AuthorizationContext, Resource};
use attune_common::repositories::analytics::AnalyticsRepository;

use crate::{
    auth::middleware::RequireAuth,
    authz::AuthorizationCheck,
    dto::{
        analytics::{
            AnalyticsQueryParams, DashboardAnalyticsResponse, EnforcementVolumeResponse,
            EventVolumeResponse, ExecutionStatusTimeSeriesResponse, ExecutionThroughputResponse,
            FailureRateResponse, TimeSeriesPoint, WorkerStatusTimeSeriesResponse,
        },
        common::ApiResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

async fn authorize_analytics_resource(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    resource: Resource,
) -> ApiResult<()> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    state
        .authorization_service()
        .authorize(
            user,
            AuthorizationCheck {
                resource,
                action: Action::Read,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await
}

/// Get a combined dashboard analytics payload.
///
/// Returns all key metrics in a single response to avoid multiple round-trips
/// from the dashboard page. Includes execution throughput, status transitions,
/// event volume, enforcement volume, worker status, and failure rate.
#[utoipa::path(
    get,
    path = "/api/v1/analytics/dashboard",
    tag = "analytics",
    params(AnalyticsQueryParams),
    responses(
        (status = 200, description = "Dashboard analytics", body = inline(ApiResponse<DashboardAnalyticsResponse>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_dashboard_analytics(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<AnalyticsQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_analytics_resource(&state, &user, Resource::Executions).await?;
    authorize_analytics_resource(&state, &user, Resource::Events).await?;
    authorize_analytics_resource(&state, &user, Resource::Enforcements).await?;
    authorize_analytics_resource(&state, &user, Resource::Workers).await?;

    let range = query.to_time_range();

    // Run all aggregate queries concurrently
    let (throughput, status, events, enforcements, workers, failure_rate) = tokio::try_join!(
        AnalyticsRepository::execution_throughput_hourly(&state.db, &range),
        AnalyticsRepository::execution_status_hourly(&state.db, &range),
        AnalyticsRepository::event_volume_hourly(&state.db, &range),
        AnalyticsRepository::enforcement_volume_hourly(&state.db, &range),
        AnalyticsRepository::worker_status_hourly(&state.db, &range),
        AnalyticsRepository::execution_failure_rate(&state.db, &range),
    )?;

    let response = DashboardAnalyticsResponse {
        since: range.since,
        until: range.until,
        execution_throughput: throughput.into_iter().map(Into::into).collect(),
        execution_status: status.into_iter().map(Into::into).collect(),
        event_volume: events.into_iter().map(Into::into).collect(),
        enforcement_volume: enforcements.into_iter().map(Into::into).collect(),
        worker_status: workers.into_iter().map(Into::into).collect(),
        failure_rate: FailureRateResponse::from_summary(failure_rate, &range),
    };

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

/// Get execution status transitions over time.
///
/// Returns hourly buckets of execution status transitions (e.g., how many
/// executions moved to "completed", "failed", "running" per hour).
#[utoipa::path(
    get,
    path = "/api/v1/analytics/executions/status",
    tag = "analytics",
    params(AnalyticsQueryParams),
    responses(
        (status = 200, description = "Execution status transitions", body = inline(ApiResponse<ExecutionStatusTimeSeriesResponse>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_execution_status_analytics(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<AnalyticsQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_analytics_resource(&state, &user, Resource::Executions).await?;
    let range = query.to_time_range();
    let rows = AnalyticsRepository::execution_status_hourly(&state.db, &range).await?;

    let data: Vec<TimeSeriesPoint> = rows.into_iter().map(Into::into).collect();

    let response = ExecutionStatusTimeSeriesResponse {
        since: range.since,
        until: range.until,
        data,
    };

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

/// Get execution throughput over time.
///
/// Returns hourly buckets of execution creation counts.
#[utoipa::path(
    get,
    path = "/api/v1/analytics/executions/throughput",
    tag = "analytics",
    params(AnalyticsQueryParams),
    responses(
        (status = 200, description = "Execution throughput", body = inline(ApiResponse<ExecutionThroughputResponse>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_execution_throughput_analytics(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<AnalyticsQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_analytics_resource(&state, &user, Resource::Executions).await?;
    let range = query.to_time_range();
    let rows = AnalyticsRepository::execution_throughput_hourly(&state.db, &range).await?;

    let data: Vec<TimeSeriesPoint> = rows.into_iter().map(Into::into).collect();

    let response = ExecutionThroughputResponse {
        since: range.since,
        until: range.until,
        data,
    };

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

/// Get the execution failure rate summary.
///
/// Returns aggregate failure/timeout/completion counts and the failure rate
/// percentage over the requested time range.
#[utoipa::path(
    get,
    path = "/api/v1/analytics/executions/failure-rate",
    tag = "analytics",
    params(AnalyticsQueryParams),
    responses(
        (status = 200, description = "Failure rate summary", body = inline(ApiResponse<FailureRateResponse>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_failure_rate_analytics(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<AnalyticsQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_analytics_resource(&state, &user, Resource::Executions).await?;
    let range = query.to_time_range();
    let summary = AnalyticsRepository::execution_failure_rate(&state.db, &range).await?;

    let response = FailureRateResponse::from_summary(summary, &range);

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

/// Get event volume over time.
///
/// Returns hourly buckets of event creation counts, aggregated across all triggers.
#[utoipa::path(
    get,
    path = "/api/v1/analytics/events/volume",
    tag = "analytics",
    params(AnalyticsQueryParams),
    responses(
        (status = 200, description = "Event volume", body = inline(ApiResponse<EventVolumeResponse>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_event_volume_analytics(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<AnalyticsQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_analytics_resource(&state, &user, Resource::Events).await?;
    let range = query.to_time_range();
    let rows = AnalyticsRepository::event_volume_hourly(&state.db, &range).await?;

    let data: Vec<TimeSeriesPoint> = rows.into_iter().map(Into::into).collect();

    let response = EventVolumeResponse {
        since: range.since,
        until: range.until,
        data,
    };

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

/// Get worker status transitions over time.
///
/// Returns hourly buckets of worker status changes (online/offline/draining).
#[utoipa::path(
    get,
    path = "/api/v1/analytics/workers/status",
    tag = "analytics",
    params(AnalyticsQueryParams),
    responses(
        (status = 200, description = "Worker status transitions", body = inline(ApiResponse<WorkerStatusTimeSeriesResponse>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_worker_status_analytics(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<AnalyticsQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_analytics_resource(&state, &user, Resource::Workers).await?;
    let range = query.to_time_range();
    let rows = AnalyticsRepository::worker_status_hourly(&state.db, &range).await?;

    let data: Vec<TimeSeriesPoint> = rows.into_iter().map(Into::into).collect();

    let response = WorkerStatusTimeSeriesResponse {
        since: range.since,
        until: range.until,
        data,
    };

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

/// Get enforcement volume over time.
///
/// Returns hourly buckets of enforcement creation counts, aggregated across all rules.
#[utoipa::path(
    get,
    path = "/api/v1/analytics/enforcements/volume",
    tag = "analytics",
    params(AnalyticsQueryParams),
    responses(
        (status = 200, description = "Enforcement volume", body = inline(ApiResponse<EnforcementVolumeResponse>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_enforcement_volume_analytics(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<AnalyticsQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_analytics_resource(&state, &user, Resource::Enforcements).await?;
    let range = query.to_time_range();
    let rows = AnalyticsRepository::enforcement_volume_hourly(&state.db, &range).await?;

    let data: Vec<TimeSeriesPoint> = rows.into_iter().map(Into::into).collect();

    let response = EnforcementVolumeResponse {
        since: range.since,
        until: range.until,
        data,
    };

    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the analytics routes.
///
/// Mounts:
/// - `GET /analytics/dashboard`              — combined dashboard payload
/// - `GET /analytics/executions/status`      — execution status transitions
/// - `GET /analytics/executions/throughput`   — execution creation throughput
/// - `GET /analytics/executions/failure-rate` — failure rate summary
/// - `GET /analytics/events/volume`          — event creation volume
/// - `GET /analytics/workers/status`         — worker status transitions
/// - `GET /analytics/enforcements/volume`    — enforcement creation volume
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/analytics/dashboard", get(get_dashboard_analytics))
        .route(
            "/analytics/executions/status",
            get(get_execution_status_analytics),
        )
        .route(
            "/analytics/executions/throughput",
            get(get_execution_throughput_analytics),
        )
        .route(
            "/analytics/executions/failure-rate",
            get(get_failure_rate_analytics),
        )
        .route("/analytics/events/volume", get(get_event_volume_analytics))
        .route(
            "/analytics/workers/status",
            get(get_worker_status_analytics),
        )
        .route(
            "/analytics/enforcements/volume",
            get(get_enforcement_volume_analytics),
        )
}
