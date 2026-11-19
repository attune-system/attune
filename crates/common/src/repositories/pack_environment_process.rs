//! Native Setup receipts reuse the coordinated environment claim and the
//! existing effect ledger. No exit or retained archive is Ready authority.
use super::workload_source::{self, SourceEpoch};
use crate::{
    crypto,
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    pack_environment::{CoordinatedPackEnvironment, PackEnvironmentManager},
    workload_launch::*,
    workload_stdin::LaunchStdinPayload,
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool, Postgres, Transaction};
use uuid::Uuid;

fn denied() -> Error {
    Error::PermissionDenied("Native environment setup unavailable".into())
}
// Deliberately private encrypted projection, never Debug/audit/API material.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    intent: Uuid,
    target: LaunchTarget,
    image: PinnedImage,
    platform: LaunchPlatform,
    inputs: Vec<PinnedLaunchInput>,
    stage: PrivateStage,
    timeout_ms: Option<u64>,
    invocation: LaunchInvocation,
    worker: i64,
    instance: Uuid,
    env_key: String,
    env_path: String,
    checksum: Option<String>,
    binding: String,
    plan: String,
}
impl Context {
    fn spec(&self) -> ProcessSpec {
        ProcessSpec {
            intent: self.intent,
            purpose: LaunchPurpose::Setup,
            target: self.target.clone(),
            image: self.image.clone(),
            platform: self.platform,
            inputs: self.inputs.clone(),
            pack_access: PackAccess::ReadOnly,
            stage: Some(self.stage.clone()),
            timeout_ms: self.timeout_ms,
            invocation: self.invocation.clone(),
        }
    }
}
#[derive(sqlx::FromRow)]
struct Record {
    intent: Uuid,
    target: Json<LaunchTarget>,
    encrypted_context: String,
    source_epochs: Json<Vec<SourceEpoch>>,
    create_submitted: bool,
    stdin_consumed: bool,
    stop_requested: bool,
    resource: Option<Json<ResourceIdentity>>,
    exit_outcome: Option<Json<LaunchExitOutcome>>,
    published_runtime: Option<Json<PinnedLaunchInput>>,
}
const COLUMNS:&str="intent,target,encrypted_context,source_epochs,create_submitted,stdin_consumed,stop_requested,resource,exit_outcome,published_runtime";
impl Record {
    fn context(&self, master: &str) -> Result<Context> {
        let c: Context = serde_json::from_str(
            &crypto::decrypt(&self.encrypted_context, master).map_err(|_| denied())?,
        )
        .map_err(|_| denied())?;
        if c.intent != self.intent || c.target != self.target.0 {
            return Err(denied());
        }
        Ok(c)
    }
}
async fn row(tx: &mut Transaction<'_, Postgres>, intent: Uuid, lock: bool) -> Result<Record> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 AND purpose='setup'{}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(intent)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(denied)
}
async fn fresh(tx: &mut Transaction<'_, Postgres>, deadline: DateTime<Utc>) -> Result<()> {
    let ok: bool = sqlx::query_scalar("SELECT clock_timestamp()<$1")
        .bind(deadline)
        .fetch_one(&mut **tx)
        .await?;
    if !ok {
        return Err(denied());
    }
    Ok(())
}
async fn source(tx: &mut Transaction<'_, Postgres>, c: &Context) -> Result<DateTime<Utc>> {
    let LaunchTarget::Setup { admission, attempt } = c.target else {
        return Err(denied());
    };
    // Same source -> epoch -> operation order as revocation. Never retain an
    // operation lock while waiting for the worker or mutable claim source.
    let worker:DateTime<Utc>=sqlx::query_scalar("SELECT last_heartbeat+interval '90 seconds' FROM worker WHERE id=$1 AND meta->>'registration_incarnation'=$2 AND status IN ('active','busy') FOR SHARE")
        .bind(c.worker).bind(c.instance.to_string()).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let lease:DateTime<Utc>=sqlx::query_scalar("SELECT claim_expires_at FROM pack_environment WHERE id=$1 AND env_key=$2 AND env_path=$3 AND manifest_checksum IS NOT DISTINCT FROM $4 AND native_binding=$5 AND status='installing' AND claimed_by_worker=$6 AND claim_id=$7 FOR UPDATE")
        .bind(admission).bind(&c.env_key).bind(&c.env_path).bind(&c.checksum).bind(&c.binding).bind(c.worker).bind(attempt).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let deadline = worker.min(lease);
    fresh(tx, deadline).await?;
    Ok(deadline)
}
pub struct PackEnvironmentProcessRepository;
impl PackEnvironmentProcessRepository {
    /// The caller owns recipe admission and passes only the fixed helper setup
    /// plan. Exact private bytes are encrypted, never included in a public hash.
    pub async fn reserve(
        pool: &PgPool,
        environment: &CoordinatedPackEnvironment,
        worker: i64,
        instance: Uuid,
        spec: &ProcessSpec,
        plan: &str,
        master: &str,
    ) -> Result<ProcessSpec> {
        let claim = environment.claim_id.ok_or_else(denied)?;
        let binding = environment.native_binding.clone().ok_or_else(denied)?;
        if spec.intent.is_nil()
            || spec.purpose != LaunchPurpose::Setup
            || spec.target
                != (LaunchTarget::Setup {
                    admission: environment.id,
                    attempt: claim,
                })
            || spec.pack_access != PackAccess::ReadOnly
            || spec.invocation.argv
                != [
                    format!("{RUNNER_ROOT}/linux-{}", spec.platform.native_arch()),
                    "setup".into(),
                ]
            || !spec.invocation.env.is_empty()
            || spec.invocation.working_root != LaunchRoot::Pack
            || plan.len() > 1024 * 1024
        {
            return Err(denied());
        }
        let c = Context {
            intent: spec.intent,
            target: spec.target.clone(),
            image: spec.image.clone(),
            platform: spec.platform,
            inputs: spec.inputs.clone(),
            stage: spec.stage.clone().ok_or_else(denied)?,
            timeout_ms: spec.timeout_ms,
            invocation: spec.invocation.clone(),
            worker,
            instance,
            env_key: environment.env_key.clone(),
            env_path: environment.env_path.clone(),
            checksum: environment.manifest_checksum.clone(),
            binding,
            plan: plan.into(),
        };
        let serialized = serde_json::to_string(&c).map_err(|_| denied())?;
        let encrypted = crypto::encrypt(&serialized, master).map_err(|_| denied())?;
        let mut tx = pool.begin().await?;
        let deadline = source(&mut tx, &c).await?;
        let epochs = workload_source::snapshot(&mut tx, vec![("worker", worker)]).await?;
        // One target/claim owns one intent even when COMMIT acknowledgement is
        // lost. Re-read that retained intent rather than mint a second create.
        sqlx::query("INSERT INTO execution_process_effect(intent,target,purpose,encrypted_context,source_epochs,setup_binding) VALUES($1,$2,'setup',$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(spec.intent).bind(Json(&spec.target)).bind(encrypted).bind(Json(&epochs)).bind(&c.binding).execute(&mut *tx).await?;
        let old:Record=sqlx::query_as(&format!("SELECT {COLUMNS} FROM execution_process_effect WHERE target=$1 AND purpose='setup' FOR UPDATE" )).bind(Json(&spec.target)).fetch_one(&mut *tx).await?;
        let retained = old.context(master)?;
        // Context and resource identity remain immutable through claim takeover.
        // An exact retry may recover the earlier random intent, never change it.
        let mut expected = c;
        expected.intent = retained.intent;
        if serde_json::to_value(&retained).map_err(|_| denied())?
            != serde_json::to_value(&expected).map_err(|_| denied())?
            || old.source_epochs.0 != epochs
            || old.stop_requested
        {
            return Err(denied());
        }
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        Ok(retained.spec())
    }
    /// Bounded old-claim cleanup from retained evidence, never row age. The
    /// caller must settle this batch before permitting replacement preparation.
    pub async fn unsettled(
        pool: &PgPool,
        environment: i64,
        except: Option<Uuid>,
        master: &str,
    ) -> Result<Vec<(ProcessSpec, bool)>> {
        let rows:Vec<Record>=sqlx::query_as(&format!("SELECT {COLUMNS} FROM execution_process_effect WHERE purpose='setup' AND (target->>'admission')::BIGINT=$1 AND NOT resource_removed AND ($2::TEXT IS NULL OR target->>'attempt'<>$2) ORDER BY id LIMIT 16"))
            .bind(environment).bind(except.map(|v|v.to_string())).fetch_all(pool).await?;
        rows.into_iter()
            .map(|old| Ok((old.context(master)?.spec(), old.create_submitted)))
            .collect()
    }
    pub async fn removal_recorded(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_process_effect WHERE intent=$1 AND purpose='setup' AND resource=$2 AND resource_removed AND stop_requested)")
            .bind(intent).bind(Json(resource)).fetch_one(pool).await?)
    }
    /// Only an acknowledged exact native removal, or a durable never-submitted
    /// tombstone, can settle byte cleanup. No provider error is absence proof.
    pub async fn acknowledge_removed(
        pool: &PgPool,
        intent: Uuid,
        resource: Option<&ResourceIdentity>,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, true).await?;
        let never_submitted = !old.create_submitted && old.resource.is_none() && resource.is_none();
        let removed = old.create_submitted
            && old.resource.as_ref().map(|r| &r.0) == resource
            && resource.is_some()
            && old.exit_outcome.is_some();
        if !old.stop_requested || !(never_submitted || removed) {
            return Err(denied());
        }
        sqlx::query("UPDATE execution_process_effect SET resource_removed=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Recover cleanup from immutable historical publication, independently of
    /// the mutable claim pointer. This never supplies new start/stdin authority.
    pub async fn retained_output(
        pool: &PgPool,
        pin: &PinnedLaunchInput,
        master: &str,
    ) -> Result<(ProcessSpec, ResourceIdentity)> {
        let intent = Uuid::parse_str(&pin.object_version).map_err(|_| denied())?;
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        if old.published_runtime.as_ref().map(|p| &p.0) != Some(pin) {
            return Err(denied());
        }
        let spec = old.context(master)?.spec();
        let resource = old.resource.ok_or_else(denied)?.0;
        Ok((spec, resource))
    }
    pub async fn published(pool: &PgPool, binding: &str) -> Result<Option<PinnedLaunchInput>> {
        let pin:Option<Json<PinnedLaunchInput>>=sqlx::query_scalar("SELECT published_runtime FROM execution_process_effect WHERE purpose='setup' AND setup_binding=$1 AND published_runtime IS NOT NULL ORDER BY id DESC LIMIT 1")
            .bind(binding).fetch_optional(pool).await?;
        Ok(pin.map(|p| p.0))
    }
    pub async fn validate_source(pool: &PgPool, intent: Uuid, master: &str) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        let c = old.context(master)?;
        let deadline = source(&mut tx, &c).await?;
        let epochs = workload_source::snapshot(&mut tx, vec![("worker", c.worker)]).await?;
        if epochs != old.source_epochs.0 || old.stop_requested {
            return Err(denied());
        }
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn begin_create(
        pool: &PgPool,
        intent: Uuid,
        master: &str,
    ) -> Result<ProcessCreateDisposition> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        if old.create_submitted || old.stop_requested {
            return Ok(ProcessCreateDisposition::ReconcileOnly);
        }
        let c = old.context(master)?;
        let deadline = source(&mut tx, &c).await?;
        let epochs = workload_source::snapshot(&mut tx, vec![("worker", c.worker)]).await?;
        let old = row(&mut tx, intent, true).await?;
        if old.create_submitted || old.stop_requested {
            return Ok(ProcessCreateDisposition::ReconcileOnly);
        }
        if old.source_epochs.0 != epochs {
            return Err(denied());
        }
        fresh(&mut tx, deadline).await?;
        sqlx::query("UPDATE execution_process_effect SET create_submitted=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        Ok(ProcessCreateDisposition::CreateOnce)
    }
    pub async fn consume_stdin(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        master: &str,
    ) -> Result<LaunchStdinPayload> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        let c = old.context(master)?;
        let deadline = source(&mut tx, &c).await?;
        let epochs = workload_source::snapshot(&mut tx, vec![("worker", c.worker)]).await?;
        let old = row(&mut tx, intent, true).await?;
        if old.source_epochs.0 != epochs
            || !old.create_submitted
            || old.stdin_consumed
            || old.stop_requested
            || old.exit_outcome.is_some()
            || old.resource.as_ref().map(|r| &r.0) != Some(resource)
        {
            return Err(denied());
        }
        fresh(&mut tx, deadline).await?;
        sqlx::query("UPDATE execution_process_effect SET stdin_consumed=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        LaunchStdinPayload::from_bytes(c.plan.into_bytes())
    }
    /// The exact validated immutable ZIP was installed durably before entry.
    /// Ready and its output pin commit together under current claim authority.
    pub async fn publish(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        pin: &PinnedLaunchInput,
        master: &str,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let snapshot = row(&mut tx, intent, false).await?;
        let c = snapshot.context(master)?;
        if let Some(old) = &snapshot.published_runtime {
            return if old.0 == *pin && snapshot.resource.as_ref().map(|r| &r.0) == Some(resource) {
                Ok(())
            } else {
                Err(denied())
            };
        }
        let deadline = source(&mut tx, &c).await?;
        let epochs = workload_source::snapshot(&mut tx, vec![("worker", c.worker)]).await?;
        let old = row(&mut tx, intent, true).await?;
        let LaunchTarget::Setup { admission, attempt } = c.target else {
            return Err(denied());
        };
        if old.source_epochs.0 != epochs
            || !old.stdin_consumed
            || old.stop_requested
            || old.resource.as_ref().map(|r| &r.0) != Some(resource)
            || old.exit_outcome.as_ref().map(|v| v.0)
                != Some(LaunchExitOutcome {
                    exit_code: 0,
                    oom_killed: false,
                })
            || pin.kind != LaunchInputKind::Runtime
            || pin.object_id != admission
            || pin.object_version != intent.to_string()
            || c.inputs.iter().any(|p| p.pin_id == pin.pin_id)
        {
            return Err(denied());
        }
        fresh(&mut tx, deadline).await?;
        sqlx::query("UPDATE execution_process_effect SET published_runtime=$2 WHERE intent=$1")
            .bind(intent)
            .bind(Json(pin))
            .execute(&mut *tx)
            .await?;
        if !PackEnvironmentManager::mark_coordinated_environment_ready_on(
            &mut tx,
            &c.env_key,
            c.worker,
            attempt,
            c.checksum.as_deref(),
        )
        .await?
        {
            return Err(denied());
        }
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        Ok(())
    }
}
