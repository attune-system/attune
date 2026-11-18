//! Trusted enrollment/lease control for execution-only hosts. Configuration and
//! capabilities come from immutable CP profiles, never registration payloads.
use crate::{
    auth::jwt::JwtConfig,
    models::{WorkerRole, WorkerType},
    workload_host::*,
    workload_launch::{LaunchEngineBinding, Sha256Digest},
    workload_session::WorkloadSession,
    Error, Result,
};
use chrono::{DateTime, Utc};
use sqlx::{types::Json, PgPool, Postgres, Transaction};

mod action;
pub use action::ClaimedActionSource;

const ENROLLMENT_COLUMNS:&str="id,profile,identity,worker_name,generation,expires_at,revoked_at,worker,instance,custody_instance,registration_generation";
#[derive(sqlx::FromRow)]
struct Enrollment {
    id: i64,
    profile: i64,
    identity: i64,
    worker_name: String,
    generation: i64,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    worker: Option<i64>,
    instance: Option<uuid::Uuid>,
    custody_instance: Option<uuid::Uuid>,
    registration_generation: i64,
}
#[derive(sqlx::FromRow)]
struct Profile {
    id: i64,
    image: i64,
    role: String,
    capabilities: Json<serde_json::Value>,
    max_concurrent: i32,
    disabled: bool,
}
pub struct WorkloadHostRepository;
fn denied() -> Error {
    Error::PermissionDenied("Workload host registration unavailable".into())
}
impl WorkloadHostRepository {
    async fn now(tx: &mut Transaction<'_, Postgres>) -> Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **tx)
            .await?)
    }
    async fn enrollment(
        tx: &mut Transaction<'_, Postgres>,
        actor: EnrollmentActor,
    ) -> Result<(Profile, Enrollment)> {
        let profile:Profile=sqlx::query_as("SELECT id,image,role,capabilities,max_concurrent,disabled FROM workload_host_profile WHERE id=$1 FOR SHARE")
            .bind(actor.profile).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        let row: Enrollment = sqlx::query_as(&format!(
            "SELECT {ENROLLMENT_COLUMNS} FROM workload_host_enrollment WHERE id=$1 FOR UPDATE"
        ))
        .bind(actor.enrollment)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        let identity: Option<i64> =
            sqlx::query_scalar("SELECT id FROM identity WHERE id=$1 AND NOT frozen FOR SHARE")
                .bind(actor.identity)
                .fetch_optional(&mut **tx)
                .await?;
        let now = Self::now(tx).await?;
        if profile.disabled
            || row.profile != profile.id
            || row.identity != actor.identity
            || row.generation != actor.generation
            || row.revoked_at.is_some()
            || row.expires_at <= now
            || actor.expires_at <= now.timestamp()
            || identity.is_none()
        {
            return Err(denied());
        }
        Ok((profile, row))
    }
    async fn profile(
        tx: &mut Transaction<'_, Postgres>,
        profile: &Profile,
    ) -> Result<HostExecutionProfile> {
        let (engine_id,daemon_id,platform,reference,sha256,controller_origin):(i64,String,String,String,String,String)=sqlx::query_as(
            "SELECT e.id,e.daemon_id,e.platform,i.reference,i.sha256,e.controller_url \
             FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine \
             WHERE i.id=$1 AND i.revoked_at IS NULL AND NOT e.disabled AND i.platform=e.platform FOR SHARE OF i,e")
            .bind(profile.image).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        let engine = LaunchEngineBinding {
            engine_id,
            daemon_id,
            platform: serde_json::from_value(serde_json::Value::String(platform))?,
        };
        engine.validate_shape()?;
        Ok(HostExecutionProfile {
            role: serde_json::from_value(serde_json::Value::String(profile.role.clone()))?,
            engine,
            image: crate::workload_launch::PinnedImage {
                reference,
                sha256: Sha256Digest::parse(sha256)?,
            },
            controller_origin,
            max_concurrent: profile.max_concurrent.try_into().map_err(|_| denied())?,
        })
    }
    fn session(actor: HostActor, jwt: &JwtConfig) -> Result<WorkloadSession> {
        Ok(WorkloadSession {
            token: mint_host(actor, jwt)?,
            expires_at: DateTime::from_timestamp(actor.expires_at, 0).ok_or_else(denied)?,
        })
    }
    pub async fn register(
        pool: &PgPool,
        actor: EnrollmentActor,
        request: &RegisterWorkloadHost,
        jwt: &JwtConfig,
    ) -> Result<RegisteredWorkloadHost> {
        if request.instance.is_nil() || request.custody_instance.is_nil() {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        let (profile, enrollment) = Self::enrollment(&mut tx, actor).await?;
        let role = match serde_json::from_value(serde_json::Value::String(profile.role.clone()))? {
            WorkloadHostRole::Action => WorkerRole::Action,
            WorkloadHostRole::Sensor => WorkerRole::Sensor,
        };
        let mut capabilities = profile.capabilities.0.clone();
        let map = capabilities.as_object_mut().ok_or_else(denied)?;
        map.insert(
            "max_concurrent_executions".into(),
            serde_json::json!(profile.max_concurrent),
        );
        map.insert(
            "log_delivery_instance".into(),
            serde_json::json!(request.custody_instance),
        );
        let registration_generation = if enrollment.instance == Some(request.instance) {
            if enrollment.custody_instance != Some(request.custody_instance) {
                return Err(denied());
            }
            let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worker WHERE id=$1 AND status IN ('active','busy') AND last_heartbeat>clock_timestamp()-interval '90 seconds')")
                .bind(enrollment.worker.ok_or_else(denied)?).fetch_one(&mut *tx).await?;
            if !current {
                return Err(denied());
            }
            enrollment.registration_generation
        } else {
            enrollment
                .registration_generation
                .checked_add(1)
                .ok_or_else(denied)?
        };
        let metadata = serde_json::json!({"registration_incarnation":request.instance,"workload_host_enrollment":enrollment.id,"workload_host_registration_generation":registration_generation});
        let worker = if let Some(worker) = enrollment.worker {
            // Re-registration owns the same logical slot; the existing worker
            // trigger permanently fences the replaced incarnation and custody.
            sqlx::query_scalar::<_,i64>("UPDATE worker SET status='active',capabilities=$2,meta=COALESCE(meta,'{}'::jsonb)||$3,last_heartbeat=clock_timestamp(),updated=clock_timestamp() WHERE id=$1 AND name=$4 AND worker_role=$5 RETURNING id")
                .bind(worker).bind(Json(capabilities)).bind(Json(metadata)).bind(&enrollment.worker_name).bind(role)
                .fetch_optional(&mut *tx).await?.ok_or_else(denied)?
        } else {
            sqlx::query_scalar::<_,i64>("INSERT INTO worker(name,worker_type,worker_role,status,capabilities,meta,last_heartbeat) VALUES($1,$2,$3,'active',$4,$5,clock_timestamp()) RETURNING id")
                .bind(&enrollment.worker_name).bind(WorkerType::Container).bind(role).bind(Json(capabilities)).bind(Json(metadata)).fetch_one(&mut *tx).await?
        };
        sqlx::query("UPDATE workload_host_enrollment SET worker=$2,instance=$3,custody_instance=$4,registration_generation=$5 WHERE id=$1")
            .bind(enrollment.id).bind(worker).bind(request.instance).bind(request.custody_instance).bind(registration_generation).execute(&mut *tx).await?;
        let description = Self::profile(&mut tx, &profile).await?;
        let now = Self::now(&mut tx).await?;
        if actor.expires_at <= now.timestamp() || enrollment.expires_at <= now {
            return Err(denied());
        }
        let host = HostActor {
            identity: actor.identity,
            enrollment: enrollment.id,
            profile: enrollment.profile,
            generation: enrollment.generation,
            worker,
            registration_generation,
            instance: request.instance,
            expires_at: (now.timestamp() + 300)
                .min(actor.expires_at)
                .min(enrollment.expires_at.timestamp()),
        };
        let result = RegisteredWorkloadHost {
            worker,
            registration_generation,
            instance: request.instance,
            session: Self::session(host, jwt)?,
            profile: description,
        };
        if actor.expires_at <= Self::now(&mut tx).await?.timestamp() {
            return Err(denied());
        }
        tx.commit().await?;
        if actor.expires_at <= Utc::now().timestamp() {
            return Err(denied());
        }
        Ok(result)
    }
    /// Source admission for every host control effect. The lease cannot be
    /// renewed after expiry; recovery needs a fresh registration incarnation.
    pub async fn admit(
        tx: &mut Transaction<'_, Postgres>,
        actor: HostActor,
    ) -> Result<DateTime<Utc>> {
        let (_, enrollment) = Self::enrollment(
            tx,
            EnrollmentActor {
                identity: actor.identity,
                enrollment: actor.enrollment,
                profile: actor.profile,
                generation: actor.generation,
                expires_at: actor.expires_at,
            },
        )
        .await?;
        if enrollment.worker != Some(actor.worker)
            || enrollment.instance != Some(actor.instance)
            || enrollment.registration_generation != actor.registration_generation
            || enrollment.custody_instance.is_none()
        {
            return Err(denied());
        }
        let heartbeat:Option<DateTime<Utc>>=sqlx::query_scalar("SELECT last_heartbeat FROM worker WHERE id=$1 AND status IN ('active','busy') AND meta->>'registration_incarnation'=$2 AND meta->>'workload_host_enrollment'=$3 FOR SHARE")
            .bind(actor.worker).bind(actor.instance.to_string()).bind(actor.enrollment.to_string()).fetch_optional(&mut **tx).await?.flatten();
        let deadline = (heartbeat.ok_or_else(denied)? + chrono::Duration::seconds(90))
            .min(enrollment.expires_at);
        if deadline <= Self::now(tx).await? {
            return Err(denied());
        }
        Ok(deadline)
    }
    pub async fn renew(
        pool: &PgPool,
        actor: HostActor,
        jwt: &JwtConfig,
    ) -> Result<WorkloadSession> {
        let mut tx = pool.begin().await?;
        let deadline = Self::admit(&mut tx, actor).await?;
        sqlx::query("UPDATE worker SET last_heartbeat=clock_timestamp(),updated=clock_timestamp() WHERE id=$1")
            .bind(actor.worker).execute(&mut *tx).await?;
        let now = Self::now(&mut tx).await?;
        if deadline <= now || actor.expires_at <= now.timestamp() {
            return Err(denied());
        }
        let expires: DateTime<Utc> =
            sqlx::query_scalar("SELECT expires_at FROM workload_host_enrollment WHERE id=$1")
                .bind(actor.enrollment)
                .fetch_one(&mut *tx)
                .await?;
        let next = HostActor {
            expires_at: (now.timestamp() + 300).min(expires.timestamp()),
            ..actor
        };
        let result = Self::session(next, jwt)?;
        if actor.expires_at <= Self::now(&mut tx).await?.timestamp() {
            return Err(denied());
        }
        tx.commit().await?;
        if actor.expires_at <= Utc::now().timestamp() {
            return Err(denied());
        }
        Ok(result)
    }
}
