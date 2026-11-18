//! Dedicated host enrollment and session transport. Every request resolves an
//! operator-admitted profile; none accepts runtime plans or host credentials.
use crate::{
    auth::middleware::RequireAuth,
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    repositories::workload_host::WorkloadHostRepository,
    workload_host::{self, RegisterWorkloadHost, RegisteredWorkloadHost},
    workload_session::{RenewWorkloadSession, WorkloadSession},
};
use axum::{
    extract::State,
    http::{header::CACHE_CONTROL, HeaderMap, HeaderValue},
    routing::post,
    Extension, Json, Router,
};
use std::sync::Arc;
#[derive(Clone, Copy)]
pub(crate) struct WorkloadHostConsumer;
#[derive(Clone, Copy)]
pub(crate) struct WorkloadHostEnrollmentConsumer;
fn response<T>(caller_expiry: i64, value: T) -> ApiResult<(HeaderMap, Json<T>)> {
    if caller_expiry <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized("Workload credential expired".into()));
    }
    Ok((
        HeaderMap::from_iter([(CACHE_CONTROL, HeaderValue::from_static("no-store"))]),
        Json(value),
    ))
}
#[utoipa::path(operation_id="workload_host_register",post,path="/api/v1/workload-hosts/register",request_body=RegisterWorkloadHost,responses((status=200,body=RegisteredWorkloadHost),(status=403,description="Host enrollment unavailable")),tag="workload-control")]
pub async fn register(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<RegisterWorkloadHost>,
) -> ApiResult<(HeaderMap, Json<RegisteredWorkloadHost>)> {
    let actor = workload_host::enrollment_actor(&user.claims)?;
    let registered =
        WorkloadHostRepository::register(&state.db, actor, &request, &state.jwt_config).await?;
    response(actor.expires_at, registered)
}
#[utoipa::path(operation_id="workload_host_session",post,path="/api/v1/workload-hosts/session",request_body=RenewWorkloadSession,responses((status=200,body=WorkloadSession),(status=403,description="Host session unavailable")),tag="workload-control")]
pub async fn session(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(_request): Json<RenewWorkloadSession>,
) -> ApiResult<(HeaderMap, Json<WorkloadSession>)> {
    let actor = workload_host::host_actor(&user.claims)?;
    let renewed = WorkloadHostRepository::renew(&state.db, actor, &state.jwt_config).await?;
    response(actor.expires_at, renewed)
}
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/workload-hosts/register",
            post(register).layer(Extension(WorkloadHostEnrollmentConsumer)),
        )
        .route(
            "/workload-hosts/session",
            post(session).layer(Extension(WorkloadHostConsumer)),
        )
}
