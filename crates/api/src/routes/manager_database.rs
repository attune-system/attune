use std::collections::HashMap;

use attune_common::{
    kubernetes_manager_api::KubernetesManagerApi,
    manager_control::{ManagerDatabaseCredential, ManagerDatabaseCredentialRequest},
    repositories::manager_db_login::ManagerDatabaseLoginRepository,
};
use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use uuid::Uuid;

use super::managers::ManagerSessionAuthorization;
use crate::{auth::middleware::RequireAuth, middleware::error::ApiError, state::SharedState};

pub(super) async fn request(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Json(request): Json<ManagerDatabaseCredentialRequest>,
) -> Result<StatusCode, ApiError> {
    let config = state
        .config
        .manager_kubernetes_database
        .as_ref()
        .ok_or_else(denied)?;
    if request.request.is_nil()
        || request.generation < 1
        || request.pod_token.len() > 12_000
        || !attune_common::manager_db_client::valid_scram_verifier(&request.verifier)
    {
        return Err(ApiError::BadRequest(
            "Invalid database credential request".into(),
        ));
    }
    // Release the session lock during Kubernetes I/O. Reauthorize at write.
    let mut tx = state.db.begin().await?;
    let session = auth.live_record(&state, &mut tx).await?;
    tx.commit().await?;
    let pool = config
        .pools
        .iter()
        .find(|pool| pool.role == session.role && pool.pool == session.pool)
        .ok_or_else(denied)?;
    let kube = KubernetesManagerApi::in_cluster()
        .await
        .map_err(|_| unavailable())?;
    let caller = KubernetesManagerApi::access_token()
        .await
        .map_err(|_| unavailable())?;
    let review = kube
        .token_review(caller.trim(), &config.audience, &request.pod_token)
        .await
        .map_err(|_| unavailable())?;
    let review: TokenReview = serde_json::from_value(review).map_err(|_| unavailable())?;
    let pod_uid = verified_uid(
        review.status,
        &config.namespace,
        &config.audience,
        &pool.service_account,
        &session.name,
    )?;
    let account = kube
        .live_pod(caller.trim(), &config.namespace, &session.name, pod_uid)
        .await
        .map_err(|_| unavailable())?;
    if account.as_deref() != Some(pool.service_account.as_str()) {
        return Err(denied());
    }
    let mut tx = state.db.begin().await?;
    let current = auth.live_record(&state, &mut tx).await?;
    if !ManagerDatabaseLoginRepository::request(&mut tx, current.id, pod_uid, &request).await? {
        return Err(ApiError::Conflict(
            "Manager database credential generation changed".into(),
        ));
    }
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn current(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
) -> Result<Json<Option<ManagerDatabaseCredential>>, ApiError> {
    let config = state
        .config
        .manager_kubernetes_database
        .as_ref()
        .ok_or_else(denied)?;
    let mut tx = state.db.begin().await?;
    let session = auth.live_record(&state, &mut tx).await?;
    if !config
        .pools
        .iter()
        .any(|pool| pool.role == session.role && pool.pool == session.pool)
    {
        return Err(denied());
    }
    let result = ManagerDatabaseLoginRepository::current(&mut tx, session.id).await?;
    tx.commit().await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct TokenReview {
    status: ReviewStatus,
}

#[derive(Deserialize)]
struct ReviewStatus {
    authenticated: Option<bool>,
    #[serde(default)]
    audiences: Vec<String>,
    user: Option<ReviewUser>,
}

#[derive(Deserialize)]
struct ReviewUser {
    username: String,
    #[serde(default)]
    extra: HashMap<String, Vec<String>>,
}

fn verified_uid(
    review: ReviewStatus,
    namespace: &str,
    audience: &str,
    account: &str,
    name: &str,
) -> Result<Uuid, ApiError> {
    let user = review.user.ok_or_else(denied)?;
    let extra = |key: &str| match user.extra.get(key).map(Vec::as_slice) {
        Some([value]) => Some(value.as_str()),
        _ => None,
    };
    if review.authenticated != Some(true)
        || !review.audiences.iter().any(|value| value == audience)
        || user.username != format!("system:serviceaccount:{namespace}:{account}")
        || extra("authentication.kubernetes.io/pod-name") != Some(name)
    {
        return Err(denied());
    }
    extra("authentication.kubernetes.io/pod-uid")
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|uid| !uid.is_nil())
        .ok_or_else(denied)
}

fn denied() -> ApiError {
    ApiError::Forbidden("Kubernetes manager identity was not admitted".into())
}

fn unavailable() -> ApiError {
    ApiError::InternalServerError("Kubernetes manager identity verification is unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credential_proof_requires_the_expected_bound_token() {
        let identity = Uuid::new_v4();
        let review = |audience: &str| {
            serde_json::from_value(json!({
                "authenticated": true,
                "audiences": [audience],
                "user": {
                    "username": "system:serviceaccount:fixture:action-build",
                    "extra": {
                        "authentication.kubernetes.io/pod-name": ["action-build-0"],
                        "authentication.kubernetes.io/pod-uid": [identity.to_string()]
                    }
                }
            }))
            .unwrap()
        };
        assert_eq!(
            verified_uid(
                review("fixture-credentials"),
                "fixture",
                "fixture-credentials",
                "action-build",
                "action-build-0"
            )
            .unwrap(),
            identity
        );
        for (audience, account, pod) in [
            ("elsewhere", "action-build", "action-build-0"),
            ("fixture-credentials", "sensor-observe", "action-build-0"),
            ("fixture-credentials", "action-build", "action-build-1"),
        ] {
            assert!(matches!(
                verified_uid(
                    review(audience),
                    "fixture",
                    "fixture-credentials",
                    account,
                    pod
                ),
                Err(ApiError::Forbidden(_))
            ));
        }
    }
}
