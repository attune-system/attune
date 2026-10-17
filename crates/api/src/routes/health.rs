//! Health check endpoints

use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::state::AppState;
use attune_common::{
    platform_catalog::{CATALOG_REVISION, COMPATIBILITY_EPOCH},
    repositories::HealthRepository,
};

/// Release enablement gate, not an operator/metadata override. The integrator
/// enables this only after ordinary API activation captures policy and consumes
/// opt-in closed-context evidence, and the coordinated runtime rollout is ready.
/// Advertising support before those hooks exist could replace an admitted core
/// with unadmitted content before bootstrap's post-upload proof fails.
pub use attune_common::pack_registry::deployment_admission::BOOTSTRAP_ADMISSION_ENABLED;

/// Health check response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Service status
    #[schema(example = "ok")]
    pub status: String,
    /// Service version
    pub version: String,
    /// Database connectivity status
    #[schema(example = "connected")]
    pub database: String,
}

/// Platform health check endpoint.
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Database and exact platform catalog are ready", body = inline(Object)),
        (status = 503, description = "Platform is not ready", body = inline(Object))
    )
)]
pub async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    platform_health(&state, "ok").await
}

/// Detailed health check endpoint
///
/// Checks database connectivity and returns detailed status
#[utoipa::path(
    get,
    path = "/health/detailed",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy with details", body = HealthResponse),
        (status = 503, description = "Service unavailable", body = inline(Object))
    )
)]
pub async fn health_detailed(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db_status = match HealthRepository::platform(&state.db).await {
        Ok(platform)
            if platform.compatibility_epoch == COMPATIBILITY_EPOCH
                && platform.catalog_revision == CATALOG_REVISION =>
        {
            "connected"
        }
        Ok(platform) => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "error",
                    "database": "connected",
                    "error": "Platform catalog version does not match this API",
                    "catalog": {
                        "compatibility_epoch": platform.compatibility_epoch,
                        "revision": platform.catalog_revision,
                        "expected_compatibility_epoch": COMPATIBILITY_EPOCH,
                        "expected_revision": CATALOG_REVISION
                    }
                })),
            ));
        }
        Err(e) => {
            tracing::error!("Database health check failed: {}", e);
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "error",
                    "database": "disconnected",
                    "error": "Database connectivity check failed"
                })),
            ));
        }
    };

    let response = HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        database: db_status.to_string(),
    };

    Ok((StatusCode::OK, Json(response)))
}

/// Readiness check endpoint
///
/// Returns 200 OK if the service is ready to accept requests
#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    responses(
        (status = 200, description = "Service is ready"),
        (status = 503, description = "Service not ready")
    )
)]
pub async fn readiness(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    platform_health(&state, "ready").await
}

/// Admission readiness without worker/content dependencies, for bootstrap jobs.
pub async fn bootstrap(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let platform = platform_health(&state, "ready").await;
    if platform.status() != StatusCode::OK {
        return platform;
    }
    match HealthRepository::admitted_core(&state.db).await {
        Ok(Some(release)) => (StatusCode::OK, Json(serde_json::json!({"status":"ready", "core":release, "bootstrap_admission":attune_common::pack_registry::deployment_admission::descriptor()}))).into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"status":"not_ready", "error":"core release has no current runtime admission"}))).into_response(),
    }
}

async fn platform_health(state: &AppState, ready_status: &'static str) -> axum::response::Response {
    match HealthRepository::platform(&state.db).await {
        Ok(platform)
            if platform.compatibility_epoch == COMPATIBILITY_EPOCH
                && platform.catalog_revision == CATALOG_REVISION =>
        {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": ready_status,
                    "pack_publication": attune_common::pack_registry::deployment_admission::publication_descriptor(),
                    "bootstrap_admission_protocol": BOOTSTRAP_ADMISSION_ENABLED.then_some(1),
                    "bootstrap_admission": attune_common::pack_registry::deployment_admission::descriptor(),
                    "catalog": {
                        "compatibility_epoch": platform.compatibility_epoch,
                        "revision": platform.catalog_revision
                    }
                })),
            )
                .into_response()
        }
        Ok(platform) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "error": "platform catalog version mismatch",
                "catalog": {
                    "compatibility_epoch": platform.compatibility_epoch,
                    "revision": platform.catalog_revision,
                    "expected_compatibility_epoch": COMPATIBILITY_EPOCH,
                    "expected_revision": CATALOG_REVISION
                }
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "Platform health check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "not_ready",
                    "error": "database or platform catalog unavailable"
                })),
            )
                .into_response()
        }
    }
}

/// Transitional content and coarse host-capability health.
///
/// Required-pack locks and candidate evidence replace this contract in issue #75.
#[utoipa::path(
    get,
    path = "/health/content",
    tag = "health",
    responses(
        (status = 200, description = "Core content and coarse host capabilities are available", body = inline(Object)),
        (status = 503, description = "Content or coarse host capabilities are absent", body = inline(Object))
    )
)]
pub async fn content(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match HealthRepository::content(&state.db).await {
        Ok(content) => {
            let platform_ready =
                HealthRepository::platform(&state.db)
                    .await
                    .is_ok_and(|platform| {
                        platform.compatibility_epoch == COMPATIBILITY_EPOCH
                            && platform.catalog_revision == CATALOG_REVISION
                    });
            let core_admitted = HealthRepository::admitted_core(&state.db)
                .await
                .is_ok_and(|core| core.is_some());
            let ready = content.core_active
                && core_admitted
                && platform_ready
                && content.action_host_available
                && content.sensor_host_available;
            (
                if ready {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                Json(serde_json::json!({
                    "status": if ready { "ready" } else { "not_ready" },
                    "content": { "core_active": content.core_active, "core_admitted": core_admitted },
                    "capabilities": {
                        "action_host_available": content.action_host_available,
                        "sensor_host_available": content.sensor_host_available
                    },
                    "transitional": true,
                    "transition": "Replaced by required-pack locks and candidate evidence in issue #75"
                })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(%error, "Content health check failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// Liveness check endpoint
///
/// Returns 200 OK if the service process is alive
#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    responses(
        (status = 200, description = "Service is alive")
    )
)]
pub async fn liveness() -> impl IntoResponse {
    StatusCode::OK
}

/// Prometheus text metrics for execution log streaming.
pub async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        state.execution_log_streams.render_metrics(),
    )
}

/// Create health check router
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/health/detailed", get(health_detailed))
        .route("/health/ready", get(readiness))
        .route("/health/bootstrap", get(bootstrap))
        .route("/health/live", get(liveness))
        .route("/health/content", get(content))
        .route("/metrics", get(metrics))
}
