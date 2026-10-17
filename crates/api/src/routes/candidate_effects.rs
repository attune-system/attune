//! Capability-only candidate ingress, deliberately separate from JWT resource consumers.
//! The repository authenticates the opaque candidate secret and admits the effect in
//! one transaction. Access/execution/worker JWTs alone cannot call this endpoint.

use crate::auth::middleware::RequireAuth;
use crate::state::AppState;
use attune_common::repositories::candidate_effect::{
    AuthenticatedCandidateContext, CandidateEffectRepository, CandidateOperation,
};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use std::sync::Arc;
use uuid::Uuid;

impl axum::extract::FromRequestParts<crate::state::SharedState>
    for RequireAuth<AuthenticatedCandidateContext>
{
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &crate::state::SharedState,
    ) -> Result<Self, Self::Rejection> {
        let Path(lease) = <Path<Uuid> as axum::extract::FromRequestParts<
            crate::state::SharedState,
        >>::from_request_parts(parts, state)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
        let token = parts
            .headers
            .get("x-attune-candidate-effects")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty() && value.len() <= 256)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let context = CandidateEffectRepository::authenticate(&state.db, lease, token)
            .await
            .map_err(|_| StatusCode::FORBIDDEN)?;
        Ok(RequireAuth(context))
    }
}

pub async fn apply(
    State(state): State<Arc<AppState>>,
    RequireAuth(context): RequireAuth<AuthenticatedCandidateContext>,
    Json(operation): Json<CandidateOperation>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let result = CandidateEffectRepository::apply_authenticated(&state.db, &context, operation)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    Ok(Json(serde_json::json!({"data": result})))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/candidates/{id}/effects", post(apply))
        .layer(DefaultBodyLimit::max(70 * 1024))
}
