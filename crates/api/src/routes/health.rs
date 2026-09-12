//! Health check endpoints

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::state::AppState;
use attune_common::repositories::{pack::PackRepository, pack_release::PackReleaseRepository};

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
pub async fn readiness(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match PackRepository::list_requiring_release(&state.db).await {
        Ok(packs) if packs.is_empty() => {
            match PackReleaseRepository::find_active_by_pack_ref(&state.db, "core").await {
                Ok(Some(_)) => StatusCode::OK.into_response(),
                Ok(None) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "status": "not_ready",
                        "error": "core pack has no active immutable release",
                        "packs": ["core"]
                    })),
                )
                    .into_response(),
                Err(e) => {
                    tracing::error!("Core pack readiness check failed: {}", e);
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
            }
        }
        Ok(packs) => {
            let pack_refs = packs.into_iter().map(|pack| pack.r#ref).collect::<Vec<_>>();
            tracing::error!(
                ?pack_refs,
                "Readiness blocked by packs without immutable releases"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "not_ready",
                    "error": "installed packs require immutable release upgrade",
                    "packs": pack_refs,
                    "repair": "restore each pack's exact installed directory, then run: attune pack register <server-visible-pack-directory> --force --skip-tests"
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Readiness check failed: {}", e);
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

/// Create health check router
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/health/detailed", get(health_detailed))
        .route("/health/ready", get(readiness))
        .route("/health/live", get(liveness))
}
