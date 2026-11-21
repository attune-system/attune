use attune_common::{
    auth::jwt::extract_token_from_header,
    manager_control::{valid_manager_name, ManagerEnrollment, ManagerSession},
    models::Id,
    repositories::manager_session::{ManagerSessionRecord, ManagerSessionRepository},
};
use axum::{
    extract::State,
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{auth::middleware::RequireAuth, middleware::error::ApiError, state::SharedState};

pub(crate) fn routes() -> Router<SharedState> {
    Router::new()
        .route("/internal/managers/enroll", post(enroll))
        .route("/internal/managers/session", post(renew).delete(withdraw))
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
    state.config.manager_enrollments.iter().any(|grant| {
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
