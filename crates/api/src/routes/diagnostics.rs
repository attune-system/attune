//! Authenticated operational diagnostics endpoints.

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use std::sync::Arc;

use attune_common::{
    metadata_cache::MetadataCacheStatsSnapshot,
    rbac::{Action, AuthorizationContext, Resource},
};

use crate::{
    auth::RequireAuth,
    authz::{AuthorizationCheck, AuthorizationService},
    dto::ApiResponse,
    middleware::ApiResult,
    state::AppState,
};

fn diagnostics_check() -> AuthorizationCheck {
    AuthorizationCheck {
        resource: Resource::Retention,
        action: Action::Read,
        context: AuthorizationContext::new(0),
    }
}

/// Get in-process metadata cache statistics.
#[utoipa::path(
    get,
    path = "/api/v1/diagnostics/metadata-cache",
    tag = "diagnostics",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Metadata cache statistics", body = ApiResponse<MetadataCacheStatsSnapshot>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    )
)]
pub async fn get_metadata_cache_stats(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> ApiResult<impl IntoResponse> {
    let authz = AuthorizationService::new(state.db.clone());
    authz.authorize(&user.0, diagnostics_check()).await?;

    let stats = state.metadata_cache.stats_snapshot().await;
    Ok((StatusCode::OK, Json(ApiResponse::new(stats))))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/diagnostics/metadata-cache", get(get_metadata_cache_stats))
}
