//! Durable native-effect evidence, not a second execution scheduler. Existing
//! admission/origin policy is rechecked before the one-shot stdin commit.
use super::{
    execution_input::{ExecutionInputRepository, FrozenInputSnapshot, SelectedExecutionInput},
    workload_launch::WorkloadLaunchRepository,
    workload_source::{self, SourceEpoch},
};
use crate::{
    crypto,
    isolated_process::{ProcessSpec, ResourceIdentity},
    workload_launch::{
        LaunchInvocation, LaunchPlatform, LaunchPurpose, LaunchTarget, PackAccess, PinnedImage,
        PinnedLaunchInput, PrivateStage,
    },
    workload_stdin::LaunchStdinPayload,
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool};
use uuid::Uuid;

const COLUMNS: &str =
    "intent,target,encrypted_context,source_epochs,resource,stdin_consumed,stop_requested";
fn denied() -> Error {
    Error::PermissionDenied("Execution process effect unavailable".into())
}

/// Private encrypted storage projection; never an API/audit/tracing DTO.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenProcess {
    intent: Uuid,
    purpose: LaunchPurpose,
    target: LaunchTarget,
    owner: i64,
    image: PinnedImage,
    platform: LaunchPlatform,
    inputs: Vec<PinnedLaunchInput>,
    pack_access: PackAccess,
    stage: Option<PrivateStage>,
    timeout_ms: Option<u64>,
    invocation: LaunchInvocation,
    input: FrozenInputSnapshot,
    stdin: String,
}
impl FrozenProcess {
    fn new(spec: &ProcessSpec, owner: i64, input: &SelectedExecutionInput) -> Self {
        Self {
            intent: spec.intent,
            purpose: spec.purpose,
            target: spec.target.clone(),
            owner,
            image: spec.image.clone(),
            platform: spec.platform,
            inputs: spec.inputs.clone(),
            pack_access: spec.pack_access,
            stage: spec.stage.clone(),
            timeout_ms: spec.timeout_ms,
            invocation: spec.invocation.clone(),
            input: FrozenInputSnapshot::from(&input.snapshot),
            stdin: input.stdin_json.clone(),
        }
    }
    fn spec(&self) -> ProcessSpec {
        ProcessSpec {
            intent: self.intent,
            purpose: self.purpose,
            target: self.target.clone(),
            image: self.image.clone(),
            platform: self.platform,
            inputs: self.inputs.clone(),
            pack_access: self.pack_access,
            stage: self.stage.clone(),
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
    resource: Option<Json<ResourceIdentity>>,
    stdin_consumed: bool,
    stop_requested: bool,
}
impl Record {
    fn context(&self, master: &str) -> Result<FrozenProcess> {
        let plaintext = crypto::decrypt(&self.encrypted_context, master).map_err(|_| denied())?;
        let context: FrozenProcess = serde_json::from_str(&plaintext).map_err(|_| denied())?;
        if context.intent != self.intent || context.target != self.target.0 {
            return Err(denied());
        }
        Ok(context)
    }
}

pub struct ExecutionProcessRepository;
impl ExecutionProcessRepository {
    /// Reserve BEFORE provider creation. A retry with changed private context
    /// is denied; randomized ciphertext has no public plaintext-secret digest.
    pub async fn reserve(
        pool: &PgPool,
        spec: &ProcessSpec,
        owner: i64,
        selected: &SelectedExecutionInput,
        master: &str,
    ) -> Result<()> {
        if spec.intent.is_nil()
            || spec.purpose != LaunchPurpose::Action
            || !matches!(spec.target, LaunchTarget::Execution { execution, .. }
                if execution==selected.snapshot.execution())
        {
            return Err(denied());
        }
        let context = FrozenProcess::new(spec, owner, selected);
        // The first owned vertical is the public-input case. Do not create a
        // provider resource for unsupported origin projections while migration
        // of the complete existing secret/CLI paths is still in progress.
        context.input.public_snapshot()?;
        let serialized = serde_json::to_string(&context).map_err(|_| denied())?;
        let encrypted = crypto::encrypt(&serialized, master).map_err(|_| denied())?;
        let mut tx = pool.begin().await?;
        let proof = WorkloadLaunchRepository::source(&mut tx, &spec.target, owner).await?;
        let mut input = ExecutionInputRepository::restore(
            &mut tx,
            selected.snapshot.execution(),
            Some(master),
            Some(&selected.snapshot),
        )
        .await?;
        let provided: serde_json::Value =
            serde_json::from_str(&selected.stdin_json).map_err(|_| denied())?;
        if &provided != input.value() {
            return Err(denied());
        }
        input.ensure_fresh().await?;
        let (_, _, _, expiry) = input.into_parts();
        let epochs = workload_source::snapshot(&mut tx, proof.sources).await?;
        sqlx::query("INSERT INTO execution_process_effect(intent,target,encrypted_context,source_epochs) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(spec.intent).bind(Json(&spec.target)).bind(encrypted).bind(Json(&epochs))
            .execute(&mut *tx).await?;
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR UPDATE"
        ))
        .bind(spec.intent)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
        if serde_json::to_string(&record.context(master)?).map_err(|_| denied())? != serialized
            || record.source_epochs.0 != epochs
            || record.stop_requested
            || record.stdin_consumed
        {
            return Err(denied());
        }
        expiry.ensure_fresh_on(&mut tx).await?;
        fresh_on(&mut tx, proof.deadline).await?;
        tx.commit().await?;
        expiry.ensure_fresh()?;
        fresh(proof.deadline)
    }

    /// Provider identity remains recordable after source revocation so cleanup
    /// can retain exact ownership. It never grants start/input authority.
    pub async fn record_resource(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let old: Option<Option<Json<ResourceIdentity>>> = sqlx::query_scalar(
            "SELECT resource FROM execution_process_effect WHERE intent=$1 FOR UPDATE",
        )
        .bind(intent)
        .fetch_optional(&mut *tx)
        .await?;
        let old = old.ok_or_else(denied)?;
        if old.as_ref().is_some_and(|old| old.0 != *resource) {
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

    /// Source/input locks precede epochs and operation. COMMIT consumes stdin
    /// before the caller can release the held gate. Lost ACK is never replayed.
    pub async fn consume_stdin(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        master: &str,
    ) -> Result<LaunchStdinPayload> {
        let mut tx = pool.begin().await?;
        let snapshot: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1"
        ))
        .bind(intent)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
        let context = snapshot.context(master)?;
        let proof =
            WorkloadLaunchRepository::source(&mut tx, &context.target, context.owner).await?;
        let expected = context.input.public_snapshot()?;
        let mut input = ExecutionInputRepository::restore(
            &mut tx,
            expected.execution(),
            Some(master),
            Some(&expected),
        )
        .await?;
        let selected: serde_json::Value =
            serde_json::from_str(&context.stdin).map_err(|_| denied())?;
        if &selected != input.value() {
            return Err(denied());
        }
        input.ensure_fresh().await?;
        let (_, _, _, expiry) = input.into_parts();
        let epochs = workload_source::snapshot(&mut tx, proof.sources).await?;
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR UPDATE"
        ))
        .bind(intent)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
        if record.encrypted_context != snapshot.encrypted_context
            || record.source_epochs.0 != epochs
            || record.stdin_consumed
            || record.stop_requested
            || record.resource.as_ref().map(|r| &r.0) != Some(resource)
        {
            return Err(denied());
        }
        expiry.ensure_fresh_on(&mut tx).await?;
        fresh_on(&mut tx, proof.deadline).await?;
        sqlx::query("UPDATE execution_process_effect SET stdin_consumed=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        expiry.ensure_fresh_on(&mut tx).await?;
        fresh_on(&mut tx, proof.deadline).await?;
        tx.commit().await?;
        expiry.ensure_fresh()?;
        fresh(proof.deadline)?;
        LaunchStdinPayload::from_bytes(context.stdin.into_bytes())
    }

    /// Retain the exact frozen plan for trusted recovery; this is not execution
    /// authorization. Recovery must consume the same source/attempt gate.
    pub async fn retained_spec(pool: &PgPool, intent: Uuid, master: &str) -> Result<ProcessSpec> {
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1"
        ))
        .bind(intent)
        .fetch_optional(pool)
        .await?
        .ok_or_else(denied)?;
        Ok(record.context(master)?.spec())
    }
    pub async fn request_stop(pool: &PgPool, intent: Uuid) -> Result<()> {
        let row =
            sqlx::query("UPDATE execution_process_effect SET stop_requested=TRUE WHERE intent=$1")
                .bind(intent)
                .execute(pool)
                .await?;
        if row.rows_affected() != 1 {
            return Err(denied());
        }
        Ok(())
    }
}
fn fresh(deadline: Option<DateTime<Utc>>) -> Result<()> {
    if deadline.is_none_or(|deadline| deadline <= Utc::now()) {
        return Err(denied());
    }
    Ok(())
}

async fn fresh_on(
    connection: &mut sqlx::PgConnection,
    deadline: Option<DateTime<Utc>>,
) -> Result<()> {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(connection)
        .await?;
    if deadline.is_none_or(|deadline| deadline <= now) {
        return Err(denied());
    }
    Ok(())
}
