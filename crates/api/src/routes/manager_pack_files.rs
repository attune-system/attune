//! Session scope for the existing pinned pack transfer endpoints.
use attune_common::{
    auth::jwt::extract_token_from_header,
    manager_control::{PACK_JOB_HEADER, SENSOR_GENERATION_HEADER, WORK_SCOPE_HEADER},
    models::{Id, PackInstall, SensorWorkloadFence, WorkerRole},
    repositories::{
        manager_assignment::{ManagerAssignmentRepository, ManagerAssignmentScope},
        manager_pack_job::ManagerPackJobRepository,
        manager_runtime::ManagerRuntimeRepository,
        runtime_preparation::RuntimePreparationRepository,
        sensor_schema::SensorSchemaRepository,
        SensorWorkloadRepository, WorkerRepository,
    },
    runtime_lifecycle::{InstalledPreparation, PreparationWorkload},
};
use axum::extract::FromRequestParts;
use axum::http::{header, request::Parts, HeaderMap};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    middleware::ApiError,
    routes::managers::{pack_job_claim, ManagerSessionAuthorization},
    state::SharedState,
};

pub(crate) enum PackTransferAuthorization {
    Existing(Box<AuthenticatedUser>),
    Manager(ManagerSessionAuthorization),
}

impl FromRequestParts<SharedState> for RequireAuth<PackTransferAuthorization> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, Response> {
        let is_manager = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(extract_token_from_header)
            .is_some_and(|token| token.starts_with("am1."));
        Ok(Self(if is_manager {
            let RequireAuth(auth) =
                RequireAuth::<ManagerSessionAuthorization>::from_request_parts(parts, state)
                    .await
                    .map_err(IntoResponse::into_response)?;
            PackTransferAuthorization::Manager(auth)
        } else {
            let RequireAuth(user) =
                RequireAuth::<AuthenticatedUser>::from_request_parts(parts, state)
                    .await
                    .map_err(IntoResponse::into_response)?;
            PackTransferAuthorization::Existing(Box::new(user))
        }))
    }
}

pub(crate) async fn candidate(
    state: &SharedState,
    auth: &ManagerSessionAuthorization,
    headers: &HeaderMap,
    install: Id,
) -> Result<PackInstall, ApiError> {
    if id_header(headers, PACK_JOB_HEADER)? != install {
        return Err(denied());
    }
    let claim = pack_job_claim(headers)?;
    let mut tx = state.db.begin().await?;
    let manager = auth.live_record(state, &mut tx).await?;
    let (install, _) = ManagerPackJobRepository::authorize_archive(
        &mut tx,
        &manager,
        auth.bearer_expires_at(),
        install,
        claim,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(install)
}

pub(crate) struct PreparedUpload<'a> {
    auth: &'a ManagerSessionAuthorization,
    install: Id,
    claim: Uuid,
}

impl PreparedUpload<'_> {
    pub(crate) async fn finish(
        &self,
        state: &SharedState,
        prepared: &InstalledPreparation,
        digest: &str,
    ) -> Result<(), ApiError> {
        let mut tx = state.db.begin().await?;
        let manager = self.auth.continue_transfer(state, &mut tx).await?;
        ManagerPackJobRepository::prepared_upload(
            &mut tx,
            &manager,
            manager.expires_at,
            self.install,
            self.claim,
            Some((prepared, digest)),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

pub(crate) async fn prepared_upload<'a>(
    state: &SharedState,
    auth: &'a ManagerSessionAuthorization,
    headers: &HeaderMap,
    install: Id,
) -> Result<(PreparedUpload<'a>, Option<String>), ApiError> {
    if id_header(headers, PACK_JOB_HEADER)? != install {
        return Err(denied());
    }
    let claim = pack_job_claim(headers)?;
    let mut tx = state.db.begin().await?;
    let manager = auth.live_record(state, &mut tx).await?;
    let receipt = ManagerPackJobRepository::prepared_upload(
        &mut tx,
        &manager,
        auth.bearer_expires_at(),
        install,
        claim,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok((
        PreparedUpload {
            auth,
            install,
            claim,
        },
        receipt,
    ))
}

pub(crate) async fn release(
    state: &SharedState,
    auth: &ManagerSessionAuthorization,
    headers: &HeaderMap,
    release: Id,
    directory: Option<Uuid>,
) -> Result<(String, Option<InstalledPreparation>), ApiError> {
    let mut tx = state.db.begin().await?;
    let manager = auth.live_record(state, &mut tx).await?;
    if headers.contains_key(PACK_JOB_HEADER) {
        if directory.is_some()
            || headers.contains_key(WORK_SCOPE_HEADER)
            || headers.contains_key(SENSOR_GENERATION_HEADER)
        {
            return Err(denied());
        }
        let install = id_header(headers, PACK_JOB_HEADER)?;
        let (_, digest) = ManagerPackJobRepository::authorize_archive(
            &mut tx,
            &manager,
            auth.bearer_expires_at(),
            install,
            pack_job_claim(headers)?,
            Some(release),
        )
        .await?;
        tx.commit().await?;
        return Ok((digest.ok_or_else(denied)?, None));
    }
    let work = id_header(headers, WORK_SCOPE_HEADER)?;
    let worker = ManagerRuntimeRepository::lock_worker(&mut tx, &manager).await?;
    let (digest, kind, reference) = match manager.role {
        WorkerRole::Action => {
            if headers.contains_key(SENSOR_GENERATION_HEADER) {
                return Err(denied());
            }
            let scope = ManagerAssignmentScope {
                session: manager.id,
                instance: manager.instance,
                bearer_expires_at: auth.bearer_expires_at(),
            };
            let execution =
                ManagerAssignmentRepository::execution_for_transfer(&mut tx, scope, work).await?;
            if execution.pack_release != Some(release) {
                return Err(denied());
            }
            (
                execution.pack_release_digest.ok_or_else(denied)?,
                PreparationWorkload::Action,
                execution.action_ref,
            )
        }
        WorkerRole::Sensor => {
            let generation = id_header(headers, SENSOR_GENERATION_HEADER)?;
            let workload = SensorWorkloadRepository::find_by_id(&mut *tx, work)
                .await?
                .ok_or_else(denied)?;
            let fence = SensorWorkloadFence {
                workload_id: work,
                worker_id: worker,
                worker_instance: manager.instance,
                generation,
            };
            SensorWorkloadRepository::lock_current_fence_workload(&mut tx, workload.sensor, fence)
                .await?
                .ok_or_else(denied)?;
            let (_, binding, _) = SensorSchemaRepository::find(&mut *tx, work, generation)
                .await?
                .ok_or_else(denied)?;
            if binding.executable.release.id != release {
                return Err(denied());
            }
            SensorWorkloadRepository::recheck_locked_fence(&mut tx, workload.sensor, fence).await?;
            (
                binding.executable.release.digest,
                PreparationWorkload::Sensor,
                binding.executable.sensor.r#ref,
            )
        }
    };
    let prepared = if let Some(directory) = directory {
        let current = WorkerRepository::find_registration_for_share(&mut tx, worker)
            .await?
            .ok_or_else(denied)?;
        Some(
            RuntimePreparationRepository::find_download_with(
                &mut *tx, &current, release, directory,
            )
            .await?
            .filter(|ready| {
                ready.release_digest == digest
                    && ready.target.kind == kind
                    && ready.target.reference == reference
            })
            .ok_or_else(denied)?,
        )
    } else {
        None
    };
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.bearer_expires_at()).await?;
    tx.commit().await?;
    Ok((digest, prepared))
}

fn id_header(headers: &HeaderMap, name: &'static str) -> Result<Id, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|id| id.parse().ok())
        .filter(|id| *id > 0)
        .ok_or_else(denied)
}

fn denied() -> ApiError {
    ApiError::Forbidden("Pack transfer is outside the manager's live work scope".into())
}
