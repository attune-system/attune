//! Native candidate effects are fenced by the existing install, download
//! credential, optional candidate API lease and worker incarnation. The ledger
//! retains uncertainty and encrypted capture; it does not schedule an install.
use super::workload_source::{self, SourceEpoch};
use crate::{
    crypto,
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    workload_launch::*,
    workload_stdin::LaunchStdinPayload,
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool, Postgres, Transaction};
use uuid::Uuid;
fn denied() -> Error {
    Error::PermissionDenied("Native candidate effects unavailable or incomplete".into())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSource {
    epochs: Vec<SourceEpoch>,
    pub install: i64,
    pub worker: i64,
    pub instance: Uuid,
    pub pack_id: Option<i64>,
    pub pack_ref: String,
    pub pack_version: String,
    pub archive_key: String,
    pub archive_version: String,
    pub archive_digest: String,
    pub archive_size: i64,
    pub archive_token_hash: String,
    pub effects: Option<CandidateEffectsSource>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateEffectsSource {
    pub lease: Uuid,
    pub token_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    source: CandidateSource,
    intent: Uuid,
    target: LaunchTarget,
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
            purpose: LaunchPurpose::CandidateTest,
            target: self.target.clone(),
            image: self.image.clone(),
            platform: self.platform,
            inputs: self.inputs.clone(),
            pack_access: PackAccess::CandidateCopy,
            stage: None,
            timeout_ms: self.timeout_ms,
            invocation: self.invocation.clone(),
        }
    }
}
#[derive(sqlx::FromRow)]
struct Record {
    id: i64,
    intent: Uuid,
    target: Json<LaunchTarget>,
    encrypted_context: String,
    source_epochs: Json<Vec<SourceEpoch>>,
    create_submitted: bool,
    stdin_consumed: bool,
    stop_requested: bool,
    resource: Option<Json<ResourceIdentity>>,
    exit_outcome: Option<Json<LaunchExitOutcome>>,
    resource_removed: bool,
    candidate_capture: Option<String>,
    created: DateTime<Utc>,
}
const COLUMNS:&str="id,intent,target,encrypted_context,source_epochs,create_submitted,stdin_consumed,stop_requested,resource,exit_outcome,resource_removed,candidate_capture,created";
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
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 AND purpose='candidate_test'{}",if lock{" FOR UPDATE"}else{""})).bind(intent).fetch_optional(&mut **tx).await?.ok_or_else(denied)
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
async fn source(tx: &mut Transaction<'_, Postgres>, s: &CandidateSource) -> Result<DateTime<Utc>> {
    let worker:DateTime<Utc>=sqlx::query_scalar("SELECT last_heartbeat+interval '90 seconds' FROM worker WHERE id=$1 AND meta->>'registration_incarnation'=$2 AND status IN ('active','busy') FOR SHARE").bind(s.worker).bind(s.instance.to_string()).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let install:DateTime<Utc>=sqlx::query_scalar("SELECT started_at+interval '7 hours' FROM pack_install WHERE id=$1 AND assigned_worker_id=$2 AND status='running' AND pack_ref=$3 AND pack_version=$4 AND candidate_object_key=$5 AND candidate_provider_version=$6 AND candidate_digest=$7 AND candidate_archive_size=$8 AND candidate_access_token_hash=$9 AND pack_id IS NOT DISTINCT FROM $10 FOR UPDATE").bind(s.install).bind(s.worker).bind(&s.pack_ref).bind(&s.pack_version).bind(&s.archive_key).bind(&s.archive_version).bind(&s.archive_digest).bind(s.archive_size).bind(&s.archive_token_hash).bind(s.pack_id).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    let mut deadline = worker.min(install);
    if let Some(e) = &s.effects {
        let expiry:DateTime<Utc>=sqlx::query_scalar("SELECT expires_at FROM pack_candidate_context WHERE lease=$1 AND install_id=$2 AND worker_id=$3 AND runtime_token_hash=$4 AND archive_key=$5 AND archive_version=$6 AND archive_digest=$7 AND archive_token_hash=$8 AND NOT revoked FOR SHARE").bind(e.lease).bind(s.install).bind(s.worker).bind(&e.token_hash).bind(&s.archive_key).bind(&s.archive_version).bind(&s.archive_digest).bind(&s.archive_token_hash).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        deadline = deadline.min(expiry);
    }
    fresh(tx, deadline).await?;
    Ok(deadline)
}
async fn epochs(
    tx: &mut Transaction<'_, Postgres>,
    s: &CandidateSource,
) -> Result<Vec<SourceEpoch>> {
    let current =
        workload_source::snapshot(tx, vec![("worker", s.worker), ("install", s.install)]).await?;
    if current != s.epochs {
        return Err(denied());
    }
    Ok(current)
}
#[derive(Clone, Serialize, Deserialize)]
pub struct CandidateCapture {
    pub interruption: Option<crate::test_executor::TestProcessInterruption>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}
/// Retain the caller's SQLx transaction through lease closure, result/terminal
/// writes and COMMIT. This deadline is not a transferable execution authority.
pub struct CandidateResultFence {
    deadline: DateTime<Utc>,
}
impl CandidateResultFence {
    pub async fn ensure_fresh(&self, tx: &mut Transaction<'_, Postgres>) -> Result<()> {
        fresh(tx, self.deadline).await
    }
}
pub struct CandidateProcessRepository;
impl CandidateProcessRepository {
    pub async fn admit(
        pool: &PgPool,
        install: i64,
        worker: i64,
        instance: Uuid,
        archive_token: &str,
    ) -> Result<CandidateSource> {
        let record = super::pack_install::PackInstallRepository::new(pool.clone())
            .find_by_id(install)
            .await?
            .ok_or_else(denied)?;
        let token_hash = crate::auth::hash_integration_token(archive_token);
        if record.candidate_access_token_hash.as_deref() != Some(&token_hash) {
            return Err(denied());
        }
        let mut s = CandidateSource {
            epochs: Vec::new(),
            install,
            worker,
            instance,
            pack_id: record.pack_id,
            pack_ref: record.pack_ref,
            pack_version: record.pack_version,
            archive_key: record.candidate_object_key.ok_or_else(denied)?,
            archive_version: record.candidate_provider_version.ok_or_else(denied)?,
            archive_digest: record.candidate_digest.ok_or_else(denied)?,
            archive_size: record.candidate_archive_size.ok_or_else(denied)?,
            archive_token_hash: token_hash,
            effects: None,
        };
        let mut tx = pool.begin().await?;
        let deadline = source(&mut tx, &s).await?;
        s.epochs =
            workload_source::snapshot(&mut tx, vec![("worker", worker), ("install", install)])
                .await?;
        let rebound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_process_effect WHERE purpose='candidate_test' AND (target->>'install')::BIGINT=$1 AND (target->>'attempt'<>$2 OR source_epochs<>$3))")
            .bind(install).bind(instance.to_string()).bind(Json(&s.epochs)).fetch_one(&mut *tx).await?;
        if rebound {
            return Err(denied());
        }
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        Ok(s)
    }
    pub async fn validate_source(pool: &PgPool, s: &CandidateSource) -> Result<()> {
        let mut tx = pool.begin().await?;
        let deadline = source(&mut tx, s).await?;
        epochs(&mut tx, s).await?;
        fresh(&mut tx, deadline).await?;
        tx.commit().await?;
        if Utc::now() >= deadline {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn reserve(
        pool: &PgPool,
        s: &CandidateSource,
        spec: &ProcessSpec,
        master: &str,
    ) -> Result<ProcessSpec> {
        if spec.purpose != LaunchPurpose::CandidateTest
            || spec.pack_access != PackAccess::CandidateCopy
            || spec.stage.is_some()
            || spec.intent.is_nil()
            || !matches!(spec.target,LaunchTarget::CandidateTest{install,test_job,attempt} if install==s.install&&test_job>0&&attempt==s.instance)
            || spec.invocation.env.contains_key("ATTUNE_API_TOKEN")
        {
            return Err(denied());
        }
        let c = Context {
            source: s.clone(),
            intent: spec.intent,
            target: spec.target.clone(),
            image: spec.image.clone(),
            platform: spec.platform,
            inputs: spec.inputs.clone(),
            timeout_ms: spec.timeout_ms,
            invocation: spec.invocation.clone(),
        };
        let encrypted = crypto::encrypt(&serde_json::to_string(&c)?, master)?;
        let mut tx = pool.begin().await?;
        let deadline = source(&mut tx, s).await?;
        let ep = epochs(&mut tx, s).await?;
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_process_effect WHERE purpose='candidate_test' AND (target->>'install')::BIGINT=$1 AND (target->>'attempt'<>$3 OR (target<>$2 AND (NOT resource_removed OR (create_submitted AND candidate_capture IS NULL)))))").bind(s.install).bind(Json(&spec.target)).bind(s.instance.to_string()).fetch_one(&mut *tx).await?;
        if blocked {
            return Err(denied());
        }
        sqlx::query("INSERT INTO execution_process_effect(intent,target,purpose,encrypted_context,source_epochs) VALUES($1,$2,'candidate_test',$3,$4) ON CONFLICT DO NOTHING").bind(spec.intent).bind(Json(&spec.target)).bind(encrypted).bind(Json(&ep)).execute(&mut *tx).await?;
        let old:Record=sqlx::query_as(&format!("SELECT {COLUMNS} FROM execution_process_effect WHERE target=$1 AND purpose='candidate_test' FOR UPDATE")).bind(Json(&spec.target)).fetch_one(&mut *tx).await?;
        let retained = old.context(master)?;
        let mut expected = c;
        expected.intent = retained.intent;
        if serde_json::to_value(&expected)? != serde_json::to_value(&retained)?
            || ep != old.source_epochs.0
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
    /// Retry cannot reset the original budget, and a manager's wall-clock
    /// offset cannot extend the database-owned deadline.
    pub async fn remaining(
        pool: &PgPool,
        intent: Uuid,
        master: &str,
    ) -> Result<std::time::Duration> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        let ms = i64::try_from(old.context(master)?.timeout_ms.ok_or_else(denied)?)
            .map_err(|_| denied())?;
        let deadline = old
            .created
            .checked_add_signed(chrono::Duration::milliseconds(ms))
            .ok_or_else(denied)?;
        let micros: i64 = sqlx::query_scalar(
            "SELECT floor(EXTRACT(EPOCH FROM ($1::TIMESTAMPTZ-clock_timestamp()))*1000000)::BIGINT",
        )
        .bind(deadline)
        .fetch_one(&mut *tx)
        .await?;
        tx.rollback().await?;
        let micros = micros.min(ms.checked_mul(1000).ok_or_else(denied)?);
        if micros <= 0 {
            return Err(Error::Timeout("Candidate runner budget expired".into()));
        }
        Ok(std::time::Duration::from_micros(micros as u64))
    }
    pub async fn validate(pool: &PgPool, intent: Uuid, master: &str) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        let c = old.context(master)?;
        let d = source(&mut tx, &c.source).await?;
        if old.stop_requested || epochs(&mut tx, &c.source).await? != old.source_epochs.0 {
            return Err(denied());
        }
        fresh(&mut tx, d).await?;
        tx.commit().await?;
        if Utc::now() >= d {
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
        let c = old.context(master)?;
        let d = source(&mut tx, &c.source).await?;
        let ep = epochs(&mut tx, &c.source).await?;
        let old = row(&mut tx, intent, true).await?;
        if old.source_epochs.0 != ep || old.stop_requested {
            return Err(denied());
        }
        if old.create_submitted {
            return Ok(ProcessCreateDisposition::ReconcileOnly);
        }
        fresh(&mut tx, d).await?;
        sqlx::query("UPDATE execution_process_effect SET create_submitted=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        fresh(&mut tx, d).await?;
        tx.commit().await?;
        if Utc::now() >= d {
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
        let d = source(&mut tx, &c.source).await?;
        let ep = epochs(&mut tx, &c.source).await?;
        let old = row(&mut tx, intent, true).await?;
        if old.source_epochs.0 != ep
            || old.stop_requested
            || !old.create_submitted
            || old.stdin_consumed
            || old.exit_outcome.is_some()
            || old.resource.as_ref().map(|r| &r.0) != Some(resource)
        {
            return Err(denied());
        }
        fresh(&mut tx, d).await?;
        sqlx::query("UPDATE execution_process_effect SET stdin_consumed=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        fresh(&mut tx, d).await?;
        tx.commit().await?;
        if Utc::now() >= d {
            return Err(denied());
        }
        LaunchStdinPayload::from_bytes(Vec::new())
    }
    pub async fn acknowledge_removed(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, true).await?;
        if old.resource.as_ref().map(|r| &r.0) != Some(resource)
            || old.exit_outcome.is_none()
            || !old.stop_requested
        {
            return Err(denied());
        }
        sqlx::query("UPDATE execution_process_effect SET resource_removed=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn capture(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        capture: &CandidateCapture,
        master: &str,
    ) -> Result<()> {
        if capture.stdout.len() > crate::test_executor::MAX_TEST_CAPTURE_TEXT_BYTES
            || capture.stderr.len() > crate::test_executor::MAX_TEST_CAPTURE_TEXT_BYTES
        {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        let snapshot = row(&mut tx, intent, false).await?;
        let c = snapshot.context(master)?;
        let d = source(&mut tx, &c.source).await?;
        let ep = epochs(&mut tx, &c.source).await?;
        let old = row(&mut tx, intent, true).await?;
        if ep != old.source_epochs.0
            || !old.resource_removed
            || !old.stdin_consumed
            || old.resource.as_ref().map(|r| &r.0) != Some(resource)
            || old.exit_outcome.as_ref().is_none_or(|o| {
                o.0.exit_code != capture.exit_code
                    || (o.0.oom_killed && capture.interruption.is_none())
            })
        {
            return Err(denied());
        }
        let value = serde_json::to_string(capture)?;
        if let Some(previous) = old.candidate_capture {
            if crypto::decrypt(&previous, master)? != value {
                return Err(denied());
            }
        } else {
            let encrypted = crypto::encrypt(&value, master)?;
            sqlx::query("UPDATE execution_process_effect SET candidate_capture=$2 WHERE intent=$1")
                .bind(intent)
                .bind(encrypted)
                .execute(&mut *tx)
                .await?;
        }
        fresh(&mut tx, d).await?;
        tx.commit().await?;
        if Utc::now() >= d {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn captured(
        pool: &PgPool,
        intent: Uuid,
        master: &str,
    ) -> Result<Option<CandidateCapture>> {
        let mut tx = pool.begin().await?;
        let old = row(&mut tx, intent, false).await?;
        let c = old.context(master)?;
        let d = source(&mut tx, &c.source).await?;
        if epochs(&mut tx, &c.source).await? != old.source_epochs.0 {
            return Err(denied());
        }
        let result = old
            .candidate_capture
            .map(|value| {
                crypto::decrypt(&value, master)
                    .and_then(|v| serde_json::from_str(&v).map_err(Into::into))
            })
            .transpose()?;
        fresh(&mut tx, d).await?;
        tx.commit().await?;
        if Utc::now() >= d {
            return Err(denied());
        }
        Ok(result)
    }
    /// Final disclosure/publication admission, independent of physical cleanup.
    /// The caller owns policy and result persistence in this SAME transaction.
    /// Close the optional API lease only after this lock, using the same owner;
    /// own revocation/terminal writes may bump epochs, so final freshness checks
    /// time rather than accepting another snapshot after those writes.
    pub async fn lock_result(
        tx: &mut Transaction<'_, Postgres>,
        s: &CandidateSource,
        master: &str,
    ) -> Result<CandidateResultFence> {
        let mut deadline = source(tx, s).await?;
        epochs(tx, s).await?;
        let mut after = 0i64;
        loop {
            let records:Vec<Record>=sqlx::query_as(&format!("SELECT {COLUMNS} FROM execution_process_effect WHERE purpose='candidate_test' AND (target->>'install')::BIGINT=$1 AND id>$2 ORDER BY id LIMIT 4"))
                .bind(s.install).bind(after).fetch_all(&mut **tx).await?;
            if records.is_empty() {
                break;
            }
            for record in records {
                after = record.id;

                let c = record.context(master)?;
                if c.source.instance != s.instance
                    || c.source.worker != s.worker
                    || !record.resource_removed
                    || (record.create_submitted && record.candidate_capture.is_none())
                {
                    return Err(denied());
                }
                deadline = deadline.min(source(tx, &c.source).await?);
                epochs(tx, &c.source).await?;
                if let Some(value) = record.candidate_capture {
                    let _: CandidateCapture =
                        serde_json::from_str(&crypto::decrypt(&value, master)?)
                            .map_err(|_| denied())?;
                }
            }
        }
        fresh(tx, deadline).await?;
        Ok(CandidateResultFence { deadline })
    }
    /// Positive never-submitted cleanup tombstones. An unknown Create commit
    /// has create_submitted=true and can never take this path.
    pub async fn settle_unsubmitted(pool: &PgPool, s: &CandidateSource) -> Result<()> {
        sqlx::query("UPDATE execution_process_effect SET stop_requested=TRUE,resource_removed=TRUE WHERE purpose='candidate_test' AND (target->>'install')::BIGINT=$1 AND NOT create_submitted AND resource IS NULL AND NOT stdin_consumed")
            .bind(s.install).execute(pool).await?;
        Ok(())
    }
    pub async fn settled(pool: &PgPool, s: &CandidateSource, master: &str) -> Result<bool> {
        let unsettled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_process_effect WHERE purpose='candidate_test' AND (target->>'install')::BIGINT=$1 AND (NOT resource_removed OR (create_submitted AND candidate_capture IS NULL)))").bind(s.install).fetch_one(pool).await?;
        if unsettled {
            return Ok(false);
        }
        // A non-NULL but unreadable capture is incomplete, not a cleanup ACK.
        use futures::TryStreamExt;
        let mut captures=sqlx::query_scalar::<_,String>("SELECT candidate_capture FROM execution_process_effect WHERE purpose='candidate_test' AND (target->>'install')::BIGINT=$1 AND candidate_capture IS NOT NULL")
            .bind(s.install).fetch(pool);
        while let Some(value) = captures.try_next().await? {
            let capture: CandidateCapture =
                serde_json::from_str(&crypto::decrypt(&value, master)?).map_err(|_| denied())?;
            if capture.stdout.len() > crate::test_executor::MAX_TEST_CAPTURE_TEXT_BYTES
                || capture.stderr.len() > crate::test_executor::MAX_TEST_CAPTURE_TEXT_BYTES
            {
                return Err(denied());
            }
        }
        Ok(true)
    }
}
