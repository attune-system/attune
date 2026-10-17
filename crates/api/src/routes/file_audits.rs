//! Operator entry points under the existing retention maintenance API.
//! No execution/worker token, supplied endpoint, or caller assertion of stopped
//! writers can turn into a native settlement proof.
use crate::{
    auth::{middleware::AuthenticatedUser, RequireAuth},
    authz::AuthorizationCheck,
    dto::ApiResponse,
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    rbac::{Action, AuthorizationContext, Resource},
    repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
        file_legacy_audit::LegacyFileAuditRepository as Audits,
    },
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestAudit {
    versions: Vec<i64>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CandidatesCursor {
    after_version: i64,
    after_operation: Uuid,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ReceiptCursor {
    after_kind: String,
    after_identity: String,
}

async fn authorize(state: &AppState, user: &AuthenticatedUser, action: Action) -> ApiResult<()> {
    // The repository checks token kind/expiry and current unrestricted operator
    // management authority without borrowing a process sponsor's mutable IAM.
    tokio::time::timeout(Duration::from_secs(30), async {
        Audits::authorize_operator(&state.db, &user.claims).await?;
        state
            .authorization_service()
            .authorize(
                user,
                AuthorizationCheck {
                    resource: Resource::Retention,
                    action,
                    context: AuthorizationContext::new(0),
                },
            )
            .await
    })
    .await
    .map_err(|_| ApiError::RetryableDatabaseError)?
}

async fn bounded_db<T>(
    future: impl std::future::Future<Output = attune_common::Result<T>>,
) -> ApiResult<T> {
    Ok(tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .map_err(|_| ApiError::RetryableDatabaseError)??)
}

async fn capabilities(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Read).await?;
    Ok(Json(ApiResponse::new(json!({
        "scope":state.legacy_file_controller.scope(),
        "management_transport":if state.legacy_file_controller.available() {"configured_unverified"} else {"manual_required"},
        "diagnostic":"A configured controller must verify persistent native writer exclusion and provider request settlement, all historical versions/multipart sessions, exact namespace ownership and exact deletion. Standard S3/RustFS data APIs or SharedFile HEAD alone cannot prove those facts. Missing proof keeps quarantine. Configure the owning mTLS management controller or choose separately authorized export/isolation.",
        "max_members":256,"catalogue_page_size":128,"cleanup_page_size":16,
    }))))
}

async fn candidates(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(cursor): Query<CandidatesCursor>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Read).await?;
    if cursor.after_version < 0 {
        return Err(ApiError::BadRequest(
            "after_version must be nonnegative".into(),
        ));
    }
    Ok(Json(ApiResponse::new(
        bounded_db(Audits::candidates(
            &state.db,
            cursor.after_version,
            cursor.after_operation,
        ))
        .await?,
    )))
}

async fn request(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RequestAudit>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Update).await?;
    if body.versions.is_empty()
        || body.versions.len() > 256
        || body.versions.iter().any(|v| *v <= 0)
    {
        return Err(ApiError::BadRequest(
            "versions must contain 1..256 positive owned version IDs".into(),
        ));
    }
    let id = bounded_db(Audits::request(
        &state.db,
        &user.claims,
        &body.versions,
        state.legacy_file_controller.as_ref(),
    ))
    .await?;
    // An unconfigured or custom provider has an actionable persisted outcome,
    // rather than an unauditable reserved operation and an unreachable trait.
    if !state.legacy_file_controller.available() {
        bounded_db(Audits::step(
            &state.db,
            id,
            state.legacy_file_controller.as_ref(),
        ))
        .await?;
    }
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::new(
            bounded_db(Audits::status(&state.db, id)).await?,
        )),
    ))
}

async fn status(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Read).await?;
    Ok(Json(ApiResponse::new(
        bounded_db(Audits::status(&state.db, id)).await?,
    )))
}

async fn quarantine(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Update).await?;
    bounded_db(Audits::quarantine(&state.db, id, &user.claims)).await?;
    Ok(Json(ApiResponse::new(
        bounded_db(Audits::status(&state.db, id)).await?,
    )))
}

async fn receipts(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(cursor): Query<ReceiptCursor>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Read).await?;
    if cursor.after_kind.len() > 64 || cursor.after_identity.len() > 256 {
        return Err(ApiError::BadRequest(
            "receipt cursor exceeds its bound".into(),
        ));
    }
    let lease = state
        .file_transfers
        .maintenance()
        .map_err(ApiError::TooManyRequests)?;
    let work = state.clone();
    let records = state
        .file_transfers
        .spawn(lease, async move {
            tokio::time::timeout(
                Duration::from_secs(30),
                Audits::receipts(&work.db, id, &cursor.after_kind, &cursor.after_identity),
            )
            .await
        })
        .await
        .map_err(|_| ApiError::RetryableDatabaseError)?
        .ok_or(ApiError::RetryableDatabaseError)?
        .map_err(|_| ApiError::RetryableDatabaseError)??;
    Ok(Json(ApiResponse::new(records)))
}

async fn step(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Update).await?;
    let lease = state
        .file_transfers
        .maintenance()
        .map_err(ApiError::TooManyRequests)?;
    let work = state.clone();
    // Detached socket handles do not detach ownership. This task is tracked by
    // the API's existing shutdown drain, shares bounded slots/memory, and has a
    // finite step deadline. Timeout/cancellation leaves remote work UNSETTLED.
    let result = state
        .file_transfers
        .spawn(lease, async move {
            tokio::time::timeout(
                Duration::from_secs(120),
                Audits::step(&work.db, id, work.legacy_file_controller.as_ref()),
            )
            .await
        })
        .await
        .map_err(|_| ApiError::RetryableDatabaseError)?
        .ok_or(ApiError::RetryableDatabaseError)?
        .map_err(|_| ApiError::RetryableDatabaseError)??;
    Ok(Json(ApiResponse::new(json!({"id":id,"state":result}))))
}

async fn readmit(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(version): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Update).await?;
    if version <= 0 {
        return Err(ApiError::BadRequest("version must be positive".into()));
    }
    // Retain the exact original frozen graph. This never reconstructs missing
    // authority from current configuration, nor settles any physical writer.
    let lease = state
        .file_transfers
        .maintenance()
        .map_err(ApiError::TooManyRequests)?;
    let work = state.clone();
    let retained = state
        .file_transfers
        .spawn(lease, async move {
            tokio::time::timeout(
                Duration::from_secs(120),
                Evidence::readmit(&work.db, version),
            )
            .await
        })
        .await
        .map_err(|_| ApiError::RetryableDatabaseError)?
        .ok_or(ApiError::RetryableDatabaseError)?
        .map_err(|_| ApiError::RetryableDatabaseError)??;
    Ok(Json(ApiResponse::new(
        json!({"version":version,"readmitted":retained,"diagnostic":if retained {"Frozen evidence retained; runtime credentials and source disclosure still require their independent checks"} else {"Not readmitted. Inspect retained version diagnostics, repair the exact frozen proof, or request separately authorized physical cleanup; unknown writer operations remain pinned"}}),
    )))
}

async fn evidence_status(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(version): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &user, Action::Read).await?;
    if version <= 0 {
        return Err(ApiError::BadRequest("version must be positive".into()));
    }
    Ok(Json(ApiResponse::new(
        bounded_db(Audits::evidence_status(&state.db, version)).await?,
    )))
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/retention-config/file-audits", post(request))
        .route(
            "/retention-config/file-audits/capabilities",
            get(capabilities),
        )
        .route("/retention-config/file-audits/candidates", get(candidates))
        .route("/retention-config/file-audits/{id}", get(status))
        .route("/retention-config/file-audits/{id}/receipts", get(receipts))
        .route("/retention-config/file-audits/{id}/step", post(step))
        .route(
            "/retention-config/file-audits/{id}/quarantine",
            post(quarantine),
        )
        .route(
            "/retention-config/file-evidence/{version}",
            get(evidence_status),
        )
        .route(
            "/retention-config/file-evidence/{version}/readmit",
            post(readmit),
        )
}
