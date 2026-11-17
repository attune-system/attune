//! Worker-targeted custody reconciliation and control-plane credential issuance.
use std::sync::Arc;

use attune_common::{
    auth::jwt::TokenType,
    log_delivery::{ControlRequest, ControlResponse},
    repositories::log_delivery::LogDeliveryRepository,
};
use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    middleware::{ApiError, ApiResult},
    state::AppState,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerBinding {
    worker_id: String,
    worker_instance: Uuid,
}

fn binding(user: &AuthenticatedUser) -> ApiResult<(i64, Uuid)> {
    if user.claims.token_type != TokenType::Worker
        || user.claims.scope.as_deref() != Some("internal_files")
    {
        return Err(denied());
    }
    let binding: WorkerBinding =
        serde_json::from_value(user.claims.metadata.clone().ok_or_else(denied)?)
            .map_err(|_| denied())?;
    let worker = binding.worker_id.parse::<i64>().map_err(|_| denied())?;
    if worker <= 0 || binding.worker_id != worker.to_string() || binding.worker_instance.is_nil() {
        return Err(denied());
    }
    Ok((worker, binding.worker_instance))
}

fn denied() -> ApiError {
    ApiError::Forbidden("Log custody control unavailable".into())
}

#[utoipa::path(
    post,
    path = "/api/v1/internal/log-delivery/control",
    request_body = ControlRequest,
    responses((status = 200, body = ControlResponse), (status = 403, description = "Custody unavailable")),
    tag = "internal"
)]
pub async fn reconcile(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<ControlRequest>,
) -> ApiResult<Json<ControlResponse>> {
    let (worker, instance) = binding(&user)?;
    if [request.execution, request.version, request.stream]
        .iter()
        .any(|id| *id <= 0)
        || request.attempt.is_nil()
        || request.source_instance.is_nil()
        || request.custody.is_nil()
    {
        return Err(denied());
    }
    let decision = LogDeliveryRepository::reconcile_control(
        &state.db,
        worker,
        instance,
        &request,
        user.claims.exp,
    )
    .await?;
    let response = match decision {
        attune_common::repositories::log_delivery::ControlDecision::Settled => {
            ControlResponse::Settled
        }
        attune_common::repositories::log_delivery::ControlDecision::Waiting => {
            ControlResponse::Waiting
        }
        attune_common::repositories::log_delivery::ControlDecision::Deliver(fence) => {
            let token = attune_common::log_delivery::mint(&fence.intent, &state.jwt_config)?;
            fence.ensure_fresh()?;
            ControlResponse::Deliver { token }
        }
    };
    Ok(Json(response))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/internal/log-delivery/control", post(reconcile))
}
