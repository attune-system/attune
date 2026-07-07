use std::{collections::HashMap, sync::Arc};

use attune_common::{
    models::WorkerStatus,
    rbac::{Action, AuthorizationContext, Resource},
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;

use attune_common::repositories::{
    execution::{ExecutionRepository, WorkerExecutionLoad},
    runtime::WorkerRepository,
    FindById, List,
};

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::PaginatedResponse, worker::runtime_support_from_capabilities, CordonWorkerRequest,
        WorkerHealthState, WorkerLoadSnapshot, WorkerQueryParams, WorkerSummary,
    },
    middleware::ApiResult,
    state::AppState,
};

const HEARTBEAT_INTERVAL_SECS: i64 = 30;
const HEARTBEAT_STALENESS_MULTIPLIER: i64 = 3;

fn capability_u64(capabilities: Option<&serde_json::Value>, key: &str) -> Option<u64> {
    capabilities
        .and_then(|capabilities| capabilities.get(key))
        .and_then(|value| value.as_u64())
}

fn capability_u32(capabilities: Option<&serde_json::Value>, key: &str) -> Option<u32> {
    capability_u64(capabilities, key).and_then(|value| u32::try_from(value).ok())
}

fn heartbeat_health(last_heartbeat: Option<chrono::DateTime<Utc>>) -> (Option<i64>, bool) {
    let Some(last_heartbeat) = last_heartbeat else {
        return (None, true);
    };
    let age_seconds = Utc::now()
        .signed_duration_since(last_heartbeat)
        .num_seconds()
        .max(0);
    let stale = age_seconds > HEARTBEAT_INTERVAL_SECS * HEARTBEAT_STALENESS_MULTIPLIER;
    (Some(age_seconds), stale)
}

fn derive_worker_status(
    status: Option<WorkerStatus>,
    total_active: u64,
    sensor_processes_running: Option<u64>,
    active_rules: Option<u64>,
) -> Option<WorkerStatus> {
    match status {
        Some(WorkerStatus::Inactive | WorkerStatus::Error | WorkerStatus::Busy) => status,
        Some(WorkerStatus::Active)
            if total_active > 0
                || sensor_processes_running.unwrap_or(0) > 0
                || active_rules.unwrap_or(0) > 0 =>
        {
            Some(WorkerStatus::Busy)
        }
        _ => status,
    }
}

fn derive_health_state(
    status: Option<WorkerStatus>,
    cordoned: bool,
    heartbeat_stale: bool,
    total_active: u64,
    sensor_processes_running: Option<u64>,
    active_rules: Option<u64>,
) -> WorkerHealthState {
    if cordoned {
        return WorkerHealthState::Cordoned;
    }

    match status {
        Some(WorkerStatus::Error) => WorkerHealthState::Error,
        Some(WorkerStatus::Inactive) => WorkerHealthState::Offline,
        Some(WorkerStatus::Active | WorkerStatus::Busy) if heartbeat_stale => {
            WorkerHealthState::Offline
        }
        Some(WorkerStatus::Busy) => WorkerHealthState::Busy,
        Some(WorkerStatus::Active)
            if total_active > 0
                || sensor_processes_running.unwrap_or(0) > 0
                || active_rules.unwrap_or(0) > 0 =>
        {
            WorkerHealthState::Busy
        }
        Some(WorkerStatus::Active) => WorkerHealthState::Active,
        None => WorkerHealthState::Inactive,
    }
}

fn summarize_worker(
    worker: attune_common::models::Worker,
    load: Option<&WorkerExecutionLoad>,
    include_sensitive_metadata: bool,
) -> WorkerSummary {
    let capabilities = worker.capabilities.as_ref();
    let max_concurrent_executions = capability_u32(capabilities, "max_concurrent_executions");
    let max_concurrent_sensors = capability_u32(capabilities, "max_concurrent_sensors");
    let sensor_processes_monitored = capability_u64(capabilities, "sensor_processes_monitored");
    let sensor_processes_running = capability_u64(capabilities, "sensor_processes_running");
    let active_rules = capability_u64(capabilities, "active_rules");
    let queue_depth = worker
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.get("health"))
        .and_then(|health| health.get("queue_depth"))
        .and_then(|value| value.as_i64())
        .and_then(|value| i32::try_from(value).ok());
    let total_active = load
        .map(|value| value.total_active.max(0) as u64)
        .unwrap_or(0);
    let utilization_percent = max_concurrent_executions
        .filter(|value| *value > 0)
        .map(|max| ((total_active as f64 / max as f64) * 100.0).round() as u32)
        .or_else(|| {
            max_concurrent_sensors
                .filter(|value| *value > 0)
                .zip(sensor_processes_running)
                .map(|(max, running)| ((running as f64 / max as f64) * 100.0).round() as u32)
        });
    let status = derive_worker_status(
        worker.status,
        total_active,
        sensor_processes_running,
        active_rules,
    );
    let (heartbeat_age_seconds, heartbeat_stale) = heartbeat_health(worker.last_heartbeat);
    let health_state = derive_health_state(
        worker.status,
        worker.cordoned,
        heartbeat_stale,
        total_active,
        sensor_processes_running,
        active_rules,
    );

    WorkerSummary {
        id: worker.id,
        name: worker.name,
        worker_type: worker.worker_type,
        worker_role: worker.worker_role,
        host: include_sensitive_metadata.then_some(worker.host).flatten(),
        port: include_sensitive_metadata.then_some(worker.port).flatten(),
        status,
        last_heartbeat: worker.last_heartbeat,
        heartbeat_age_seconds,
        heartbeat_stale,
        cordoned: worker.cordoned,
        cordon_reason: include_sensitive_metadata
            .then_some(worker.cordon_reason)
            .flatten(),
        cordoned_by: include_sensitive_metadata
            .then_some(worker.cordoned_by)
            .flatten(),
        cordoned_at: include_sensitive_metadata
            .then_some(worker.cordoned_at)
            .flatten(),
        health_state,
        supported_runtimes: runtime_support_from_capabilities(worker.capabilities.as_ref()),
        load: WorkerLoadSnapshot {
            requested: load.map(|value| value.requested.max(0) as u64).unwrap_or(0),
            scheduling: load
                .map(|value| value.scheduling.max(0) as u64)
                .unwrap_or(0),
            scheduled: load.map(|value| value.scheduled.max(0) as u64).unwrap_or(0),
            running: load.map(|value| value.running.max(0) as u64).unwrap_or(0),
            canceling: load.map(|value| value.canceling.max(0) as u64).unwrap_or(0),
            total_active,
            max_concurrent_executions,
            utilization_percent,
            queue_depth,
            sensor_processes_monitored,
            sensor_processes_running,
            active_rules,
            max_concurrent_sensors,
        },
        created: worker.created,
        updated: worker.updated,
    }
}

async fn has_workers_manage_access(
    state: &AppState,
    user: &AuthenticatedUser,
    identity_id: i64,
) -> ApiResult<bool> {
    let grants = AuthorizationService::new(state.db.clone())
        .effective_grants(user)
        .await?;
    Ok(AuthorizationService::is_allowed(
        &grants,
        Resource::Workers,
        Action::Manage,
        &AuthorizationContext::new(identity_id),
    ))
}

async fn authorize_workers_manage(state: &AppState, user: &AuthenticatedUser) -> ApiResult<i64> {
    let identity_id = user.identity_id().map_err(|_| {
        crate::middleware::ApiError::Unauthorized("Invalid user identity".to_string())
    })?;
    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Workers,
                action: Action::Manage,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await?;
    Ok(identity_id)
}

#[utoipa::path(
    get,
    path = "/api/v1/workers",
    tag = "workers",
    params(WorkerQueryParams),
    responses(
        (status = 200, description = "List workers with runtime support and current load", body = PaginatedResponse<WorkerSummary>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_workers(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<WorkerQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let identity_id = user.identity_id().map_err(|_| {
        crate::middleware::ApiError::Unauthorized("Invalid user identity".to_string())
    })?;
    AuthorizationService::new(state.db.clone())
        .authorize(
            &user,
            AuthorizationCheck {
                resource: Resource::Workers,
                action: Action::Read,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await?;
    let include_sensitive_metadata = has_workers_manage_access(&state, &user, identity_id).await?;

    let workers = WorkerRepository::list(&state.db).await?;
    let worker_ids = workers.iter().map(|worker| worker.id).collect::<Vec<_>>();
    let load_by_worker = ExecutionRepository::current_load_by_worker_ids(&state.db, &worker_ids)
        .await?
        .into_iter()
        .map(|load| (load.worker_id, load))
        .collect::<HashMap<_, _>>();

    let filtered = workers
        .into_iter()
        .map(|worker| {
            let load = load_by_worker.get(&worker.id);
            summarize_worker(worker, load, include_sensitive_metadata)
        })
        .filter(|summary| query.role.is_none_or(|role| summary.worker_role == role))
        .filter(|summary| {
            query
                .status
                .is_none_or(|status| summary.status == Some(status))
        })
        .filter(|summary| {
            query
                .cordoned
                .is_none_or(|cordoned| summary.cordoned == cordoned)
        })
        .filter(|summary| {
            query
                .health_state
                .is_none_or(|health_state| summary.health_state == health_state)
        })
        .collect::<Vec<_>>();

    let total = filtered.len() as u64;
    let start = query.offset() as usize;
    let end = (start + query.limit() as usize).min(filtered.len());
    let summaries = if start >= filtered.len() {
        Vec::new()
    } else {
        filtered[start..end].to_vec()
    };

    Ok((
        StatusCode::OK,
        Json(PaginatedResponse::new(
            summaries,
            &query.pagination(),
            total,
        )),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/workers/{id}",
    tag = "workers",
    params(("id" = i64, Path, description = "Worker ID")),
    responses(
        (status = 200, description = "Worker with runtime support and current load", body = WorkerSummary),
        (status = 404, description = "Worker not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_worker(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(worker_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let identity_id = user.identity_id().map_err(|_| {
        crate::middleware::ApiError::Unauthorized("Invalid user identity".to_string())
    })?;
    AuthorizationService::new(state.db.clone())
        .authorize(
            &user,
            AuthorizationCheck {
                resource: Resource::Workers,
                action: Action::Read,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await?;
    let include_sensitive_metadata = has_workers_manage_access(&state, &user, identity_id).await?;

    let worker = WorkerRepository::find_by_id(&state.db, worker_id)
        .await?
        .ok_or_else(|| {
            crate::middleware::ApiError::NotFound(format!("Worker with ID {} not found", worker_id))
        })?;
    let load = ExecutionRepository::current_load_by_worker_ids(&state.db, &[worker.id])
        .await?
        .into_iter()
        .next();

    Ok((
        StatusCode::OK,
        Json(summarize_worker(
            worker,
            load.as_ref(),
            include_sensitive_metadata,
        )),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/workers/{id}/cordon",
    tag = "workers",
    params(("id" = i64, Path, description = "Worker ID")),
    request_body = CordonWorkerRequest,
    responses((status = 200, description = "Worker cordoned", body = WorkerSummary)),
    security(("bearer_auth" = []))
)]
pub async fn cordon_worker(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(worker_id): Path<i64>,
    Json(payload): Json<CordonWorkerRequest>,
) -> ApiResult<impl IntoResponse> {
    let identity_id = authorize_workers_manage(&state, &user).await?;
    let reason = payload
        .reason
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty());

    let worker =
        WorkerRepository::set_cordoned(&state.db, worker_id, true, reason, Some(identity_id))
            .await?;
    let load = ExecutionRepository::current_load_by_worker_ids(&state.db, &[worker.id])
        .await?
        .into_iter()
        .next();

    Ok((
        StatusCode::OK,
        Json(summarize_worker(worker, load.as_ref(), true)),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/workers/{id}/uncordon",
    tag = "workers",
    params(("id" = i64, Path, description = "Worker ID")),
    responses((status = 200, description = "Worker uncordoned", body = WorkerSummary)),
    security(("bearer_auth" = []))
)]
pub async fn uncordon_worker(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(worker_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_workers_manage(&state, &user).await?;
    let worker = WorkerRepository::set_cordoned(&state.db, worker_id, false, None, None).await?;
    let load = ExecutionRepository::current_load_by_worker_ids(&state.db, &[worker.id])
        .await?
        .into_iter()
        .next();

    Ok((
        StatusCode::OK,
        Json(summarize_worker(worker, load.as_ref(), true)),
    ))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workers", get(list_workers))
        .route("/workers/{id}", get(get_worker))
        .route("/workers/{id}/cordon", post(cordon_worker))
        .route("/workers/{id}/uncordon", post(uncordon_worker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_routes_structure() {
        let _router = routes();
    }
}
