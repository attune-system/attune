//! Private exact-byte input; resource admission and authentication precede body
//! decoding. Dedicated errors never expose selected data or field names.
use super::*;
use attune_common::workload_stdin::{AuthorizeLaunchStdinRequest, MAX_PRIVATE_STDIN_REQUEST_BYTES};
use axum::body::{to_bytes, Body};

#[utoipa::path(operation_id="workload_launch_authorize_stdin",post,path="/api/v1/workload-launches/{id}/authorize-stdin",params(("id"=i64,Path),("x-attune-launch-lease"=String,Header,description="Exact target-scoped grant"),("x-attune-launch-io"=String,Header,description="Exact stdin grant")),request_body=AuthorizeLaunchStdinRequest,responses((status=200,body=LaunchIoAuthorization),(status=403,description="Private input unavailable")),tag="workload-control")]
pub async fn authorize(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<LaunchIoAuthorization>> {
    let actor = actor(&user)?;
    let lease = token(&headers, LAUNCH_LEASE_HEADER)?;
    let grant = token(&headers, LAUNCH_IO_HEADER)?;
    let _memory = state
        .file_transfers
        .private_stdin()
        .map_err(|_| ApiError::TooManyRequests("Private input admission unavailable".into()))?;
    let bytes = to_bytes(body, MAX_PRIVATE_STDIN_REQUEST_BYTES)
        .await
        .map_err(|_| ApiError::BadRequest("Invalid private input body".into()))?;
    let request: AuthorizeLaunchStdinRequest = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::BadRequest("Invalid private input body".into()))?;
    handle(id, &request.handle)?;
    let authorization = Ledger::authorize_stdin(
        &state.db,
        actor,
        &request,
        &lease,
        &grant,
        state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or_else(denied)?,
    )
    .await?;
    fresh_deadline(authorization.expires_at)?;
    response(actor, authorization)
}
