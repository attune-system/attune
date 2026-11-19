//! Durable native-effect evidence, not a second execution scheduler. Existing
//! admission/origin policy is rechecked before the one-shot stdin commit.
use super::{
    execution_input::{
        ExecutionInputRepository, FrozenInputFrontier, FrozenInputSnapshot, SelectedExecutionInput,
    },
    workload_source::{self, SourceEpoch},
};
use crate::{
    crypto,
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    workload_launch::{
        LaunchExitOutcome, LaunchInvocation, LaunchPlatform, LaunchPurpose, LaunchTarget,
        PackAccess, PinnedImage, PinnedLaunchInput, PrivateStage,
    },
    workload_stdin::LaunchStdinPayload,
    Error, Result,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool};
use uuid::Uuid;

const COLUMNS: &str =
    "intent,target,purpose,encrypted_context,source_epochs,resource,stdin_consumed,stop_requested,create_submitted,exit_outcome";
fn denied() -> Error {
    Error::PermissionDenied("Execution process effect unavailable".into())
}
fn purpose_name(purpose: LaunchPurpose) -> Result<&'static str> {
    match purpose {
        LaunchPurpose::Action => Ok("action"),
        LaunchPurpose::RuntimeProbe => Ok("runtime_probe"),
        _ => Err(denied()),
    }
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
    input_frontier: FrozenInputFrontier,
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
            input_frontier: FrozenInputFrontier::empty(),
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
    purpose: String,
    encrypted_context: String,
    source_epochs: Json<Vec<SourceEpoch>>,
    resource: Option<Json<ResourceIdentity>>,
    stdin_consumed: bool,
    stop_requested: bool,
    create_submitted: bool,
    exit_outcome: Option<Json<LaunchExitOutcome>>,
}

#[derive(sqlx::FromRow)]
struct RuntimeFence {
    intent: Uuid,
    target: Json<LaunchTarget>,
    source_epochs: Json<Vec<SourceEpoch>>,
}
impl Record {
    fn context(&self, master: &str) -> Result<FrozenProcess> {
        let plaintext = crypto::decrypt(&self.encrypted_context, master).map_err(|_| denied())?;
        let context: FrozenProcess = serde_json::from_str(&plaintext).map_err(|_| denied())?;
        if context.intent != self.intent
            || context.target != self.target.0
            || purpose_name(context.purpose)? != self.purpose
        {
            return Err(denied());
        }
        Ok(context)
    }
}

/// Source protection for one existing API publication transaction. Prepare
/// before input-origin locking; retain after all source locks and before
/// target/provider-operation waits. This is not execution or upload authority.
pub struct NativeFilePublication {
    snapshot: RuntimeFence,
    worker_deadline: DateTime<Utc>,
    input_fence: super::runtime_authority::ApprovalEffectFence,
}
impl NativeFilePublication {
    pub async fn retain(
        self,
        connection: &mut sqlx::PgConnection,
    ) -> Result<super::runtime_authority::ApprovalEffectFence> {
        let sources = self
            .snapshot
            .source_epochs
            .iter()
            .map(|epoch| (epoch.kind.as_str(), epoch.subject))
            .collect();
        let current = workload_source::snapshot(connection, sources).await?;
        if current != self.snapshot.source_epochs.0 {
            return Err(denied());
        }
        let admitted: Option<bool> = sqlx::query_scalar(
            "SELECT stdin_consumed AND resource IS NOT NULL \
             FROM execution_process_effect WHERE intent=$1 AND target=$2 AND source_epochs=$3 FOR SHARE"
        ).bind(self.snapshot.intent).bind(&self.snapshot.target)
            .bind(&self.snapshot.source_epochs).fetch_optional(&mut *connection).await?;
        if admitted != Some(true) {
            return Err(denied());
        }
        fresh_on(connection, Some(self.worker_deadline)).await?;
        self.input_fence.ensure_fresh_on(connection).await?;
        Ok(self.input_fence.intersect(
            &super::runtime_authority::ApprovalEffectFence::from_input_approval(Some(
                self.worker_deadline,
            ))?,
        ))
    }
}

struct SourceProof {
    sources: Vec<(&'static str, i64)>,
    deadline: Option<DateTime<Utc>>,
}

impl SourceProof {
    fn include_input(&mut self, frontier: &FrozenInputFrontier) {
        self.sources.extend(frontier.epoch_sources());
        self.deadline = self.deadline.into_iter().chain(frontier.deadline()).min();
    }
}

pub struct ExecutionProcessRepository;
impl ExecutionProcessRepository {
    /// Reuse the existing execution authority choice and attempt fence. The
    /// retired controller relay contributes no policy or scheduling state.
    async fn source(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        target: &LaunchTarget,
        expected_owner: i64,
    ) -> Result<SourceProof> {
        super::schema_type::SchemaTypeRepository::lock_selection(tx).await?;
        Self::source_for(tx, target, expected_owner, false).await
    }
    async fn source_for(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        target: &LaunchTarget,
        expected_owner: i64,
        allow_canceling: bool,
    ) -> Result<SourceProof> {
        use super::{
            delegated_execution::DelegatedExecutionRepository,
            execution_runtime_admission::{
                ExecutionAdmissionRepository, ExistingExecutionAuthority,
            },
            runtime_authority::{
                RuntimeAuthorityRepository, RuntimeCredentialBinding, RuntimeOwner,
            },
        };
        match target {
            LaunchTarget::Execution {
                execution,
                worker,
                worker_instance,
                attempt,
            } => {
                let mut deadline = None;
                let selected =
                    ExecutionAdmissionRepository::selected_authority(tx, *execution).await?;
                match selected {
                    ExistingExecutionAuthority::Delegated => {
                        let authority =
                            DelegatedExecutionRepository::validate(tx, *execution).await?;
                        if authority.owner != Some(expected_owner) || !authority.explicit {
                            return Err(denied());
                        }
                        let current: Option<Uuid>=sqlx::query_scalar("SELECT a.attempt FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=$1 AND e.worker=$2 AND a.worker=$2 AND NOT a.closed AND a.producer_kind='worker' AND (e.status='running' OR ($3 AND e.status='canceling')) FOR SHARE OF e,a")
                            .bind(execution).bind(worker).bind(allow_canceling).fetch_optional(&mut **tx).await?;
                        if current != Some(*attempt) {
                            return Err(denied());
                        }
                        DelegatedExecutionRepository::validate(tx, *execution).await?;
                    }
                    ExistingExecutionAuthority::Recorded => {
                        let (record, fence) = ExecutionAdmissionRepository::require_with_fence(
                            tx,
                            RuntimeOwner::Execution(*execution),
                        )
                        .await?;
                        if record.authority.owner.sponsor_identity_id != expected_owner {
                            return Err(denied());
                        }
                        let current: Option<Uuid>=sqlx::query_scalar("SELECT f.attempt FROM execution e JOIN runtime_authority_execution_fence f ON f.execution=e.id WHERE e.id=$1 AND e.worker=$2 AND (e.status='running' OR ($4 AND e.status='canceling')) AND f.admission=$3 FOR SHARE OF e,f")
                            .bind(execution).bind(worker).bind(record.id).bind(allow_canceling).fetch_optional(&mut **tx).await?;
                        if current != Some(*attempt) {
                            return Err(denied());
                        }
                        if !allow_canceling {
                            RuntimeAuthorityRepository::validate_native_execution_source(
                                tx,
                                &RuntimeCredentialBinding {
                                    execution_id: *execution,
                                    admission_id: record.id,
                                    sponsor_identity_id: expected_owner,
                                    action_ref: record.authority.binding.action_ref().into(),
                                    attempt: *attempt,
                                },
                            )
                            .await?
                            .ensure_fresh_on(tx)
                            .await?;
                        }
                        fence.ensure_fresh_on(tx).await?;
                        deadline = fence.approval_deadline();
                    }
                }
                if !allow_canceling {
                    let execution_deadline:Option<DateTime<Utc>>=sqlx::query_scalar("SELECT started_at+make_interval(secs=>timeout_seconds) FROM execution WHERE id=$1 AND started_at IS NOT NULL AND timeout_seconds>0")
                        .bind(execution).fetch_optional(&mut **tx).await?;
                    let execution_deadline = execution_deadline.ok_or_else(denied)?;
                    deadline = Some(deadline.map_or(execution_deadline, |approval| {
                        approval.min(execution_deadline)
                    }));
                    fresh_on(tx, Some(execution_deadline)).await?;
                }
                let worker_deadline = Self::worker_deadline(tx, *worker, *worker_instance).await?;
                deadline = Some(
                    deadline.map_or(worker_deadline, |deadline| deadline.min(worker_deadline)),
                );
                let mut sources = vec![
                    ("owner", expected_owner),
                    ("execution", *execution),
                    ("worker", *worker),
                ];
                if !allow_canceling {
                    sources.push(("execution_input", *execution));
                }
                Ok(SourceProof { sources, deadline })
            }
            _ => Err(denied()),
        }
    }
    /// Called by the existing file API among source locks. Runtime logs keep
    /// their independent custody/handover contract.
    pub async fn prepare_file_publication(
        connection: &mut sqlx::PgConnection,
        version: i64,
        execution: i64,
        attempt: Uuid,
        master: Option<&str>,
    ) -> Result<Option<NativeFilePublication>> {
        let ordinary: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM artifact_version v JOIN artifact a ON a.id=v.artifact \
         WHERE v.id=$1 AND v.execution=$2 AND v.file_path IS NOT NULL AND a.classification<>'runtime_log')"
    ).bind(version).bind(execution).fetch_one(&mut *connection).await?;
        if !ordinary {
            return Ok(None);
        }
        let snapshot: Option<Record> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM execution_process_effect \
         WHERE purpose='action' AND target->>'kind'='execution' AND (target->>'execution')::BIGINT=$1 AND target->>'attempt'=$2"
    )).bind(execution).bind(attempt.to_string()).fetch_optional(&mut *connection).await?;
        let Some(snapshot) = snapshot else {
            Self::deny_inspection_only(connection, execution, attempt).await?;
            return Ok(None);
        };
        // Legacy/public publications above require no encryption configuration.
        // A native effect must positively retain its own encrypted input proof.
        let context = snapshot.context(master.ok_or_else(denied)?)?;
        let LaunchTarget::Execution {
            worker,
            worker_instance,
            ..
        } = snapshot.target.0
        else {
            return Err(denied());
        };
        let worker_deadline = Self::worker_deadline(connection, worker, worker_instance).await?;
        let input_fence = context.input_frontier.retain(connection).await?;
        fresh_on(connection, Some(worker_deadline)).await?;
        Ok(Some(NativeFilePublication {
            snapshot: RuntimeFence {
                intent: snapshot.intent,
                target: snapshot.target,
                source_epochs: snapshot.source_epochs,
            },
            worker_deadline,
            input_fence,
        }))
    }

    async fn deny_inspection_only(
        connection: &mut sqlx::PgConnection,
        execution: i64,
        attempt: Uuid,
    ) -> Result<()> {
        let probe: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM execution_process_effect WHERE purpose='runtime_probe' \
             AND target->>'kind'='execution' AND (target->>'execution')::BIGINT=$1 AND target->>'attempt'=$2)"
        ).bind(execution).bind(attempt.to_string()).fetch_one(connection).await?;
        if probe {
            return Err(denied());
        }
        Ok(())
    }

    async fn worker_deadline(
        connection: &mut sqlx::PgConnection,
        worker: i64,
        instance: Uuid,
    ) -> Result<DateTime<Utc>> {
        sqlx::query_scalar(
            "SELECT last_heartbeat+interval '90 seconds' FROM worker WHERE id=$1 \
         AND (meta->>'registration_incarnation')::UUID=$2 AND status IN ('active','busy') \
         AND last_heartbeat>clock_timestamp()-interval '90 seconds' FOR SHARE",
        )
        .bind(worker)
        .bind(instance)
        .fetch_optional(connection)
        .await?
        .ok_or_else(denied)
    }

    /// Runtime token consumers retain this fence through their own effect
    /// transaction. Native exit/stop waits for admitted calls, then closes new
    /// allocation even while execution remains Running for log custody drain.
    pub(crate) async fn retain_runtime_run(
        connection: &mut sqlx::PgConnection,
        execution: i64,
        attempt: Uuid,
    ) -> Result<()> {
        let snapshot: Option<RuntimeFence> = sqlx::query_as(
            "SELECT intent,target,source_epochs FROM execution_process_effect \
             WHERE purpose='action' AND target->>'kind'='execution' AND (target->>'execution')::BIGINT=$1 \
             AND target->>'attempt'=$2",
        )
        .bind(execution)
        .bind(attempt.to_string())
        .fetch_optional(&mut *connection)
        .await?;
        let Some(snapshot) = snapshot else {
            Self::deny_inspection_only(connection, execution, attempt).await?;
            // Non-process producers retain their existing admission contracts.
            // Isolated process evidence cannot disappear: its deletion guard
            // retains the original attempt's closure/tombstone independently.
            return Ok(());
        };
        let LaunchTarget::Execution {
            worker,
            worker_instance,
            ..
        } = snapshot.target.0
        else {
            return Err(denied());
        };
        let current: Option<Uuid> = sqlx::query_scalar(
            "SELECT (meta->>'registration_incarnation')::UUID FROM worker \
             WHERE id=$1 AND status IN ('active','busy') \
             AND last_heartbeat>clock_timestamp()-interval '90 seconds' FOR SHARE",
        )
        .bind(worker)
        .fetch_optional(&mut *connection)
        .await?;
        if current != Some(worker_instance) {
            return Err(denied());
        }
        let sources = snapshot
            .source_epochs
            .iter()
            .filter(|epoch| !super::execution_input::origin_epoch(&epoch.kind))
            .map(|epoch| (epoch.kind.as_str(), epoch.subject))
            .collect();
        let current = workload_source::snapshot(connection, sources).await?;
        let expected: Vec<_> = snapshot
            .source_epochs
            .iter()
            .filter(|epoch| !super::execution_input::origin_epoch(&epoch.kind))
            .cloned()
            .collect();
        if current != expected {
            return Err(denied());
        }
        let open: bool = sqlx::query_scalar(
            "SELECT NOT stop_requested AND exit_outcome IS NULL AND stdin_consumed \
             AND resource IS NOT NULL FROM execution_process_effect WHERE intent=$1 FOR SHARE",
        )
        .bind(snapshot.intent)
        .fetch_one(connection)
        .await?;
        if !open {
            return Err(denied());
        }
        Ok(())
    }

    /// CLI stdin is derived from the same restored input and bound argument
    /// contract as argv. Original parameters stay in encrypted context; no new
    /// plaintext fingerprint or independently supplied payload is authority.
    /// Program metadata is useful only with a positive, exact, current native
    /// inspection receipt. It never confers action completion or file authority.
    async fn prove_inspection(
        connection: &mut sqlx::PgConnection,
        action: &FrozenProcess,
        intent: Uuid,
        master: &str,
    ) -> Result<()> {
        let snapshot: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1"
        ))
        .bind(intent)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(denied)?;
        let probe = snapshot.context(master)?;
        if probe.purpose != LaunchPurpose::RuntimeProbe
            || action.purpose != LaunchPurpose::Action
            || probe.target != action.target
            || probe.owner != action.owner
            || probe.image != action.image
            || probe.platform != action.platform
            || probe.inputs != action.inputs
            || probe.pack_access != action.pack_access
            || probe.stage.is_some()
            || action.stage.is_some()
            || probe.stdin != action.stdin
            || probe.input != action.input
            || probe.input_frontier != action.input_frontier
        {
            return Err(denied());
        }
        if probe.invocation.argv.len() != 3
            || probe.invocation.argv[0]
                != format!(
                    "{}/linux-{}",
                    crate::workload_launch::RUNNER_ROOT,
                    probe.platform.native_arch()
                )
            || probe.invocation.argv[1] != "inspect-command-program"
            || probe.invocation.env.contains_key("ATTUNE_API_TOKEN")
        {
            return Err(denied());
        }
        let input_fence = probe.input_frontier.retain(connection).await?;
        let sources = snapshot
            .source_epochs
            .iter()
            .map(|epoch| (epoch.kind.as_str(), epoch.subject))
            .collect();
        let current = workload_source::snapshot(connection, sources).await?;
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR SHARE"
        ))
        .bind(intent)
        .fetch_one(&mut *connection)
        .await?;
        if record.encrypted_context != snapshot.encrypted_context
            || record.source_epochs.0 != current
            || record.stop_requested
            || !record.create_submitted
            || !record.stdin_consumed
            || record.resource.is_none()
            || record.exit_outcome.as_ref().map(|exit| exit.0)
                != Some(LaunchExitOutcome {
                    exit_code: 0,
                    oom_killed: false,
                })
        {
            return Err(denied());
        }
        input_fence.ensure_fresh_on(connection).await?;
        Ok(())
    }

    /// Capture the fixed helper's bounded metadata on the transaction that owns
    /// its fresh source/input/epoch/effect proof. No nested pooled transaction.
    pub async fn complete_command_inspection(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        selected: &crate::action_cli::SelectedProgram,
        master: &str,
    ) -> Result<()> {
        use super::{execution::ExecutionRepository, FindById};
        let mut tx = pool.begin().await?;
        let snapshot: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1"
        ))
        .bind(intent)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
        let context = snapshot.context(master)?;
        if context.purpose != LaunchPurpose::RuntimeProbe {
            return Err(denied());
        }
        let LaunchTarget::Execution {
            execution,
            worker,
            attempt,
            ..
        } = context.target
        else {
            return Err(denied());
        };
        let mut proof = Self::source(&mut tx, &context.target, context.owner).await?;
        let expected = context.input.retained_snapshot();
        let mut input =
            ExecutionInputRepository::restore(&mut tx, execution, Some(master), Some(&expected))
                .await?;
        let value: serde_json::Value =
            serde_json::from_str(&context.stdin).map_err(|_| denied())?;
        if &value != input.value() {
            return Err(denied());
        }
        input.ensure_fresh().await?;
        let (_, _, _, expiry) = input.into_parts();
        if expiry.frozen_frontier()? != context.input_frontier {
            return Err(denied());
        }
        proof.include_input(&context.input_frontier);
        Self::action_stdin(&mut tx, &context, master).await?;
        let execution_record = ExecutionRepository::find_by_id(&mut *tx, execution)
            .await?
            .ok_or_else(denied)?;
        let snapshot_action = execution_record
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?;
        let Some(crate::pack_format::LaunchSpec::Command { program, .. }) =
            &snapshot_action.executable.action.launch
        else {
            return Err(denied());
        };
        let path = selected.path.to_str().ok_or_else(denied)?;
        if selected.invocation_name != *program
            || selected.artifact.is_some()
            || selected.variant.is_some()
            || !path.starts_with('/')
            || path.len() > 4096
            || path.chars().any(char::is_control)
            || path
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
            || selected.path.starts_with(crate::workload_launch::PACK_ROOT)
            || selected.sha256.len() != 64
            || !selected
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(denied());
        }
        let epochs = workload_source::snapshot(&mut tx, proof.sources).await?;
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR UPDATE"
        ))
        .bind(intent)
        .fetch_one(&mut *tx)
        .await?;
        if record.encrypted_context != snapshot.encrypted_context
            || record.source_epochs.0 != epochs
            || record.stop_requested
            || !record.create_submitted
            || !record.stdin_consumed
            || record.resource.as_ref().map(|resource| &resource.0) != Some(resource)
            || record.exit_outcome.as_ref().map(|exit| exit.0)
                != Some(LaunchExitOutcome {
                    exit_code: 0,
                    oom_killed: false,
                })
        {
            return Err(denied());
        }
        super::execution_program::ExecutionProgramRepository::record_inspected_on(
            &mut tx,
            execution,
            worker,
            attempt,
            snapshot_action.release.id,
            serde_json::to_value(selected).map_err(|_| denied())?,
            Some(intent),
        )
        .await?;
        expiry.ensure_fresh_on(&mut tx).await?;
        fresh_on(&mut tx, proof.deadline).await?;
        tx.commit().await?;
        expiry.ensure_fresh()?;
        fresh(proof.deadline)
    }

    /// Recovery reuses captured metadata only after the original inspection's
    /// source/attempt proof is current. Consumed-but-uncaptured input is denied
    /// by the ordinary one-shot gate; it is never inspected a second time.
    pub async fn inspected_program(
        pool: &PgPool,
        action: &ProcessSpec,
        master: &str,
    ) -> Result<Option<crate::action_cli::SelectedProgram>> {
        let LaunchTarget::Execution {
            execution,
            worker,
            attempt,
            ..
        } = action.target
        else {
            return Err(denied());
        };
        let mut tx = pool.begin().await?;
        let row: Option<(Json<crate::action_cli::SelectedProgram>, Option<Uuid>)> = sqlx::query_as(
            "SELECT selection,native_probe FROM execution_program_selection WHERE execution=$1 AND attempt=$2 AND worker=$3"
        ).bind(execution).bind(attempt).bind(worker).fetch_optional(&mut *tx).await?;
        let Some((Json(selected), Some(intent))) = row else {
            return Ok(None);
        };
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1"
        ))
        .bind(intent)
        .fetch_one(&mut *tx)
        .await?;
        let probe = record.context(master)?;
        let mut proof = Self::source(&mut tx, &probe.target, probe.owner).await?;
        let expected = probe.input.retained_snapshot();
        let mut input =
            ExecutionInputRepository::restore(&mut tx, execution, Some(master), Some(&expected))
                .await?;
        let value: serde_json::Value = serde_json::from_str(&probe.stdin).map_err(|_| denied())?;
        if input.value() != &value {
            return Err(denied());
        }
        input.ensure_fresh().await?;
        let (_, _, _, expiry) = input.into_parts();
        if expiry.frozen_frontier()? != probe.input_frontier {
            return Err(denied());
        }
        proof.include_input(&probe.input_frontier);
        let action_context = FrozenProcess {
            purpose: LaunchPurpose::Action,
            intent: action.intent,
            target: action.target.clone(),
            image: action.image.clone(),
            platform: action.platform,
            inputs: action.inputs.clone(),
            pack_access: action.pack_access,
            stage: action.stage.clone(),
            timeout_ms: action.timeout_ms,
            invocation: action.invocation.clone(),
            ..probe
        };
        Self::prove_inspection(&mut tx, &action_context, intent, master).await?;
        expiry.ensure_fresh_on(&mut tx).await?;
        fresh_on(&mut tx, proof.deadline).await?;
        tx.commit().await?;
        expiry.ensure_fresh()?;
        fresh(proof.deadline)?;
        Ok(Some(selected))
    }

    async fn action_stdin(
        connection: &mut sqlx::PgConnection,
        context: &FrozenProcess,
        master: &str,
    ) -> Result<Vec<u8>> {
        use super::{
            execution::ExecutionRepository,
            schema_contract::{BindingOwner, SchemaContractRepository},
            FindById,
        };
        use crate::{
            action_cli::{ArgumentLimits, ArgumentPlan, InputDelivery, SelectedProgram},
            models::{ParameterDelivery, ParameterFormat},
            workload_launch::RUNNER_ROOT,
        };
        let LaunchTarget::Execution {
            execution,
            worker,
            attempt,
            ..
        } = context.target
        else {
            return Err(denied());
        };
        let execution = ExecutionRepository::find_by_id(&mut *connection, execution)
            .await?
            .ok_or_else(denied)?;
        let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
        let action = &snapshot.executable.action;
        if context.purpose == LaunchPurpose::RuntimeProbe {
            let Some(crate::pack_format::LaunchSpec::Command { program, .. }) = &action.launch
            else {
                return Err(denied());
            };
            let expected = vec![
                format!("{RUNNER_ROOT}/linux-{}", context.platform.native_arch()),
                "inspect-command-program".into(),
                program.clone(),
            ];
            if context.invocation.argv != expected
                || context.stage.is_some()
                || context.pack_access != PackAccess::ReadOnly
                || context.invocation.env.contains_key("ATTUNE_API_TOKEN")
            {
                return Err(denied());
            }
            return Ok(Vec::new());
        }
        let Some(launch) = &action.launch else {
            return if action.parameter_delivery == ParameterDelivery::None {
                Ok(Vec::new())
            } else {
                // Transport remains the canonical admitted JSON. The fixed
                // guest gate handles declared formats/private parameter files.
                Ok(context.stdin.as_bytes().to_vec())
            };
        };
        if !matches!(
            launch,
            crate::pack_format::LaunchSpec::Native { .. }
                | crate::pack_format::LaunchSpec::Command { .. }
        ) || action.parameter_format != ParameterFormat::Json
        {
            return Err(denied());
        }
        let selected: Option<(sqlx::types::Json<SelectedProgram>, Option<Uuid>)> = sqlx::query_as(
            // Metadata is immutable under the retained execution source. Taking
            // its row lock before probe epochs/operation would invert the
            // inspection publisher's operation -> metadata order.
            "SELECT selection,native_probe FROM execution_program_selection WHERE execution=$1 AND attempt=$2 AND worker=$3 AND release=$4",
        ).bind(execution.id).bind(attempt).bind(worker).bind(snapshot.release.id)
            .fetch_optional(&mut *connection).await?;
        let (Json(selected), probe) = selected.ok_or_else(denied)?;
        if matches!(launch, crate::pack_format::LaunchSpec::Command { .. }) {
            Self::prove_inspection(connection, context, probe.ok_or_else(denied)?, master).await?;
        } else if probe.is_some() {
            return Err(denied());
        }
        let binding =
            SchemaContractRepository::find(connection, BindingOwner::Execution, execution.id)
                .await?
                .ok_or_else(denied)?;
        let schema = action
            .param_schema
            .as_ref()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let plan =
            ArgumentPlan::from_bound_contract(&schema, launch.action_arguments()?, binding.input)
                .map_err(|_| denied())?;
        let delivery = match action.parameter_delivery {
            ParameterDelivery::None => InputDelivery::None,
            ParameterDelivery::Stdin => InputDelivery::Stdin,
            _ => return Err(denied()),
        };
        let parameters: serde_json::Value =
            serde_json::from_str(&context.stdin).map_err(|_| denied())?;
        let materialized = plan
            .materialize(
                selected.argument_budget_program()?,
                &parameters,
                &context.input.secret_paths(),
                delivery,
                ArgumentLimits::default(),
            )
            .map_err(|_| denied())?;
        let mut expected = vec![
            format!("{RUNNER_ROOT}/linux-{}", context.platform.native_arch()),
            "exec".into(),
            selected.sha256,
            selected.path.to_str().ok_or_else(denied)?.to_owned(),
            selected.invocation_name,
        ];
        expected.extend(materialized.argv().iter().cloned());
        if context.invocation.argv != expected {
            return Err(denied());
        }
        Ok(materialized.stdin_json().unwrap_or("").as_bytes().to_vec())
    }

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
            || !matches!(
                spec.purpose,
                LaunchPurpose::Action | LaunchPurpose::RuntimeProbe
            )
            || !matches!(spec.target, LaunchTarget::Execution { execution, .. }
                if execution==selected.snapshot.execution())
        {
            return Err(denied());
        }
        let mut context = FrozenProcess::new(spec, owner, selected);
        let expected = context.input.retained_snapshot();
        let mut tx = pool.begin().await?;
        let mut proof = Self::source(&mut tx, &spec.target, owner).await?;
        let mut input = ExecutionInputRepository::restore(
            &mut tx,
            expected.execution(),
            Some(master),
            Some(&expected),
        )
        .await?;
        let provided: serde_json::Value =
            serde_json::from_str(&selected.stdin_json).map_err(|_| denied())?;
        if &provided != input.value() {
            return Err(denied());
        }
        input.ensure_fresh().await?;
        let (_, _, _, expiry) = input.into_parts();
        context.input_frontier = expiry.frozen_frontier()?;
        proof.include_input(&context.input_frontier);
        Self::action_stdin(&mut tx, &context, master).await?;
        let serialized = serde_json::to_string(&context).map_err(|_| denied())?;
        let encrypted = crypto::encrypt(&serialized, master).map_err(|_| denied())?;
        let epochs = workload_source::snapshot(&mut tx, proof.sources).await?;
        sqlx::query("INSERT INTO execution_process_effect(intent,target,purpose,encrypted_context,source_epochs) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(spec.intent).bind(Json(&spec.target)).bind(purpose_name(spec.purpose)?).bind(encrypted).bind(Json(&epochs))
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
            || record.exit_outcome.is_some()
        {
            return Err(denied());
        }
        expiry.ensure_fresh_on(&mut tx).await?;
        fresh_on(&mut tx, proof.deadline).await?;
        tx.commit().await?;
        expiry.ensure_fresh()?;
        fresh(proof.deadline)
    }

    /// The acknowledged durable first-submit decision precedes any provider
    /// create. All subsequent calls are reconciliation-only, including after a
    /// lost COMMIT acknowledgement or source revocation. Reconciliation cannot
    /// release the payload gate or prove provider absence from a 404.
    pub async fn begin_create(
        pool: &PgPool,
        intent: Uuid,
        master: &str,
    ) -> Result<ProcessCreateDisposition> {
        let mut tx = pool.begin().await?;
        let snapshot: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1"
        ))
        .bind(intent)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
        if snapshot.create_submitted || snapshot.stop_requested {
            return Ok(ProcessCreateDisposition::ReconcileOnly);
        }
        let context = snapshot.context(master)?;
        let mut proof = Self::source(&mut tx, &context.target, context.owner).await?;
        context.input_frontier.retain(&mut tx).await?;
        proof.include_input(&context.input_frontier);
        let epochs = workload_source::snapshot(&mut tx, proof.sources).await?;
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR UPDATE"
        ))
        .bind(intent)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
        if record.create_submitted || record.stop_requested {
            return Ok(ProcessCreateDisposition::ReconcileOnly);
        }
        if record.encrypted_context != snapshot.encrypted_context
            || record.source_epochs.0 != epochs
        {
            return Err(denied());
        }
        fresh_on(&mut tx, proof.deadline).await?;
        sqlx::query("UPDATE execution_process_effect SET create_submitted=TRUE WHERE intent=$1")
            .bind(intent)
            .execute(&mut *tx)
            .await?;
        fresh_on(&mut tx, proof.deadline).await?;
        tx.commit().await?;
        fresh(proof.deadline)?;
        Ok(ProcessCreateDisposition::CreateOnce)
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
        let mut proof = Self::source(&mut tx, &context.target, context.owner).await?;
        let expected = context.input.retained_snapshot();
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
        if expiry.frozen_frontier()? != context.input_frontier {
            return Err(denied());
        }
        proof.include_input(&context.input_frontier);
        let payload = Self::action_stdin(&mut tx, &context, master).await?;
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
            || !record.create_submitted
            || record.stdin_consumed
            || record.stop_requested
            || record.exit_outcome.is_some()
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
        LaunchStdinPayload::from_bytes(payload)
    }

    /// Record only the exact native resource's terminal outcome. It closes
    /// runtime API effects while capture/publication may still be finalizing.
    pub async fn record_exit(
        pool: &PgPool,
        intent: Uuid,
        resource: &ResourceIdentity,
        outcome: LaunchExitOutcome,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR UPDATE"
        ))
        .bind(intent)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(denied)?;
        if record.resource.as_ref().map(|value| &value.0) != Some(resource)
            || !record.create_submitted
            || record
                .exit_outcome
                .as_ref()
                .is_some_and(|value| value.0 != outcome)
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

    /// Hold native attempt and publication fences through the caller's output
    /// protection and terminal transition transaction. A historical committed
    /// terminal transition is acknowledged without replaying result writes.
    pub async fn lock_completion(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        intent: Uuid,
        master: &str,
    ) -> Result<Option<super::execution::ExecutionTransition>> {
        use super::{execution::ExecutionRepository, FindById};
        use crate::models::ExecutionStatus;
        let snapshot: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1"
        ))
        .bind(intent)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        let context = snapshot.context(master)?;
        if context.purpose != LaunchPurpose::Action {
            return Err(denied());
        }
        let LaunchTarget::Execution {
            execution,
            worker,
            attempt,
            ..
        } = context.target
        else {
            return Err(denied());
        };
        // Transaction-owned serialization precedes all source locks. It avoids
        // two completion callers both holding shared source locks while trying
        // to upgrade the same execution row. No session lock spans COMMIT.
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('execution-process-completion',$1))",
        )
        .bind(execution)
        .execute(&mut **tx)
        .await?;
        let current = ExecutionRepository::find_by_id(&mut **tx, execution)
            .await?
            .ok_or_else(denied)?;
        let terminal = matches!(
            current.status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Timeout
                | ExecutionStatus::Cancelled
        );
        if terminal {
            let current = ExecutionRepository::find_by_id_for_update(tx, execution)
                .await?
                .ok_or_else(denied)?;
            let mode =
            super::execution_runtime_admission::ExecutionAdmissionRepository::selected_authority(
                tx, execution,
            )
            .await?;
            let same_attempt: bool = match mode {
            super::execution_runtime_admission::ExistingExecutionAuthority::Delegated => {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM delegated_execution_attempt WHERE execution=$1 AND worker=$2 AND attempt=$3)")
                    .bind(execution).bind(worker).bind(attempt).fetch_one(&mut **tx).await?
            }
            super::execution_runtime_admission::ExistingExecutionAuthority::Recorded => {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_authority_execution_fence WHERE execution=$1 AND attempt=$2)")
                    .bind(execution).bind(attempt).fetch_one(&mut **tx).await?
            }
        };
            let record: Record = sqlx::query_as(&format!(
                "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR UPDATE"
            ))
            .bind(intent)
            .fetch_one(&mut **tx)
            .await?;
            if current.worker != Some(worker)
                || !same_attempt
                || !record.stdin_consumed
                || record.resource.is_none()
                || record.exit_outcome.is_none()
                || record.encrypted_context != snapshot.encrypted_context
            {
                return Err(denied());
            }
            return Ok(Some(super::execution::ExecutionTransition {
                previous_status: current.status,
                execution: current,
            }));
        }
        let mut proof = Self::source_for(tx, &context.target, context.owner, true).await?;
        context.input_frontier.retain(tx).await?;
        proof.include_input(&context.input_frontier);
        // Take the mutable execution source lock BEFORE epoch/operation locks.
        // Input callers can finish without waiting behind an operation whose
        // owner is itself waiting to upgrade their shared execution lock.
        ExecutionRepository::find_by_id_for_update(tx, execution)
            .await?
            .ok_or_else(denied)?;
        proof.sources.push(("execution_input", execution));
        let epochs = workload_source::snapshot(tx, proof.sources).await?;
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE intent=$1 FOR UPDATE"
        ))
        .bind(intent)
        .fetch_one(&mut **tx)
        .await?;
        if !record.stdin_consumed
            || record.resource.is_none()
            || record.exit_outcome.is_none()
            || record.encrypted_context != snapshot.encrypted_context
            || record.source_epochs.0 != epochs
        {
            return Err(denied());
        }
        fresh_on(tx, proof.deadline).await?;
        Ok(None)
    }

    /// Retain the exact frozen plan for trusted recovery; this is not execution
    /// authorization. Recovery must consume the same source/attempt gate.
    pub async fn retained_for_attempt(
        pool: &PgPool,
        execution: i64,
        attempt: Uuid,
        master: &str,
    ) -> Result<Option<ProcessSpec>> {
        Self::retained_for_purpose(pool, execution, attempt, LaunchPurpose::Action, master).await
    }

    pub async fn retained_for_purpose(
        pool: &PgPool,
        execution: i64,
        attempt: Uuid,
        purpose: LaunchPurpose,
        master: &str,
    ) -> Result<Option<ProcessSpec>> {
        let record: Option<Record> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM execution_process_effect WHERE target->>'kind'='execution' \
             AND (target->>'execution')::BIGINT=$1 AND target->>'attempt'=$2 AND purpose=$3"
        ))
        .bind(execution)
        .bind(attempt.to_string())
        .bind(purpose_name(purpose)?)
        .fetch_optional(pool)
        .await?;
        record
            .map(|value| value.context(master).map(|context| context.spec()))
            .transpose()
    }

    /// Exact frozen material only, with no fresh disclosure or start authority.
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
    /// Exact provider evidence for trusted cleanup. Absence is not a provider
    /// settlement receipt; callers retain uncertain creation/publication.
    pub async fn retained_resource(
        pool: &PgPool,
        intent: Uuid,
    ) -> Result<Option<ResourceIdentity>> {
        let resource: Option<Option<Json<ResourceIdentity>>> =
            sqlx::query_scalar("SELECT resource FROM execution_process_effect WHERE intent=$1")
                .bind(intent)
                .fetch_optional(pool)
                .await?;
        Ok(resource.ok_or_else(denied)?.map(|value| value.0))
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
