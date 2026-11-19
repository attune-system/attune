//! Source-fenced startup discovery. This retains evidence in the existing effect
//! ledger; it never impersonates an action or grants execution completion.
use super::workload_source::{self, SourceEpoch};
use crate::{
    crypto,
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    runtime_lifecycle::{RuntimeRecipe, VerifiedTool},
    workload_launch::*,
    workload_stdin::LaunchStdinPayload,
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool, Postgres, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

pub type VerifiedToolMap = BTreeMap<String, VerifiedTool>;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProbeSource {
    pub worker: i64,
    pub incarnation: Uuid,
    pub runtime: i64,
    pub runtime_version: Option<i64>,
    pub runtime_version_string: Option<String>,
    pub provider_pack: i64,
    pub release: i64,
    pub release_digest: String,
    pub runtime_config: serde_json::Value,
    pub version_config: Option<serde_json::Value>,
    pub runtime_distributions: serde_json::Value,
    pub version_distributions: Option<serde_json::Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    intent: Uuid,
    target: LaunchTarget,
    source: RuntimeProbeSource,
    image: PinnedImage,
    platform: LaunchPlatform,
    inputs: Vec<PinnedLaunchInput>,
    timeout_ms: Option<u64>,
    invocation: LaunchInvocation,
}
impl Context {
    fn spec(&self) -> ProcessSpec {
        ProcessSpec {
            intent: self.intent,
            target: self.target.clone(),
            purpose: LaunchPurpose::RuntimeProbe,
            image: self.image.clone(),
            platform: self.platform,
            inputs: self.inputs.clone(),
            timeout_ms: self.timeout_ms,
            invocation: self.invocation.clone(),
            pack_access: PackAccess::ReadOnly,
            stage: None,
        }
    }
    fn legacy(&self) -> Result<crate::runtime_detection::NativeRuntimeVerification> {
        let execution_config = self
            .source
            .version_config
            .as_ref()
            .unwrap_or(&self.source.runtime_config)
            .clone();
        if execution_config.get("lifecycle").is_some()
            || self.source.runtime_config.get("lifecycle").is_some()
        {
            return Err(denied());
        }
        Ok(crate::runtime_detection::NativeRuntimeVerification {
            registered_version: self.source.runtime_version.is_some(),
            execution_config,
            distributions: self
                .source
                .version_distributions
                .as_ref()
                .unwrap_or(&self.source.runtime_distributions)
                .clone(),
        })
    }
    fn recipe(&self) -> Result<RuntimeRecipe> {
        let config = self
            .source
            .version_config
            .as_ref()
            .unwrap_or(&self.source.runtime_config);
        if let Some(version) = &self.source.version_config {
            crate::runtime_lifecycle::validate_version_mode(&self.source.runtime_config, version)
                .map_err(|_| denied())?;
        }
        let recipe: RuntimeRecipe =
            serde_json::from_value(config.get("lifecycle").cloned().ok_or_else(denied)?)
                .map_err(|_| denied())?;
        recipe.validate().map_err(|_| denied())?;
        Ok(recipe)
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
    runtime_probe_tools: Option<Json<VerifiedToolMap>>,
    resource_removed: bool,
}
const COLUMNS:&str="intent,target,encrypted_context,source_epochs,create_submitted,stdin_consumed,stop_requested,resource,exit_outcome,runtime_probe_tools,resource_removed";
fn denied() -> Error {
    Error::PermissionDenied("Native runtime probe unavailable".into())
}
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
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 AND purpose='runtime_probe' AND target->>'kind'='runtime_probe'{}",if lock {" FOR UPDATE"}else{""}))
        .bind(intent).fetch_optional(&mut **tx).await?.ok_or_else(denied)
}
async fn fresh(tx: &mut Transaction<'_, Postgres>, deadline: DateTime<Utc>) -> Result<()> {
    if !sqlx::query_scalar::<_, bool>("SELECT clock_timestamp()<$1")
        .bind(deadline)
        .fetch_one(&mut **tx)
        .await?
    {
        return Err(denied());
    }
    Ok(())
}
async fn source(
    tx: &mut Transaction<'_, Postgres>,
    c: &Context,
) -> Result<(DateTime<Utc>, Vec<SourceEpoch>)> {
    let s = &c.source;
    let LaunchTarget::RuntimeProbe { admission, attempt } = c.target else {
        return Err(denied());
    };
    let worker:DateTime<Utc>=sqlx::query_scalar("SELECT last_heartbeat+interval '90 seconds' FROM worker WHERE id=$1 AND meta->>'registration_incarnation'=$2 AND status IN ('active','busy') FOR SHARE")
        .bind(s.worker).bind(s.incarnation.to_string()).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let pack: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pack WHERE id=$1 AND active_release=$2 FOR SHARE)",
    )
    .bind(s.provider_pack)
    .bind(s.release)
    .fetch_one(&mut **tx)
    .await?;
    if !pack {
        return Err(denied());
    }
    let release: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pack_release WHERE id=$1 AND pack=$2 AND digest=$3 FOR SHARE)",
    )
    .bind(s.release)
    .bind(s.provider_pack)
    .bind(&s.release_digest)
    .fetch_one(&mut **tx)
    .await?;
    let runtime:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime WHERE id=$1 AND pack=$2 AND execution_config=$3 AND distributions=$4 AND retired_at IS NULL FOR SHARE)")
        .bind(s.runtime).bind(s.provider_pack).bind(&s.runtime_config).bind(&s.runtime_distributions).fetch_one(&mut **tx).await?;
    if !release || !runtime {
        return Err(denied());
    }
    if let Some(version) = s.runtime_version {
        let ok:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_version WHERE id=$1 AND runtime=$2 AND execution_config=$3 AND version=$4 AND distributions=$5 AND retired_at IS NULL FOR SHARE)")
            .bind(version).bind(s.runtime).bind(s.version_config.as_ref().ok_or_else(denied)?).bind(s.runtime_version_string.as_ref().ok_or_else(denied)?).bind(s.version_distributions.as_ref().ok_or_else(denied)?).fetch_one(&mut **tx).await?;
        if !ok {
            return Err(denied());
        }
    } else if s.version_config.is_some()
        || s.runtime_version_string.is_some()
        || s.version_distributions.is_some()
    {
        return Err(denied());
    }
    let expiry:DateTime<Utc>=sqlx::query_scalar("SELECT expires_at FROM runtime_probe_admission WHERE id=$1 AND attempt=$2 AND worker=$3 AND incarnation=$4 AND runtime=$5 AND runtime_version IS NOT DISTINCT FROM $6 AND provider_pack=$7 AND release=$8 AND revoked_at IS NULL FOR SHARE")
        .bind(admission).bind(attempt).bind(s.worker).bind(s.incarnation).bind(s.runtime).bind(s.runtime_version).bind(s.provider_pack).bind(s.release).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let deadline = worker.min(expiry);
    fresh(tx, deadline).await?;
    let mut subjects = vec![
        ("worker", s.worker),
        ("runtime_definition", s.runtime),
        ("runtime_provider", s.provider_pack),
        ("runtime_probe_admission", admission),
    ];
    if let Some(version) = s.runtime_version {
        subjects.push(("runtime_version_definition", version));
    }
    let epochs = workload_source::snapshot(tx, subjects).await?;
    Ok((deadline, epochs))
}
async fn checked(
    tx: &mut Transaction<'_, Postgres>,
    intent: Uuid,
    master: &str,
) -> Result<(Record, Context, DateTime<Utc>)> {
    let old = row(tx, intent, false).await?;
    let c = old.context(master)?;
    let (deadline, epochs) = source(tx, &c).await?;
    let old = row(tx, intent, true).await?;
    if old.source_epochs.0 != epochs || old.stop_requested || old.resource_removed {
        return Err(denied());
    }
    fresh(tx, deadline).await?;
    Ok((old, c, deadline))
}
pub struct RuntimeProbeProcessRepository;
impl RuntimeProbeProcessRepository {
    /// Caller has admitted the exact recipe digest and materialized its public
    /// release and Runner pins. Attempt UUID is caller-owned across retries.
    pub async fn reserve(
        pool: &PgPool,
        attempt: Uuid,
        source: RuntimeProbeSource,
        mut spec: ProcessSpec,
        master: &str,
    ) -> Result<ProcessSpec> {
        if attempt.is_nil()
            || spec.intent.is_nil()
            || source.incarnation.is_nil()
            || spec.purpose != LaunchPurpose::RuntimeProbe
            || spec.pack_access != PackAccess::ReadOnly
            || spec.stage.is_some()
            || !spec.invocation.env.is_empty()
            || spec.invocation.working_root != LaunchRoot::Pack
            || !spec.timeout_ms.is_some_and(|ms| ms > 0 && ms <= 120000)
            || spec.inputs.len() != 2
            || spec
                .inputs
                .iter()
                .filter(|p| p.kind == LaunchInputKind::Runner)
                .count()
                != 1
            || spec
                .inputs
                .iter()
                .filter(|p| {
                    p.kind == LaunchInputKind::Pack
                        && p.object_id == source.release
                        && p.object_version == source.release_digest
                })
                .count()
                != 1
        {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        sqlx::query("INSERT INTO runtime_probe_admission(attempt,worker,incarnation,runtime,runtime_version,provider_pack,release,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,clock_timestamp()+interval '120 seconds') ON CONFLICT(attempt) DO NOTHING")
            .bind(attempt).bind(source.worker).bind(source.incarnation).bind(source.runtime).bind(source.runtime_version).bind(source.provider_pack).bind(source.release).execute(&mut *tx).await?;
        let admission: i64 =
            sqlx::query_scalar("SELECT id FROM runtime_probe_admission WHERE attempt=$1")
                .bind(attempt)
                .fetch_one(&mut *tx)
                .await?;
        spec.target = LaunchTarget::RuntimeProbe { admission, attempt };
        let c = Context {
            intent: spec.intent,
            target: spec.target.clone(),
            source,
            image: spec.image.clone(),
            platform: spec.platform,
            inputs: spec.inputs.clone(),
            timeout_ms: spec.timeout_ms,
            invocation: spec.invocation.clone(),
        };
        let definition =
            if c.invocation.argv.get(1).map(String::as_str) == Some("inspect-runtime-tools") {
                serde_json::to_value(c.recipe()?)?
            } else if c.invocation.argv.get(1).map(String::as_str)
                == Some("inspect-runtime-verification")
            {
                serde_json::to_value(c.legacy()?)?
            } else {
                return Err(denied());
            };
        if c.invocation.argv.len() < 4
            || c.invocation.argv[0] != format!("{RUNNER_ROOT}/linux-{}", c.platform.native_arch())
            || c.invocation.argv.iter().any(|a| a.contains('\0'))
            || c.invocation.argv.iter().map(String::len).sum::<usize>() > 1024 * 1024
            || serde_json::from_str::<serde_json::Value>(&c.invocation.argv[3..].concat())
                .map_err(|_| denied())?
                != definition
        {
            return Err(denied());
        }
        let (deadline, epochs) = source_check(&mut tx, &c).await?;
        let encrypted =
            crypto::encrypt(&serde_json::to_string(&c)?, master).map_err(|_| denied())?;
        sqlx::query("INSERT INTO execution_process_effect(intent,target,purpose,encrypted_context,source_epochs) VALUES($1,$2,'runtime_probe',$3,$4) ON CONFLICT(target) DO NOTHING")
            .bind(c.intent).bind(Json(&c.target)).bind(encrypted).bind(Json(&epochs)).execute(&mut *tx).await?;
        let old: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE target=$1 FOR UPDATE"
        ))
        .bind(Json(&c.target))
        .fetch_one(&mut *tx)
        .await?;
        let retained = old.context(master)?;
        let mut expected = c;
        expected.intent = retained.intent;
        if serde_json::to_value(&expected)? != serde_json::to_value(&retained)?
            || epochs != old.source_epochs.0
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
    /// Recover only this worker's retained probe intents, including previous
    /// incarnations. No row-age or provider-absence cleanup is inferred.
    pub async fn unsettled(
        pool: &PgPool,
        worker: i64,
        master: &str,
    ) -> Result<Vec<(ProcessSpec, bool, Option<ResourceIdentity>)>> {
        let records:Vec<Record>=sqlx::query_as(&format!("SELECT {} FROM execution_process_effect WHERE purpose='runtime_probe' AND target->>'kind'='runtime_probe' AND NOT resource_removed AND (target->>'admission')::BIGINT IN (SELECT id FROM runtime_probe_admission WHERE worker=$1) ORDER BY id LIMIT 16",COLUMNS))
            .bind(worker).fetch_all(pool).await?;
        records
            .into_iter()
            .map(|r| {
                Ok((
                    r.context(master)?.spec(),
                    r.create_submitted,
                    r.resource.map(|r| r.0),
                ))
            })
            .collect()
    }
    /// Retain only the exact result of a latched create/reconciliation. This
    /// records evidence even if live source authority has since been revoked.
    pub async fn record_resource(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, true).await?;
        if !old.create_submitted
            || old.resource_removed
            || old.resource.as_ref().is_some_and(|r| r.0 != *resource)
        {
            return Err(denied());
        }
        sqlx::query("UPDATE execution_process_effect SET resource=$2 WHERE intent=$1")
            .bind(intent)
            .bind(Json(resource))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn record_exit(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        outcome: LaunchExitOutcome,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, true).await?;
        if !old.create_submitted
            || old.resource.as_ref().map(|r| &r.0) != Some(resource)
            || old.exit_outcome.as_ref().is_some_and(|r| r.0 != outcome)
        {
            return Err(denied());
        }
        sqlx::query("UPDATE execution_process_effect SET exit_outcome=$2 WHERE intent=$1")
            .bind(intent)
            .bind(Json(outcome))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn request_stop(pool: &PgPool, intent: Uuid) -> Result<()> {
        let mut tx = pool.begin().await?;
        row(&mut tx, intent, true).await?;
        sqlx::query("UPDATE execution_process_effect SET stop_requested=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Caller has positively acknowledged exact provider removal. Missing,
    /// unknown and timed-out creates never grant the never-submitted branch.
    pub async fn acknowledge_removed(
        pool: &PgPool,
        intent: Uuid,
        resource: Option<&ResourceIdentity>,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, true).await?;
        let never_submitted = !old.create_submitted && old.resource.is_none() && resource.is_none();
        let removed = old.create_submitted
            && resource.is_some()
            && old.resource.as_ref().map(|r| &r.0) == resource
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
    pub async fn retained(
        pool: &PgPool,
        intent: Uuid,
        master: &str,
    ) -> Result<(ProcessSpec, Option<ResourceIdentity>)> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        Ok((old.context(master)?.spec(), old.resource.map(|r| r.0)))
    }
    pub async fn validate_source(pool: &PgPool, intent: Uuid, master: &str) -> Result<()> {
        let mut tx = pool.begin().await?;
        let (_, _, deadline) = checked(&mut tx, intent, master).await?;
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
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
        let (old, _, deadline) = checked(&mut tx, intent, master).await?;
        if old.create_submitted {
            return Ok(ProcessCreateDisposition::ReconcileOnly);
        }
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
        let (old, _, deadline) = checked(&mut tx, intent, master).await?;
        if !old.create_submitted
            || old.stdin_consumed
            || old.exit_outcome.is_some()
            || old.resource.as_ref().map(|r| &r.0) != Some(resource)
        {
            return Err(denied());
        }
        sqlx::query("UPDATE execution_process_effect SET stdin_consumed=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        LaunchStdinPayload::from_bytes(Vec::new())
    }
    /// Missing/partial capture never creates an availability receipt. The owning
    /// loop has joined both bounded streams and observed this exact exit.
    pub async fn complete(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        tools: &VerifiedToolMap,
        master: &str,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let (old, c, deadline) = checked(&mut tx, intent, master).await?;
        let recipe = if c.invocation.argv.get(1).map(String::as_str)
            == Some("inspect-runtime-verification")
        {
            c.legacy()?;
            None
        } else {
            Some(c.recipe()?)
        };
        if serde_json::to_vec(tools)?.len() > 4 * 1024 * 1024 {
            return Err(denied());
        }
        if !old.stdin_consumed
            || old.resource.as_ref().map(|r| &r.0) != Some(resource)
            || !old
                .exit_outcome
                .as_ref()
                .is_some_and(|e| e.0.exit_code == 0 && !e.0.oom_killed)
            || match &recipe {
                Some(recipe) => {
                    tools.len() != recipe.tools.len()
                        || recipe.tools.iter().any(|t| {
                            !tools.get(&t.name).is_some_and(|v| {
                                v.executable.is_absolute()
                                    && v.identity.is_absolute()
                                    && !v.version.is_empty()
                            })
                        })
                }
                None => !tools.is_empty(),
            }
            || old.runtime_probe_tools.as_ref().is_some_and(|old| {
                serde_json::to_value(&old.0).ok() != serde_json::to_value(tools).ok()
            })
        {
            return Err(denied());
        }
        sqlx::query("UPDATE execution_process_effect SET runtime_probe_tools=$2 WHERE intent=$1")
            .bind(intent)
            .bind(Json(tools))
            .execute(&mut *tx)
            .await?;
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn tools(pool: &PgPool, intent: Uuid, master: &str) -> Result<VerifiedToolMap> {
        let mut tx = pool.begin().await?;
        // Removal closes effects but does not revoke a captured availability
        // receipt. Recheck live sources/epochs even after acknowledged cleanup.
        let old = row(&mut tx, intent, false).await?;
        let c = old.context(master)?;
        let (deadline, epochs) = source(&mut tx, &c).await?;
        if old.source_epochs.0 != epochs {
            return Err(denied());
        }
        let tools = old.runtime_probe_tools.ok_or_else(denied)?.0;
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        Ok(tools)
    }
}
// Avoid shadowing the source-check function by reserve's public input name.
async fn source_check(
    tx: &mut Transaction<'_, Postgres>,
    c: &Context,
) -> Result<(DateTime<Utc>, Vec<SourceEpoch>)> {
    source(tx, c).await
}
