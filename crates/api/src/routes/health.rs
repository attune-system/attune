//! Health check endpoints

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::state::AppState;

/// Health check response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Service status
    #[schema(example = "ok")]
    pub status: String,
    /// Service version
    #[schema(example = "0.2.1")]
    pub version: String,
    /// Database connectivity status
    #[schema(example = "connected")]
    pub database: String,
}

/// Basic health check endpoint
///
/// Returns 200 OK if the service is running
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy", body = inline(Object), example = json!({"status": "ok"}))
    )
)]
pub async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok"
        })),
    )
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
    // Check database connectivity
    let db_status = match sqlx::query("SELECT 1").fetch_one(&state.db).await {
        Ok(_) => "connected",
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
pub async fn readiness(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, StatusCode> {
    // Check if database is ready
    match sqlx::query("SELECT 1").fetch_one(&state.db).await {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            tracing::error!("Readiness check failed: {}", e);
            Err(StatusCode::SERVICE_UNAVAILABLE)
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

/// Create health check router
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/health/detailed", get(health_detailed))
        .route("/health/ready", get(readiness))
        .route("/health/live", get(liveness))
}
