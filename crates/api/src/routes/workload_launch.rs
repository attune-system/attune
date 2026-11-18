//! Dedicated authenticated control transport. Engine requests contain only CP
//! admitted IDs/digests; no raw image/path/HostConfig input is accepted here.
use crate::{
    auth::middleware::RequireAuth,
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    repositories::workload_launch::{ControllerActor, WorkloadLaunchRepository as Ledger},
    workload_launch::*,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::post,
    Extension, Json, Router,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) struct WorkloadLaunchConsumer;
fn denied() -> ApiError {
    ApiError::Forbidden("Workload launch unavailable".into())
}
fn actor(user: &crate::auth::middleware::AuthenticatedUser) -> ApiResult<ControllerActor> {
    Ok(attune_common::workload_controller::actor(&user.claims)?)
}
fn token(headers: &HeaderMap, name: &str) -> ApiResult<LaunchToken> {
    // Duplicate credentials are ambiguous and rejected, never first-value wins.
    if headers.get_all(name).iter().count() != 1 {
        return Err(denied());
    }
    LaunchToken::new(
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(denied)?
            .into(),
    )
    .map_err(|_| denied())
}
fn key(id: i64, request: &LaunchRequest) -> ApiResult<()> {
    if id <= 0 || id != request.launch_id {
        Err(denied())
    } else {
        Ok(())
    }
}
fn handle(id: i64, request: &LaunchHandle) -> ApiResult<()> {
    if id <= 0 || id != request.launch_id {
        Err(denied())
    } else {
        Ok(())
    }
}
fn response<T>(actor: ControllerActor, value: T) -> ApiResult<Json<T>> {
    if actor.expires_at <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized(
            "Controller credential expired".into(),
        ));
    }
    Ok(Json(value))
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/resolve",params(("id"=i64,Path)),request_body=LaunchRequest,responses((status=200,body=LaunchManifest),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn resolve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> ApiResult<Json<LaunchManifest>> {
    key(id, &request)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_DISPATCH_HEADER)?;
    let result = Ledger::resolve(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/claim",params(("id"=i64,Path)),request_body=LaunchRequest,responses((status=200,body=ClaimedLaunch),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn claim(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> ApiResult<Json<ClaimedLaunch>> {
    key(id, &request)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_DISPATCH_HEADER)?;
    let result = Ledger::claim(
        &state.db,
        actor,
        &request,
        &credential,
        state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or_else(denied)?,
    )
    .await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/status",params(("id"=i64,Path)),request_body=LaunchRequest,responses((status=200,body=LaunchStatus),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> ApiResult<Json<LaunchStatus>> {
    key(id, &request)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_DISPATCH_HEADER)?;
    let result = Ledger::status(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/stop",params(("id"=i64,Path)),request_body=StopLaunchRequest,responses((status=200,body=LaunchStatus),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn stop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<StopLaunchRequest>,
) -> ApiResult<Json<LaunchStatus>> {
    key(id, &request.request)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_DISPATCH_HEADER)?;
    let result = Ledger::stop(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/create-intent",params(("id"=i64,Path)),request_body=LaunchLeaseRequest,responses((status=200,body=LaunchCreateIntent),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn create_intent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchLeaseRequest>,
) -> ApiResult<Json<LaunchCreateIntent>> {
    handle(id, &request.handle)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_LEASE_HEADER)?;
    let result = Ledger::create_intent(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/start",params(("id"=i64,Path)),request_body=LaunchLeaseRequest,responses((status=200,body=LaunchStartIntent),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchLeaseRequest>,
) -> ApiResult<Json<LaunchStartIntent>> {
    handle(id, &request.handle)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_LEASE_HEADER)?;
    let result = Ledger::start_intent(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/renew",params(("id"=i64,Path)),request_body=LaunchLeaseRequest,responses((status=200,body=LaunchLease),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn renew(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchLeaseRequest>,
) -> ApiResult<Json<LaunchLease>> {
    handle(id, &request.handle)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_LEASE_HEADER)?;
    let result = Ledger::renew(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/observe",params(("id"=i64,Path)),request_body=ObserveLaunchRequest,responses((status=200,body=LaunchStatus),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn observe(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<ObserveLaunchRequest>,
) -> ApiResult<Json<LaunchStatus>> {
    handle(id, &request.handle)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_LEASE_HEADER)?;
    let result = Ledger::observe(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/stop-ack",params(("id"=i64,Path)),request_body=StopLaunchAcknowledgement,responses((status=200,body=LaunchStatus),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn stop_ack(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<StopLaunchAcknowledgement>,
) -> ApiResult<Json<LaunchStatus>> {
    handle(id, &request.handle)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_LEASE_HEADER)?;
    let result = Ledger::stop_ack(&state.db, actor, &request, &credential).await?;
    response(actor, result)
}

#[utoipa::path(post,path="/api/v1/workload-launches/{id}/inputs/{pin_id}/body",params(("id"=i64,Path),("pin_id"=i64,Path)),request_body=LaunchLeaseRequest,responses((status=200,description="Exact pinned ZIP body",content_type="application/zip"),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn input_body(
    State(state): State<Arc<AppState>>,
    Path((id, pin_id)): Path<(i64, i64)>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchLeaseRequest>,
) -> ApiResult<axum::response::Response> {
    use attune_common::{
        blob_store::{ObjectKey, ProviderVersion},
        repositories::workload_launch::WorkloadLaunchRepository,
    };
    use futures::TryStreamExt;
    handle(id, &request.handle)?;
    let actor = actor(&user)?;
    let credential = token(&headers, LAUNCH_LEASE_HEADER)?;
    let pin = Ledger::input(&state.db, actor, &request.handle, &credential, pin_id).await?;
    let digest: [u8; 32] = hex::decode(&pin.sha256)
        .map_err(|_| denied())?
        .try_into()
        .map_err(|_| denied())?;
    let key = ObjectKey::new(pin.object_key).map_err(|_| denied())?;
    let version = ProviderVersion::from_stored(pin.provider_version).map_err(|_| denied())?;
    let reader = state
        .blob_store
        .get_pinned(&key, &version, pin.size_bytes as u64, digest, None)
        .await
        .map_err(|_| denied())?;
    // Each finite body chunk is freshly fenced. Revocation/expiry during provider
    // waits cannot continue an already-open stream with historical admission.
    let pool = state.db.clone();
    let handle = request.handle;
    let body = futures::stream::try_unfold(reader, move |mut body| {
        let pool = pool.clone();
        let credential = credential.clone();
        let handle = handle.clone();
        async move {
            let chunk = body
                .try_next()
                .await
                .map_err(|_| std::io::Error::other("Launch input unavailable"))?;
            match chunk {
                None => Ok::<_, std::io::Error>(None),
                Some(bytes) => {
                    WorkloadLaunchRepository::input(&pool, actor, &handle, &credential, pin_id)
                        .await
                        .map_err(|_| std::io::Error::other("Launch input unavailable"))?;
                    Ok(Some((bytes, body)))
                }
            }
        }
    });
    axum::response::Response::builder()
        .header("content-type", LAUNCH_ARCHIVE_CONTENT_TYPE)
        .header("content-length", pin.size_bytes.to_string())
        .body(axum::body::Body::from_stream(body))
        .map_err(|_| denied())
}
#[utoipa::path(post,path="/api/v1/workload-launches/reconcile",request_body=ReconcileLaunchesRequest,responses((status=200,body=ReconcileLaunchesPage),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn reconcile(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<ReconcileLaunchesRequest>,
) -> ApiResult<Json<ReconcileLaunchesPage>> {
    let actor = actor(&user)?;
    let result = Ledger::reconcile(
        &state.db,
        actor,
        &request,
        state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or_else(denied)?,
    )
    .await?;
    response(actor, result)
}
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workload-launches/reconcile", post(reconcile))
        .route("/workload-launches/{id}/resolve", post(resolve))
        .route("/workload-launches/{id}/claim", post(claim))
        .route("/workload-launches/{id}/status", post(status))
        .route("/workload-launches/{id}/stop", post(stop))
        .route("/workload-launches/{id}/create-intent", post(create_intent))
        .route("/workload-launches/{id}/start", post(start))
        .route("/workload-launches/{id}/renew", post(renew))
        .route("/workload-launches/{id}/observe", post(observe))
        .route("/workload-launches/{id}/stop-ack", post(stop_ack))
        .route(
            "/workload-launches/{id}/inputs/{pin_id}/body",
            post(input_body),
        )
        .layer(Extension(WorkloadLaunchConsumer))
}
