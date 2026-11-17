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
    let response = finalize_response(decision, user.claims.exp, &state.jwt_config, || {
        chrono::Utc::now().timestamp()
    })?;
    Ok(Json(response))
}

fn finalize_response(
    decision: attune_common::repositories::log_delivery::ControlDecision,
    caller_deadline: i64,
    jwt: &attune_common::auth::jwt::JwtConfig,
    mut now: impl FnMut() -> i64,
) -> ApiResult<ControlResponse> {
    let fresh = |time| {
        if caller_deadline <= time {
            Err(ApiError::Unauthorized("Worker credential expired".into()))
        } else {
            Ok(())
        }
    };
    let response = match decision {
        attune_common::repositories::log_delivery::ControlDecision::Settled => {
            ControlResponse::Settled
        }
        attune_common::repositories::log_delivery::ControlDecision::Waiting => {
            ControlResponse::Waiting
        }
        attune_common::repositories::log_delivery::ControlDecision::Deliver(fence) => {
            // A scheduling pause after repository commit cannot extend the
            // caller credential across issuance or response publication.
            fresh(now())?;
            let token = attune_common::log_delivery::mint(&fence.intent, jwt)?;
            fence.ensure_fresh()?;
            ControlResponse::Deliver { token }
        }
    };
    fresh(now())?;
    Ok(response)
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/internal/log-delivery/control", post(reconcile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::repositories::log_delivery::{
        ControlDecision, LogDeliveryFence, LogDeliveryIntent,
    };

    fn decision() -> ControlDecision {
        let now = chrono::Utc::now();
        ControlDecision::Deliver(Box::new(LogDeliveryFence {
            intent: LogDeliveryIntent {
                id: 1,
                execution: 2,
                artifact_version: 3,
                stream: 4,
                worker: 5,
                source_instance: Uuid::new_v4(),
                delivery_instance: Uuid::new_v4(),
                attempt: Uuid::new_v4(),
                owner: 6,
                custody: Uuid::new_v4(),
                state: "active".into(),
                revision: 1,
                segments: sqlx::types::Json(vec![]),
                total_bytes: 1,
                sha256: "0".repeat(64),
                truncated: false,
                authenticated_manifest: None,
                finalize_by: now + chrono::Duration::minutes(1),
                failure: None,
                created: now,
                updated: now,
            },
        }))
    }
    fn jwt() -> attune_common::auth::jwt::JwtConfig {
        attune_common::auth::install_crypto_provider();
        attune_common::auth::jwt::JwtConfig {
            secret: "owned-issuance-boundary-test-key".into(),
            access_token_expiration: 60,
            refresh_token_expiration: 60,
        }
    }

    #[test]
    fn caller_expiry_between_commit_and_issuance_denies_delivery_credential() {
        let jwt = jwt();
        assert!(matches!(
            finalize_response(decision(), 20, &jwt, || 20),
            Err(ApiError::Unauthorized(_))
        ));
    }

    #[test]
    fn caller_expiry_during_signing_denies_response_publication() {
        let jwt = jwt();
        let mut clock = [19, 20].into_iter();
        assert!(matches!(
            finalize_response(decision(), 20, &jwt, || clock.next().unwrap()),
            Err(ApiError::Unauthorized(_))
        ));
    }

    #[test]
    fn live_caller_receives_only_exact_delivery_credential() {
        let jwt = jwt();
        let ControlResponse::Deliver { token } =
            finalize_response(decision(), 20, &jwt, || 19).unwrap()
        else {
            panic!("delivery credential missing");
        };
        let claims = attune_common::auth::jwt::validate_token(&token, &jwt).unwrap();
        let binding = attune_common::log_delivery::credential(&claims)
            .unwrap()
            .unwrap();
        assert_eq!((binding.intent, binding.version, binding.worker), (1, 3, 5));
    }
}
