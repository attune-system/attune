//! The manager's dedicated current-incarnation identity may reserve and settle
//! only its assigned exact execution log custody. No signing/decryption inputs.
use super::*;
use attune_common::{
    log_custody::{
        CustodyAbsence, CustodyOperationAck, PrepareCustody, ReserveCustody,
        MAX_CUSTODY_REQUEST_BYTES,
    },
    repositories::custody_operation::CustodyOperationRepository as Custody,
};
use axum::extract::{rejection::JsonRejection, DefaultBodyLimit};

fn unavailable() -> ApiError {
    ApiError::Forbidden("Log custody operation unavailable".into())
}
fn fresh(deadline: chrono::DateTime<chrono::Utc>) -> ApiResult<()> {
    let deadline = chrono::DateTime::from_timestamp_micros(deadline.timestamp_micros())
        .ok_or_else(unavailable)?;
    if deadline <= chrono::Utc::now() {
        return Err(unavailable());
    }
    Ok(())
}
#[utoipa::path(operation_id="workload_host_custody_reserve",post,path="/api/v1/workload-hosts/custody/reserve",request_body=ReserveCustody,responses((status=200,body=CustodyOperationAck),(status=403,description="Custody unavailable")),tag="workload-control")]
pub async fn reserve(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    request: Result<Json<ReserveCustody>, JsonRejection>,
) -> ApiResult<(HeaderMap, Json<CustodyOperationAck>)> {
    let actor = workload_host::host_actor(&user.claims)?;
    let Json(request) = request.map_err(|_| unavailable())?;
    let result = Custody::reserve(&state.db, actor, &request).await?;
    fresh(request.finalize_by)?;
    response(actor.expires_at, result)
}
#[utoipa::path(operation_id="workload_host_custody_prepare",post,path="/api/v1/workload-hosts/custody/prepare",request_body=PrepareCustody,responses((status=200,body=CustodyOperationAck),(status=403,description="Custody unavailable")),tag="workload-control")]
pub async fn prepare(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    request: Result<Json<PrepareCustody>, JsonRejection>,
) -> ApiResult<(HeaderMap, Json<CustodyOperationAck>)> {
    let actor = workload_host::host_actor(&user.claims)?;
    let Json(request) = request.map_err(|_| unavailable())?;
    let result = Custody::prepare(&state.db, actor, &request).await?;
    fresh(request.reservation.finalize_by)?;
    response(actor.expires_at, result)
}
#[utoipa::path(operation_id="workload_host_custody_cancel_absent",post,path="/api/v1/workload-hosts/custody/cancel-absent",request_body=ReserveCustody,responses((status=200,body=CustodyAbsence),(status=403,description="Custody unavailable")),tag="workload-control")]
pub async fn cancel_absent(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    request: Result<Json<ReserveCustody>, JsonRejection>,
) -> ApiResult<(HeaderMap, Json<CustodyAbsence>)> {
    let actor = workload_host::host_actor(&user.claims)?;
    let Json(request) = request.map_err(|_| unavailable())?;
    response(
        actor.expires_at,
        Custody::cancel_absent(&state.db, actor, &request).await?,
    )
}
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workload-hosts/custody/reserve", post(reserve))
        .route("/workload-hosts/custody/prepare", post(prepare))
        .route("/workload-hosts/custody/cancel-absent", post(cancel_absent))
        .layer(DefaultBodyLimit::max(MAX_CUSTODY_REQUEST_BYTES))
        .layer(Extension(WorkloadHostConsumer))
}
