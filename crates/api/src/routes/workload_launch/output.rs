//! Bounded, pinned ZIP transfer. Native provider work outlives HTTP cancellation;
//! durable reservation/continuation owns any uncertain external PUT outcome.
use super::*;
use attune_common::blob_store::{BlobStoreError, ObjectKey};
use axum::{
    body::Body,
    extract::Query,
    http::header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use futures::{StreamExt, TryStreamExt};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

#[derive(utoipa::ToSchema)]
#[schema(value_type=String,format=Binary)]
pub struct CanonicalLaunchZip(pub Vec<u8>);

#[utoipa::path(operation_id="workload_launch_reserve_output",post,path="/api/v1/workload-launches/{id}/outputs",params(("id"=i64,Path),("x-attune-launch-lease"=String,Header,description="Exact target-scoped grant")),request_body=ReserveLaunchOutput,responses((status=200,body=LaunchOutputGrant),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn reserve_output(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<ReserveLaunchOutput>,
) -> ApiResult<Json<LaunchOutputGrant>> {
    handle(id, &request.handle)?;
    let actor = actor(&user)?;
    let result = Ledger::reserve_output(
        &state.db,
        actor,
        &request,
        &token(&headers, LAUNCH_LEASE_HEADER)?,
        state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or_else(denied)?,
    )
    .await?;
    fresh_deadline(result.expires_at)?;
    response(actor, result)
}
#[utoipa::path(operation_id="workload_launch_stage_result",post,path="/api/v1/workload-launches/{id}/stage-result",params(("id"=i64,Path),("x-attune-launch-dispatch"=String,Header,description="Exact target-scoped grant")),request_body=LaunchRequest,responses((status=200,body=LaunchStageResult),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn stage_result(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(request): Json<LaunchRequest>,
) -> ApiResult<Json<LaunchStageResult>> {
    key(id, &request)?;
    let actor = actor(&user)?;
    response(
        actor,
        Ledger::stage_result(
            &state.db,
            actor,
            &request,
            &token(&headers, LAUNCH_DISPATCH_HEADER)?,
        )
        .await?,
    )
}
#[utoipa::path(operation_id="workload_launch_output_body",post,path="/api/v1/workload-launches/{id}/outputs/{output_id}/body",params(("id"=i64,Path),("output_id"=i64,Path),("admitted_digest"=Sha256Digest,Query),("x-attune-launch-lease"=String,Header,description="Exact target-scoped grant"),("x-attune-launch-output"=String,Header,description="Exact target-scoped grant")),request_body(content=CanonicalLaunchZip,content_type="application/zip"),responses((status=200,body=LaunchStageResult),(status=403,description="Launch unavailable")),tag="workload-control")]
pub async fn output_body(
    State(state): State<Arc<AppState>>,
    Path((id, output_id)): Path<(i64, i64)>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<LaunchOutputBodyQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<LaunchStageResult>> {
    let actor = actor(&user)?;
    if id <= 0
        || output_id <= 0
        || headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok())
            != Some(LAUNCH_ARCHIVE_CONTENT_TYPE)
    {
        return Err(denied());
    }
    let request = LaunchRequest {
        launch_id: id,
        admitted_digest: query.admitted_digest,
    };
    let lease = token(&headers, LAUNCH_LEASE_HEADER)?;
    let grant = token(&headers, LAUNCH_OUTPUT_HEADER)?;
    let permit =
        Ledger::output_body_permit(&state.db, actor, &request, output_id, &lease, &grant).await?;
    if headers.get(CONTENT_LENGTH).is_some_and(|value| {
        value.to_str().ok().and_then(|v| v.parse::<u64>().ok()) != Some(permit.size)
    }) {
        return Err(denied());
    }
    let admission = state
        .file_transfers
        .archive_upload(permit.size, permit.policy.max_files)
        .map_err(ApiError::TooManyRequests)?;
    let mut file = tokio::fs::File::from_std(tempfile::tempfile().map_err(|_| denied())?);
    let mut incoming = body.into_data_stream();
    let mut bytes = 0u64;
    let mut digest = Sha256::new();
    while let Some(chunk) = incoming.next().await {
        let chunk = chunk.map_err(|_| denied())?;
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .filter(|n| *n <= permit.size)
            .ok_or_else(denied)?;
        // Read waits are outside a transaction. Fresh admission precedes each
        // actual byte effect, not only authentication at request start.
        Ledger::output_body_permit(&state.db, actor, &request, output_id, &lease, &grant).await?;
        digest.update(&chunk);
        file.write_all(&chunk).await.map_err(|_| denied())?;
    }
    let actual = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if bytes != permit.size || actual != permit.sha256.as_str() {
        return Err(denied());
    }
    file.sync_all().await.map_err(|_| denied())?;
    file.rewind().await.map_err(|_| denied())?;
    let mut file = file.into_std().await;
    let policy = permit.policy;
    let state_task = state.clone();
    let key = ObjectKey::new(permit.key).map_err(|_| denied())?;
    let digest: [u8; 32] = hex::decode(permit.sha256.as_str())
        .map_err(|_| denied())?
        .try_into()
        .map_err(|_| denied())?;
    // Transfer ownership before starting non-cancelable blocking validation.
    // Dropping the HTTP waiter or shutting down cannot release its admission
    // while a ZIP parser or an uncertain provider publication remains active.
    let task = state
        .file_transfers
        .finish_publication(admission, async move {
            let file = tokio::task::spawn_blocking(move || {
                attune_common::workload_archive::validate_zip(&mut file, &policy)
                    .map_err(|_| denied())?;
                Ok::<_, ApiError>(file)
            })
            .await
            .map_err(|_| denied())??;
            Ledger::output_body_permit(&state_task.db, actor, &request, output_id, &lease, &grant)
                .await?;
            let body = tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(file))
                .map_err(|error| BlobStoreError::Interrupted(error.to_string()))
                .boxed();
            let object = match state_task
                .blob_store
                .put_file_atomic(&key, body, permit.size, digest)
                .await
            {
                Ok(object) => object,
                Err(_) => state_task
                    .blob_store
                    .head(&key)
                    .await
                    .map_err(|_| denied())?
                    .ok_or_else(denied)?,
            };
            if object.size != permit.size || object.sha256 != digest {
                return Err(denied());
            }
            Ledger::publish_output(
                &state_task.db,
                actor,
                &request,
                output_id,
                &lease,
                &grant,
                &object,
            )
            .await
            .map_err(ApiError::from)
        });
    let result = task.await.map_err(|_| denied())??;
    response(actor, result)
}
