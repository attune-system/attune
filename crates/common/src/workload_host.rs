//! Machine identity for execution-only managers. Enrollment selects a persisted
//! CP profile; hosts cannot nominate an image, engine, owner, runtime or argv.
use crate::{
    auth::jwt::{Claims, JwtConfig, TokenType},
    workload_launch::{
        LaunchEngineBinding, LaunchIoGrant, LaunchToken, PinnedImage, ReservedLaunch,
    },
    workload_session::WorkloadSession,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub const HOST_SCOPE: &str = "workload_host";
pub const ENROLLMENT_SCOPE: &str = "workload_host_enrollment";
#[derive(Clone, Copy, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadHostRole {
    Action,
    Sensor,
}
#[derive(Clone, Copy)]
pub struct HostActor {
    pub identity: i64,
    pub enrollment: i64,
    pub profile: i64,
    pub generation: i64,
    pub worker: i64,
    pub registration_generation: i64,
    pub instance: Uuid,
    pub expires_at: i64,
}
#[derive(Clone, Copy)]
pub struct EnrollmentActor {
    pub identity: i64,
    pub enrollment: i64,
    pub profile: i64,
    pub generation: i64,
    pub expires_at: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostBinding {
    enrollment: i64,
    profile: i64,
    generation: i64,
    worker: i64,
    registration_generation: i64,
    instance: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentBinding {
    enrollment: i64,
    profile: i64,
    generation: i64,
}
fn denied() -> Error {
    Error::PermissionDenied("Workload host credential unavailable".into())
}
fn identity(claims: &Claims, scope: &str) -> Result<i64> {
    let identity = claims.sub.parse::<i64>().map_err(|_| denied())?;
    if claims.token_type != TokenType::Worker
        || claims.scope.as_deref() != Some(scope)
        || identity <= 0
        || claims.sub != identity.to_string()
        || claims.exp <= chrono::Utc::now().timestamp()
    {
        return Err(denied());
    }
    Ok(identity)
}
pub fn host_actor(claims: &Claims) -> Result<HostActor> {
    let identity = identity(claims, HOST_SCOPE)?;
    let b: HostBinding = serde_json::from_value(claims.metadata.clone().ok_or_else(denied)?)
        .map_err(|_| denied())?;
    if b.enrollment <= 0
        || b.profile <= 0
        || b.worker <= 0
        || b.generation < 0
        || b.registration_generation <= 0
        || b.instance.is_nil()
    {
        return Err(denied());
    }
    Ok(HostActor {
        identity,
        enrollment: b.enrollment,
        profile: b.profile,
        generation: b.generation,
        worker: b.worker,
        registration_generation: b.registration_generation,
        instance: b.instance,
        expires_at: claims.exp,
    })
}
pub fn enrollment_actor(claims: &Claims) -> Result<EnrollmentActor> {
    let identity = identity(claims, ENROLLMENT_SCOPE)?;
    let b: EnrollmentBinding = serde_json::from_value(claims.metadata.clone().ok_or_else(denied)?)
        .map_err(|_| denied())?;
    if b.enrollment <= 0 || b.profile <= 0 || b.generation < 0 {
        return Err(denied());
    }
    Ok(EnrollmentActor {
        identity,
        enrollment: b.enrollment,
        profile: b.profile,
        generation: b.generation,
        expires_at: claims.exp,
    })
}
fn mint(
    identity: i64,
    scope: &str,
    metadata: serde_json::Value,
    expires_at: i64,
    max_seconds: i64,
    jwt: &JwtConfig,
) -> Result<LaunchToken> {
    let now = chrono::Utc::now().timestamp();
    if identity <= 0 || expires_at <= now || expires_at > now + max_seconds {
        return Err(denied());
    }
    let claims = Claims {
        sub: identity.to_string(),
        login: "workload-host".into(),
        iat: now,
        exp: expires_at,
        token_type: TokenType::Worker,
        scope: Some(scope.into()),
        metadata: Some(metadata),
    };
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(jwt.secret.as_bytes()),
    )
    .map_err(|_| denied())?;
    LaunchToken::new(token)
}
/// Called by the CP only after persisted enrollment/worker checks under locks.
pub fn mint_host(actor: HostActor, jwt: &JwtConfig) -> Result<LaunchToken> {
    if actor.enrollment <= 0
        || actor.profile <= 0
        || actor.worker <= 0
        || actor.generation < 0
        || actor.registration_generation <= 0
        || actor.instance.is_nil()
    {
        return Err(denied());
    }
    mint(
        actor.identity,
        HOST_SCOPE,
        serde_json::to_value(HostBinding {
            enrollment: actor.enrollment,
            profile: actor.profile,
            generation: actor.generation,
            worker: actor.worker,
            registration_generation: actor.registration_generation,
            instance: actor.instance,
        })?,
        actor.expires_at,
        300,
        jwt,
    )
}
/// Enrollment credentials remain outside pack PID/filesystem/invocation scope.
/// Rotation/revocation is persisted by enrollment generation, never IAM lookup.
pub fn mint_enrollment(actor: EnrollmentActor, jwt: &JwtConfig) -> Result<LaunchToken> {
    if actor.enrollment <= 0 || actor.profile <= 0 || actor.generation < 0 {
        return Err(denied());
    }
    mint(
        actor.identity,
        ENROLLMENT_SCOPE,
        serde_json::to_value(EnrollmentBinding {
            enrollment: actor.enrollment,
            profile: actor.profile,
            generation: actor.generation,
        })?,
        actor.expires_at,
        30 * 24 * 3600,
        jwt,
    )
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterWorkloadHost {
    pub instance: Uuid,
    pub custody_instance: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HostExecutionProfile {
    pub role: WorkloadHostRole,
    pub engine: LaunchEngineBinding,
    pub image: PinnedImage,
    pub controller_origin: String,
    pub max_concurrent: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisteredWorkloadHost {
    pub worker: i64,
    pub registration_generation: i64,
    pub instance: Uuid,
    pub session: WorkloadSession,
    pub profile: HostExecutionProfile,
}
/// A dedicated response carrying selected input to the non-pack custodian.
/// Values are excluded from Debug, audit events, and public execution DTOs.
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignedAction {
    pub execution: i64,
    pub launch: ReservedLaunch,
    pub controller_origin: String,
    pub stdin_json: String,
    pub stdin_grant: LaunchIoGrant,
    pub stdout_grant: LaunchIoGrant,
    pub stderr_grant: LaunchIoGrant,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
}
impl std::fmt::Debug for AssignedAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssignedAction")
            .field("execution", &self.execution)
            .field("launch", &self.launch)
            .finish_non_exhaustive()
    }
}
