//! IDs-only action acquisition. The owning CP preparer/planner chooses frozen
//! input, image, runtime and command; a manager cannot submit an execution plan.
use super::*;
use attune_common::{
    repositories::workload_launch::WorkloadLaunchRepository as Ledger,
    workload_host::{AssignedAction, NextActionRequest, NextActionResponse},
    workload_launch::{LaunchIoGrant, LaunchIoGrantRequest},
};
use axum::{
    body::{Body, Bytes},
    http::header::CONTENT_TYPE,
    response::{IntoResponse, Response},
};

struct PrivateAssignmentBody {
    bytes: Vec<u8>,
    _admission: crate::file_transfers::ControlMemoryLease,
}
impl AsRef<[u8]> for PrivateAssignmentBody {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
fn assignment_response(
    actor: workload_host::HostActor,
    assignment: Option<AssignedAction>,
    admission: crate::file_transfers::ControlMemoryLease,
) -> ApiResult<Response> {
    let (mut headers, Json(value)) = response(actor.expires_at, result(assignment))?;
    let bytes = serde_json::to_vec(&value).map_err(|_| {
        ApiError::InternalServerError("Assignment serialization unavailable".into())
    })?;
    if actor.expires_at <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized("Workload credential expired".into()));
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    // Bytes ownership retains memory admission through HTTP body transmission,
    // including a slow/disconnected consumer, not just handler serialization.
    Ok((
        headers,
        Body::from(Bytes::from_owner(PrivateAssignmentBody {
            bytes,
            _admission: admission,
        })),
    )
        .into_response())
}

fn master(state: &AppState) -> ApiResult<&str> {
    state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::InternalServerError("Control-plane preparation unavailable".into())
        })
}
fn limits(state: &AppState) -> (u64, u64) {
    state
        .config
        .worker
        .as_ref()
        .map_or((10 * 1024 * 1024, 10 * 1024 * 1024), |config| {
            (
                config.max_stdout_bytes as u64,
                config.max_stderr_bytes as u64,
            )
        })
}
fn result(assignment: Option<AssignedAction>) -> NextActionResponse {
    assignment.map_or(NextActionResponse::Idle, |assignment| {
        NextActionResponse::Assigned {
            assignment: Box::new(assignment),
        }
    })
}

#[utoipa::path(operation_id="workload_host_next_action",post,path="/api/v1/workload-hosts/actions/next",request_body=NextActionRequest,responses((status=200,body=NextActionResponse),(status=403,description="Action unavailable")),tag="workload-control")]
pub async fn next(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<NextActionRequest>,
) -> ApiResult<Response> {
    let actor = workload_host::host_actor(&user.claims)?;
    let memory = state
        .file_transfers
        .private_stdin()
        .map_err(|_| ApiError::TooManyRequests("Assignment memory admission unavailable".into()))?;
    let (stdout, stderr) = limits(&state);
    if let Some(assignment) = Ledger::assigned_action(
        &state.db,
        actor,
        &request.known_executions,
        master(&state)?,
        stdout,
        stderr,
    )
    .await?
    {
        return assignment_response(actor, Some(assignment), memory);
    }
    // Deliberately retain coordinated recorded-consumer readiness: the new host
    // does not enable missing recorded completion/disclosure consumers by flag.
    let readiness = attune_common::execution_authority::RecordedConsumerReadiness::default();
    let Some(source) = WorkloadHostRepository::claim_action(
        &state.db,
        actor,
        &request.known_executions,
        readiness,
    )
    .await?
    else {
        return assignment_response(actor, None, memory);
    };
    let prepared = crate::workload_planning::prepare_action_context(
        &state,
        &source,
        source.api_origin.clone(),
        source.http_policy,
        readiness,
    )
    .await?;
    // Missing interpreted Setup/runtime publication remains a hard rejection;
    // no local setup, runtime-directory guess or mutable host plan is accepted.
    crate::workload_planning::freeze_action_launch(state.clone(), &source, prepared, None).await?;
    let assignment = Ledger::assigned_action(
        &state.db,
        actor,
        &request.known_executions,
        master(&state)?,
        stdout,
        stderr,
    )
    .await?;
    assignment_response(actor, assignment, memory)
}

#[utoipa::path(operation_id="workload_host_io_grant",post,path="/api/v1/workload-hosts/actions/io-grants",request_body=LaunchIoGrantRequest,responses((status=200,body=LaunchIoGrant),(status=403,description="Channel unavailable")),tag="workload-control")]
pub async fn io_grant(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<LaunchIoGrantRequest>,
) -> ApiResult<(HeaderMap, Json<LaunchIoGrant>)> {
    let actor = workload_host::host_actor(&user.claims)?;
    response(
        actor.expires_at,
        Ledger::host_io_grant(&state.db, actor, &request).await?,
    )
}
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workload-hosts/actions/next", post(next))
        .route("/workload-hosts/actions/io-grants", post(io_grant))
        .layer(Extension(WorkloadHostConsumer))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn private_assignment_bytes_retain_admission_until_the_body_owner_is_dropped() {
        let transfers = crate::file_transfers::FileTransfers::new(128 * 1024 * 1024, 1);
        let actor = workload_host::HostActor {
            identity: 1,
            enrollment: 1,
            profile: 1,
            generation: 0,
            worker: 1,
            registration_generation: 1,
            instance: uuid::Uuid::new_v4(),
            expires_at: chrono::Utc::now().timestamp() + 60,
        };
        let response =
            assignment_response(actor, None, transfers.private_stdin().unwrap()).unwrap();
        assert!(transfers.private_stdin().is_err());
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert!(transfers.private_stdin().is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"state":"idle"})
        );
        drop(bytes);
        assert!(transfers.private_stdin().is_ok());
        assert!(transfers.drain(std::time::Duration::from_secs(1)).await);
    }
}
