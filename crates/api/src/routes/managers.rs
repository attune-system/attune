use attune_common::{
    auth::jwt::extract_token_from_header,
    manager_control::{
        valid_manager_name, ClaimPackJob, ClaimWork, ClaimedPackJob, FinishPackJob, FinishWork,
        ManagerEnrollment, ManagerEnrollmentGrant, ManagerRuntimeCatalog, ManagerRuntimeReport,
        ManagerSession, PackJobOffer, RuntimeReportReceipt, WorkCompletion, WorkLease, WorkOffer,
    },
    models::Id,
    repositories::{
        manager_assignment::{
            ManagerAssignmentRepository, ManagerAssignmentScope, ManagerClaim, ManagerLeaseRenewal,
        },
        manager_pack_job::ManagerPackJobRepository,
        manager_runtime::ManagerRuntimeRepository,
        manager_session::{ManagerSessionRecord, ManagerSessionRepository},
    },
};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{auth::middleware::RequireAuth, middleware::error::ApiError, state::SharedState};

const MANAGER_POLL_TIMEOUT: Duration = Duration::from_secs(8);

pub(crate) fn routes() -> Router<SharedState> {
    Router::new()
        .route(
            "/internal/managers/sensors",
            get(super::manager_sensors::available),
        )
        .route(
            "/internal/managers/sensors/{sensor}/start",
            post(super::manager_sensors::start),
        )
        .route(
            "/internal/managers/sensor-workloads/{workload}/{generation}",
            post(super::manager_sensors::renew).delete(super::manager_sensors::release),
        )
        .route(
            "/internal/managers/sensor-workloads/{workload}/{generation}/process",
            post(super::manager_sensors::report_process),
        )
        .route(
            "/internal/managers/sensor-workloads/{workload}/{generation}/logs",
            post(super::manager_sensors::open_log),
        )
        .route("/internal/managers/enroll", post(enroll))
        .route("/internal/managers/session", post(renew).delete(withdraw))
        .route(
            "/internal/managers/database-credential",
            get(super::manager_database::current)
                .post(super::manager_database::request)
                .layer(axum::extract::DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/internal/managers/runtime",
            get(runtime_catalog).post(report_runtime),
        )
        .route("/internal/managers/pack-jobs", get(wait_pack_job))
        .route(
            "/internal/managers/pack-jobs/{install}/claim",
            post(claim_pack_job),
        )
        .route(
            "/internal/managers/pack-jobs/{install}/lease",
            post(renew_pack_job),
        )
        .route(
            "/internal/managers/pack-jobs/{install}/finish",
            post(finish_pack_job),
        )
        .route("/internal/managers/work", get(wait_work))
        .route(
            "/internal/managers/work/{assignment}/launch",
            post(super::manager_actions::launch),
        )
        .route(
            "/internal/managers/work/{assignment}/program",
            post(super::manager_actions::record_program),
        )
        .route(
            "/internal/managers/work/{assignment}/logs",
            post(super::manager_logs::open),
        )
        .route(
            "/internal/managers/logs/{version}/prepare",
            post(super::manager_logs::prepare),
        )
        .route(
            "/internal/managers/logs/{version}/resume",
            post(super::manager_logs::resume),
        )
        .route("/internal/managers/work/drain", post(drain_work))
        .route(
            "/internal/managers/work/{assignment}/claim",
            post(claim_work),
        )
        .route(
            "/internal/managers/work/{assignment}/lease",
            post(renew_work),
        )
        .route(
            "/internal/managers/work/{assignment}/finish",
            post(finish_work).layer(axum::extract::DefaultBodyLimit::max(128 * 1024 * 1024)),
        )
}

pub(crate) struct ManagerSessionAuthorization {
    id: Id,
    instance: Uuid,
    expires_at: DateTime<Utc>,
}

struct ManagerEnrollmentAuthorization {
    digest: String,
}

impl axum::extract::FromRequestParts<SharedState> for RequireAuth<ManagerEnrollmentAuthorization> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &SharedState) -> Result<Self, ApiError> {
        let secret = bearer(&parts.headers)?;
        if secret.len() != 64 || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_credential());
        }
        Ok(Self(ManagerEnrollmentAuthorization {
            digest: hex::encode(Sha256::digest(secret.as_bytes())),
        }))
    }
}

impl axum::extract::FromRequestParts<SharedState> for RequireAuth<ManagerSessionAuthorization> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self, ApiError> {
        let token = bearer(&parts.headers)?;
        let key = session_signing_key(state)?;
        let (message, signature) = token.rsplit_once('.').ok_or_else(invalid_credential)?;
        if message.len() > 80 || signature.len() != 64 {
            return Err(invalid_credential());
        }
        let signature = hex::decode(signature).map_err(|_| invalid_credential())?;
        let mut mac = session_mac(key);
        mac.update(message.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| invalid_credential())?;

        let (identity, expiry) = message
            .strip_prefix("am1.")
            .and_then(|message| message.rsplit_once('.'))
            .ok_or_else(invalid_credential)?;
        let (id, instance) = identity.split_once('.').ok_or_else(invalid_credential)?;
        let id = id
            .parse::<Id>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(invalid_credential)?;
        let instance = Uuid::parse_str(instance).map_err(|_| invalid_credential())?;
        let expires_at = expiry
            .parse::<i64>()
            .ok()
            .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
            .ok_or_else(invalid_credential)?;
        Ok(Self(ManagerSessionAuthorization {
            id,
            instance,
            expires_at,
        }))
    }
}

impl ManagerSessionAuthorization {
    pub(crate) fn bearer_expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    pub(crate) async fn lock(
        &self,
        state: &SharedState,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        allow_closed: bool,
    ) -> Result<ManagerSessionRecord, ApiError> {
        let record = ManagerSessionRepository::lock_for_update(
            tx,
            self.id,
            self.instance,
            self.expires_at,
            allow_closed,
        )
        .await?
        .ok_or_else(invalid_credential)?;
        if !matches_enrollment(state, record.role, &record.pool, &record.enrollment_digest) {
            return Err(invalid_credential());
        }
        Ok(record)
    }

    pub(crate) async fn live_record(
        &self,
        state: &SharedState,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<ManagerSessionRecord, ApiError> {
        let record =
            ManagerSessionRepository::lock_for_work(tx, self.id, self.instance, self.expires_at)
                .await?
                .ok_or_else(invalid_credential)?;
        if !matches_enrollment(state, record.role, &record.pool, &record.enrollment_digest) {
            return Err(invalid_credential());
        }
        Ok(record)
    }

    pub(crate) async fn continue_transfer(
        &self,
        state: &SharedState,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<ManagerSessionRecord, ApiError> {
        let record = ManagerSessionRepository::lock_for_continuation(tx, self.id, self.instance)
            .await?
            .ok_or_else(invalid_credential)?;
        if !matches_enrollment(state, record.role, &record.pool, &record.enrollment_digest) {
            return Err(invalid_credential());
        }
        Ok(record)
    }

    pub(crate) async fn work_scope(
        &self,
        state: &SharedState,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<ManagerAssignmentScope, ApiError> {
        let record = self.live_record(state, tx).await?;
        Ok(ManagerAssignmentScope {
            session: record.id,
            instance: record.instance,
            bearer_expires_at: self.expires_at,
        })
    }
}

#[derive(Deserialize)]
struct CatalogQuery {
    since: Option<i64>,
}

fn object_preparation(state: &SharedState) -> bool {
    !matches!(
        state.config.storage,
        attune_common::config::BlobStorageConfig::Filesystem { .. }
    )
}

async fn runtime_catalog(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Query(query): Query<CatalogQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let mut tx = state.db.begin().await?;
    let revision = ManagerRuntimeRepository::lock_revision(&mut tx).await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let catalog: Option<ManagerRuntimeCatalog> = if query.since == Some(revision) {
        None
    } else {
        Some(
            ManagerRuntimeRepository::catalog(&mut tx, revision, object_preparation(&state))
                .await?,
        )
    };
    ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.expires_at).await?;
    tx.commit().await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(catalog)))
}

async fn report_runtime(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Json(report): Json<ManagerRuntimeReport>,
) -> Result<Json<RuntimeReportReceipt>, ApiError> {
    let mut tx = state.db.begin().await?;
    let revision = ManagerRuntimeRepository::lock_revision(&mut tx).await?;
    let manager = auth.lock(&state, &mut tx, false).await?;
    if revision != report.revision {
        return Err(ApiError::Conflict(
            "Runtime catalog revision changed".into(),
        ));
    }
    let catalog =
        ManagerRuntimeRepository::catalog(&mut tx, revision, object_preparation(&state)).await?;
    let grant = enrollment_grant(
        &state,
        manager.role,
        &manager.pool,
        &manager.enrollment_digest,
    )
    .ok_or_else(invalid_credential)?;
    let receipt = ManagerRuntimeRepository::publish(
        &mut tx,
        &manager,
        auth.expires_at,
        &catalog,
        &report,
        grant,
    )
    .await
    .map_err(|error| match error {
        attune_common::Error::InvalidState(message) => ApiError::Conflict(message),
        other => other.into(),
    })?;
    tx.commit().await?;
    Ok(Json(receipt))
}

async fn wait_pack_job(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
) -> Result<impl IntoResponse, ApiError> {
    let mut wake = state.manager_work_wakeups.subscribe(auth.id);
    let shutdown = state.execution_log_streams.shutdown_token();
    let until = tokio::time::Instant::now() + MANAGER_POLL_TIMEOUT;
    loop {
        let mut tx = state.db.begin().await?;
        let manager = auth.live_record(&state, &mut tx).await?;
        if manager.role != attune_common::models::WorkerRole::Action {
            return Err(ApiError::Forbidden(
                "Pack jobs require an action manager".into(),
            ));
        }
        let offer = ManagerPackJobRepository::next_offer(&mut tx, &manager).await?;
        ManagerRuntimeRepository::check_deadline(&mut tx, &manager, auth.expires_at).await?;
        tx.commit().await?;
        if offer.is_some() || tokio::time::Instant::now() >= until {
            return Ok(([(header::CACHE_CONTROL, "no-store")], Json(offer)));
        }
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(([(header::CACHE_CONTROL, "no-store")], Json(None::<PackJobOffer>))),
            _ = wake.wait(until.saturating_duration_since(tokio::time::Instant::now())) => {},
        }
    }
}

async fn claim_pack_job(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(install): Path<Id>,
    Json(request): Json<ClaimPackJob>,
) -> Result<impl IntoResponse, ApiError> {
    let mut tx = state.db.begin().await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let mut mac = Hmac::<Sha256>::new_from_slice(session_signing_key(&state)?)
        .expect("HMAC accepts any key size");
    mac.update(b"attune:manager-candidate-effect:v1\0");
    mac.update(&manager.id.to_be_bytes());
    mac.update(&install.to_be_bytes());
    mac.update(&request.attempt.to_be_bytes());
    mac.update(request.request.as_bytes());
    let signature = mac.finalize().into_bytes();
    let lease_digest = Sha256::digest(signature);
    let lease = Uuid::from_bytes(lease_digest[..16].try_into().expect("16-byte lease"));
    let token = hex::encode(signature);
    let job: ClaimedPackJob = ManagerPackJobRepository::claim(
        &mut tx,
        &manager,
        auth.expires_at,
        install,
        request,
        (lease, &token),
    )
    .await?;
    tx.commit().await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(job)))
}

async fn renew_pack_job(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(install): Path<Id>,
    headers: HeaderMap,
) -> Result<Json<Option<DateTime<Utc>>>, ApiError> {
    let claim = pack_job_claim(&headers)?;
    let mut tx = state.db.begin().await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    let deadline =
        ManagerPackJobRepository::renew(&mut tx, &manager, auth.expires_at, install, claim).await?;
    tx.commit().await?;
    Ok(Json(deadline))
}

async fn finish_pack_job(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(install): Path<Id>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let claim = pack_job_claim(&headers)?;
    let mut tx = state.db.begin().await?;
    let manager = auth.live_record(&state, &mut tx).await?;
    ManagerPackJobRepository::authorize_finish(&mut tx, &manager, auth.expires_at, install, claim)
        .await?;
    tx.commit().await?;
    // A single runner can produce 1 MiB each of stdout and stderr. The normal
    // Axum JSON limit would discard even that result after JSON encoding.
    let body_limit =
        attune_common::config::PackUploadConfig::DEFAULT_MAX_EXTRACTED_SIZE_BYTES as usize;
    let result = to_bytes(body, body_limit).await.map_err(|_| {
        ApiError::PayloadTooLarge("Pack job result could not be read within the size limit".into())
    })?;
    let Json(result) = Json::<FinishPackJob>::from_bytes(&result)
        .map_err(|_| ApiError::BadRequest("Invalid pack job result".into()))?;
    let mut tx = state.db.begin().await?;
    let manager = auth.continue_transfer(&state, &mut tx).await?;
    ManagerPackJobRepository::finish(
        &mut tx,
        &manager,
        manager.expires_at,
        install,
        claim,
        result,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn pack_job_claim(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    headers
        .get(attune_common::manager_control::PACK_JOB_CLAIM_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|claim| !claim.is_nil())
        .ok_or_else(|| ApiError::Forbidden("Pack job claim is required".into()))
}

async fn wait_work(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
) -> Result<impl IntoResponse, ApiError> {
    let mut wake = state.manager_work_wakeups.subscribe(auth.id);
    let shutdown = state.execution_log_streams.shutdown_token();
    let deadline = tokio::time::Instant::now() + MANAGER_POLL_TIMEOUT;
    loop {
        let mut tx = state.db.begin().await?;
        let scope = auth.work_scope(&state, &mut tx).await?;
        let offer = ManagerAssignmentRepository::next_offer(&mut tx, scope).await?;
        tx.commit().await?;
        if offer.is_some() || tokio::time::Instant::now() >= deadline {
            return Ok((
                [(header::CACHE_CONTROL, "no-store")],
                Json(offer.map(WorkOffer::from)),
            ));
        }
        tokio::select! {
            _ = shutdown.cancelled() => {
                return Ok(([(header::CACHE_CONTROL, "no-store")], Json(None)));
            }
            _ = wake.wait(deadline.saturating_duration_since(tokio::time::Instant::now())) => {}
        }
    }
}

async fn claim_work(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(assignment): Path<Id>,
    Json(request): Json<ClaimWork>,
) -> Result<impl IntoResponse, ApiError> {
    let mut tx = state.db.begin().await?;
    let scope = auth.work_scope(&state, &mut tx).await?;
    let claim =
        ManagerAssignmentRepository::claim(&mut tx, scope, assignment, request.request).await?;
    let (status, row) = match claim {
        ManagerClaim::Started(row) => (StatusCode::CREATED, row),
        ManagerClaim::Replay(row) => (StatusCode::OK, row),
    };
    tx.commit().await?;
    Ok((
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(WorkOffer::from(row)),
    ))
}

#[derive(Deserialize)]
struct LeaseWait {
    #[serde(default)]
    wait: bool,
}

async fn renew_work(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(assignment): Path<Id>,
    Query(wait): Query<LeaseWait>,
) -> Result<Json<WorkLease>, ApiError> {
    let mut wake = wait
        .wait
        .then(|| state.manager_work_wakeups.subscribe(auth.id));
    let first = renew_assignment(&state, &auth, assignment).await?;
    if let Some(wake) = &mut wake {
        if matches!(first, WorkLease::Running { .. }) {
            let shutdown = state.execution_log_streams.shutdown_token();
            tokio::select! {
                _ = shutdown.cancelled() => return Ok(Json(first)),
                _ = wake.wait(MANAGER_POLL_TIMEOUT) => {}
            }
            return Ok(Json(renew_assignment(&state, &auth, assignment).await?));
        }
    }
    Ok(Json(first))
}

async fn renew_assignment(
    state: &SharedState,
    auth: &ManagerSessionAuthorization,
    assignment: Id,
) -> Result<WorkLease, ApiError> {
    let mut tx = state.db.begin().await?;
    let scope = auth.work_scope(state, &mut tx).await?;
    let state = match ManagerAssignmentRepository::renew(&mut tx, scope, assignment).await? {
        ManagerLeaseRenewal::Extended(deadline) => WorkLease::Running { deadline },
        ManagerLeaseRenewal::Cancel => WorkLease::Cancel,
    };
    tx.commit().await?;
    Ok(state)
}

async fn drain_work(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    let scope = auth.work_scope(&state, &mut tx).await?;
    ManagerAssignmentRepository::drain(&mut tx, scope).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn finish_work(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
    Path(assignment): Path<Id>,
    Json(request): Json<FinishWork>,
) -> Result<Json<WorkCompletion>, ApiError> {
    let key = state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::InternalServerError("Manager result encryption unavailable".into())
        })?;
    let mut fingerprint = Hmac::<Sha256>::new_from_slice(session_signing_key(&state)?)
        .expect("HMAC accepts any key size");
    fingerprint.update(b"attune:manager-finish:v1\0");
    fingerprint.update(&assignment.to_be_bytes());
    fingerprint.update(auth.instance.as_bytes());
    fingerprint.update(
        &serde_json::to_vec(&request).map_err(|_| ApiError::BadRequest("Invalid result".into()))?,
    );
    let digest = hex::encode(fingerprint.finalize().into_bytes());
    let mut tx = state.db.begin().await?;
    let scope = auth.work_scope(&state, &mut tx).await?;
    let status = ManagerAssignmentRepository::finish(
        &mut tx,
        scope,
        assignment,
        &digest,
        request.status,
        serde_json::Value::Object(request.result),
        key,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(WorkCompletion { status }))
}

async fn enroll(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerEnrollmentAuthorization>,
    Json(enrollment): Json<ManagerEnrollment>,
) -> Result<impl IntoResponse, ApiError> {
    if !valid_manager_name(&enrollment.pool)
        || !valid_manager_name(&enrollment.name)
        || enrollment.instance.is_nil()
    {
        return Err(ApiError::BadRequest(
            "Invalid manager scope or instance".into(),
        ));
    }
    let key = session_signing_key(&state)?;
    if !matches_enrollment(&state, enrollment.role, &enrollment.pool, &auth.digest) {
        return Err(invalid_credential());
    }
    let record = ManagerSessionRepository::enroll(&state.db, &enrollment, &auth.digest)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(
                "Manager slot already has a live instance or this instance was retired".into(),
            )
        })?;
    Ok(session_response(key, record))
}

async fn renew(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
) -> Result<impl IntoResponse, ApiError> {
    let key = session_signing_key(&state)?;
    let mut tx = state.db.begin().await?;
    let mut record = auth.lock(&state, &mut tx, false).await?;
    record.expires_at = ManagerSessionRepository::renew(&mut tx, record.id, auth.expires_at)
        .await?
        .ok_or_else(invalid_credential)?;
    ManagerRuntimeRepository::heartbeat(&mut tx, &record).await?;
    tx.commit().await?;
    Ok(session_response(key, record))
}

async fn withdraw(
    State(state): State<SharedState>,
    RequireAuth(auth): RequireAuth<ManagerSessionAuthorization>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    let record = auth.lock(&state, &mut tx, true).await?;
    if record.closed_at.is_none()
        && !ManagerSessionRepository::withdraw(&mut tx, record.id, auth.expires_at).await?
    {
        return Err(invalid_credential());
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn matches_enrollment(
    state: &SharedState,
    role: attune_common::models::WorkerRole,
    pool: &str,
    digest: &str,
) -> bool {
    enrollment_grant(state, role, pool, digest).is_some()
}

fn enrollment_grant<'a>(
    state: &'a SharedState,
    role: attune_common::models::WorkerRole,
    pool: &str,
    digest: &str,
) -> Option<&'a ManagerEnrollmentGrant> {
    state.config.manager_enrollments.iter().find(|grant| {
        grant.role == role
            && grant.pool == pool
            && bool::from(grant.secret_sha256.as_bytes().ct_eq(digest.as_bytes()))
    })
}

fn session_response(key: &[u8], record: ManagerSessionRecord) -> impl IntoResponse {
    let message = format!(
        "am1.{}.{}.{}",
        record.id,
        record.instance,
        record.expires_at.timestamp()
    );
    let mut mac = session_mac(key);
    mac.update(message.as_bytes());
    let token = format!("{message}.{}", hex::encode(mac.finalize().into_bytes()));
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(ManagerSession {
            id: record.id,
            instance: record.instance,
            expires_at: record.expires_at,
            token,
        }),
    )
}

fn session_mac(key: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(b"attune:manager-session:v1\0");
    mac
}

fn session_signing_key(state: &SharedState) -> Result<&[u8], ApiError> {
    state
        .config
        .security
        .jwt_secret
        .as_deref()
        .filter(|key| !state.config.manager_enrollments.is_empty() && key.trim().len() >= 32)
        .map(str::as_bytes)
        .ok_or_else(invalid_credential)
}

fn bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_token_from_header)
        .ok_or_else(invalid_credential)
}

fn invalid_credential() -> ApiError {
    ApiError::Unauthorized("Invalid workload manager credential".into())
}
