//! Trusted control-plane launch ledger. No provider effect is authorized without
//! source -> epoch -> operation admission in an owning SQLx transaction. Public
//! requests select only a previously frozen launch; they cannot supply a plan.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    delegated_execution::DelegatedExecutionRepository,
    execution_input::{
        ExecutionInputRepository, ExecutionInputSnapshot, FrozenInputSnapshot,
        SelectedExecutionInput,
    },
    execution_runtime_admission::{ExecutionAdmissionRepository, ExistingExecutionAuthority},
    managed_sensor_metadata::ManagedSensorMetadataRepository,
    runtime_authority::{RuntimeAuthorityRepository, RuntimeCredentialBinding, RuntimeOwner},
    sensor_workload::SensorWorkloadRepository,
    workload_source::{self, SourceEpoch},
};
use crate::{
    crypto, models::SensorWorkloadFence, workload_launch::*,
    workload_launch_state::LaunchOperation, Error, Result,
};

mod artifact;
mod host;
mod materialization;
mod output;
mod stdin;
pub use artifact::ArtifactAllocation;
pub use materialization::{PackInputReservation, PublicInputCleanup};
pub use output::{LaunchOutputCleanup, OutputBodyPermit};

const COLUMNS: &str = "id,engine,image,owner,manifest,admitted_digest,encrypted_context,source_epochs,operation,dispatch_hash,lease_hash,controller";
fn denied() -> Error {
    Error::PermissionDenied("Workload launch unavailable".into())
}

#[cfg(test)]
mod selected_input_epoch_tests {
    use super::*;
    #[test]
    fn input_mutation_fences_run_without_reclassifying_retained_capture() {
        let source = |kind: &str, epoch| SourceEpoch {
            kind: kind.into(),
            subject: 1,
            epoch,
        };
        let admitted = vec![source("execution_input", 0), source("owner", 0)];
        let input_changed = vec![source("execution_input", 2), source("owner", 0)];
        assert!(!WorkloadLaunchRepository::same_epochs(
            &input_changed,
            &admitted,
            false
        ));
        assert!(WorkloadLaunchRepository::same_epochs(
            &input_changed,
            &admitted,
            true
        ));
        let owner_revoked = vec![source("execution_input", 2), source("owner", 1)];
        assert!(!WorkloadLaunchRepository::same_epochs(
            &owner_revoked,
            &admitted,
            true
        ));
    }
}
fn same_input(left: Option<&str>, right: Option<&str>) -> Result<bool> {
    match (left, right) {
        (None, None) => Ok(true),
        (Some(left), Some(right)) => Ok(serde_json::from_str::<serde_json::Value>(left)
            .map_err(|_| denied())?
            == serde_json::from_str::<serde_json::Value>(right).map_err(|_| denied())?),
        _ => Ok(false),
    }
}
fn credential_hash(token: &LaunchToken) -> String {
    Sha256Digest::of_bytes(token.as_str().as_bytes())
        .as_str()
        .into()
}
fn new_token() -> Result<LaunchToken> {
    use base64::Engine;
    LaunchToken::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crypto::random_bytes::<32>()),
    )
}
#[derive(sqlx::FromRow)]
struct Record {
    id: i64,
    engine: i64,
    image: i64,
    owner: i64,
    manifest: Json<LaunchManifest>,
    admitted_digest: String,
    encrypted_context: String,
    source_epochs: Json<Vec<SourceEpoch>>,
    operation: Json<LaunchOperation>,
    dispatch_hash: String,
    lease_hash: Option<String>,
    controller: Option<i64>,
    #[sqlx(skip)]
    effect_deadline: Option<DateTime<Utc>>,
}
#[derive(sqlx::FromRow)]
struct ReservationReplay {
    manifest: Json<LaunchManifest>,
    encrypted_context: String,
    source_epochs: Json<Vec<SourceEpoch>>,
    encrypted_dispatch: String,
    admitted_digest: String,
}
/// Validated identity supplied only by the dedicated RequireAuth consumer. DB
/// binding is rechecked inside each transaction, including cleanup operations.
#[derive(Clone, Copy)]
pub struct ControllerActor {
    pub controller: i64,
    pub identity: i64,
    pub engine: i64,
    pub incarnation: Uuid,
    pub expires_at: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenContext {
    launch_id: i64,
    owner: i64,
    purpose: LaunchPurpose,
    target: LaunchTarget,
    invocation: LaunchInvocation,
    // Private selected input is authenticated by the randomized ciphertext
    // reference. It is never embedded in a public input archive or plan hash.
    stdin_json: Option<String>,
    selected_input: Option<FrozenInputSnapshot>,
}
struct PrivateInput {
    stdin_json: Option<String>,
    snapshot: Option<ExecutionInputSnapshot>,
}
impl PrivateInput {
    fn unbound(stdin_json: Option<String>) -> Self {
        Self {
            stdin_json,
            snapshot: None,
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct LaunchInputBody {
    pub object_key: String,
    pub provider_version: String,
    pub size_bytes: i64,
    pub sha256: String,
}

/// Trusted planner snapshot, not a host-supplied cache selector. Configuration
/// hashes random-nonce ciphertext; runtime definitions and inputs are public pins.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStageBinding {
    pub engine_id: i64,
    pub platform: LaunchPlatform,
    pub image_sha256: Sha256Digest,
    pub pack_sha256: Sha256Digest,
    pub configuration_ciphertext_sha256: Sha256Digest,
    pub runtime_definition_sha256: Sha256Digest,
}
#[derive(sqlx::FromRow)]
struct ActionAdmission {
    id: i64,
    source_target: Json<LaunchTarget>,
    image: i64,
    source_epochs: Json<Vec<SourceEpoch>>,
    artifact_limits: Json<crate::workload_artifact::ArtifactLimits>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}
#[derive(sqlx::FromRow)]
struct PrerequisiteSource {
    source_target: Json<LaunchTarget>,
    source_epochs: Json<Vec<SourceEpoch>>,
    image: i64,
    expires_at: DateTime<Utc>,
}
pub(crate) struct SourceProof {
    pub(crate) sources: Vec<(&'static str, i64)>,
    pub(crate) deadline: Option<DateTime<Utc>>,
    retained: Option<Vec<SourceEpoch>>,
    image: Option<i64>,
}

pub struct WorkloadLaunchRepository;
impl WorkloadLaunchRepository {
    /// Immutable engine/platform proof. Cleanup may read a disabled engine's
    /// identity; disabling admission must never make owned resources unremovable.
    pub async fn engine_binding(
        pool: &PgPool,
        actor: ControllerActor,
    ) -> Result<LaunchEngineBinding> {
        let mut tx = pool.begin().await?;
        Self::actor(&mut tx, actor).await?;
        let (daemon_id, platform): (String, String) =
            sqlx::query_as("SELECT daemon_id,platform FROM workload_engine WHERE id=$1 FOR SHARE")
                .bind(actor.engine)
                .fetch_one(&mut *tx)
                .await?;
        let result = LaunchEngineBinding {
            engine_id: actor.engine,
            daemon_id,
            platform: serde_json::from_value(serde_json::Value::String(platform))?,
        };
        result.validate_shape()?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// A current, persisted controller renews its dedicated machine session.
    /// Source row locks cover signing; caller expiry is checked after commit as
    /// well, before any new credential can leave the trusted control plane.
    pub async fn renew_session(
        pool: &PgPool,
        actor: ControllerActor,
        jwt: &crate::auth::jwt::JwtConfig,
    ) -> Result<crate::workload_session::WorkloadSession> {
        let mut tx = pool.begin().await?;
        Self::actor(&mut tx, actor).await?;
        let expires_at =
            chrono::DateTime::from_timestamp(Utc::now().timestamp() + 300, 0).ok_or_else(denied)?;
        if actor.expires_at <= Self::now(&mut tx).await?.timestamp() {
            return Err(denied());
        }
        let token = LaunchToken::new(crate::workload_controller::mint(
            ControllerActor {
                expires_at: expires_at.timestamp(),
                ..actor
            },
            jwt,
        )?)?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        if actor.expires_at <= Utc::now().timestamp() {
            return Err(denied());
        }
        Ok(crate::workload_session::WorkloadSession { token, expires_at })
    }
    async fn now(tx: &mut Transaction<'_, Postgres>) -> Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **tx)
            .await?)
    }
    async fn actor(tx: &mut Transaction<'_, Postgres>, actor: ControllerActor) -> Result<()> {
        super::schema_type::SchemaTypeRepository::lock_selection(tx).await?;
        let valid: Option<i64> = sqlx::query_scalar("SELECT c.id FROM workload_controller c JOIN identity i ON i.id=c.identity JOIN workload_engine e ON e.id=c.engine WHERE c.id=$1 AND c.identity=$2 AND c.engine=$3 AND c.incarnation=$4 AND c.revoked_at IS NULL AND NOT i.frozen AND $5>extract(epoch FROM clock_timestamp()) FOR SHARE OF c,i")
            .bind(actor.controller).bind(actor.identity).bind(actor.engine).bind(actor.incarnation).bind(actor.expires_at).fetch_optional(&mut **tx).await?;
        if valid.is_none() {
            return Err(denied());
        }
        Ok(())
    }
    async fn record(
        tx: &mut Transaction<'_, Postgres>,
        request: &LaunchRequest,
        lock: bool,
    ) -> Result<Record> {
        if request.launch_id <= 0 {
            return Err(denied());
        }
        let suffix = if lock { " FOR UPDATE" } else { "" };
        let record: Record = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM workload_launch WHERE id=$1 AND admitted_digest=$2{suffix}"
        ))
        .bind(request.launch_id)
        .bind(request.admitted_digest.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        if record.manifest.admitted_digest()? != request.admitted_digest
            || record.operation.status.request != *request
            || record.engine != record.manifest.engine_id
            || record.owner != record.manifest.owner_identity_id
            || record.id != request.launch_id
            || record.admitted_digest != request.admitted_digest.as_str()
        {
            return Err(denied());
        }
        Ok(record)
    }
    async fn save(tx: &mut Transaction<'_, Postgres>, record: &Record) -> Result<()> {
        record.operation.status.validate_shape()?;
        let now = Self::now(tx).await?;
        if (!record.operation.cleanup_only() || record.operation.publication_pending())
            && record
                .effect_deadline
                .is_some_and(|deadline| deadline <= now)
        {
            return Err(denied());
        }
        sqlx::query(
            "UPDATE workload_launch SET operation=$2,lease_hash=$3,controller=$4 WHERE id=$1",
        )
        .bind(record.id)
        .bind(&record.operation)
        .bind(&record.lease_hash)
        .bind(record.controller)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
    /// Live authority is selected through the recorded mode, never inferred from
    /// ownership or borrowed from a missing/revoked managed admission.
    pub(crate) async fn source(
        tx: &mut Transaction<'_, Postgres>,
        target: &LaunchTarget,
        expected_owner: i64,
    ) -> Result<SourceProof> {
        super::schema_type::SchemaTypeRepository::lock_selection(tx).await?;
        Self::source_for(tx, target, expected_owner, false).await
    }
    async fn source_for(
        tx: &mut Transaction<'_, Postgres>,
        target: &LaunchTarget,
        expected_owner: i64,
        allow_canceling: bool,
    ) -> Result<SourceProof> {
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
                            RuntimeAuthorityRepository::validate_execution_credential(
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
                    if execution_deadline <= Self::now(tx).await? {
                        return Err(denied());
                    }
                }
                Self::worker(tx, *worker, *worker_instance).await?;
                let mut sources = vec![
                    ("owner", expected_owner),
                    ("execution", *execution),
                    ("worker", *worker),
                ];
                if !allow_canceling {
                    sources.push(("execution_input", *execution));
                }
                Ok(SourceProof {
                    sources,
                    deadline,
                    retained: None,
                    image: None,
                })
            }
            LaunchTarget::Sensor {
                sensor,
                workload,
                worker,
                worker_instance,
                generation,
            } => {
                let fence = SensorWorkloadFence {
                    workload_id: *workload,
                    worker_id: *worker,
                    worker_instance: *worker_instance,
                    generation: *generation,
                };
                if !SensorWorkloadRepository::lock_current_fence(tx, *sensor, fence).await? {
                    return Err(denied());
                }
                let metadata =
                    ManagedSensorMetadataRepository::for_generation(tx, *workload, *generation)
                        .await?
                        .ok_or_else(denied)?;
                if metadata.owner.sponsor_identity_id != expected_owner {
                    return Err(denied());
                }
                Self::worker(tx, *worker, *worker_instance).await?;
                SensorWorkloadRepository::recheck_locked_fence(tx, *sensor, fence).await?;
                let deadline = sqlx::query_scalar(
                    "SELECT lease_expires_at FROM sensor_workload_assignment WHERE workload=$1",
                )
                .bind(workload)
                .fetch_one(&mut **tx)
                .await?;
                Ok(SourceProof {
                    sources: vec![
                        ("owner", expected_owner),
                        ("workload", *workload),
                        ("sensor", *sensor),
                        ("worker", *worker),
                    ],
                    deadline,
                    retained: None,
                    image: None,
                })
            }
            LaunchTarget::CandidateTest {
                install,
                test_job,
                attempt,
            } => {
                let valid: Option<i64>=sqlx::query_scalar("SELECT c.id FROM pack_install p JOIN pack_candidate_context c ON c.install_id=p.id JOIN identity i ON i.id=p.requested_by WHERE p.id=$1 AND c.id=$2 AND c.lease=$3 AND p.status='running' AND NOT c.revoked AND c.expires_at>clock_timestamp() AND p.assigned_worker_id=c.worker_id AND c.archive_digest=p.candidate_digest AND c.archive_version=p.candidate_provider_version AND c.archive_key=p.candidate_object_key AND p.requested_by=$4 AND NOT i.frozen FOR SHARE OF p,c,i")
                    .bind(install).bind(test_job).bind(attempt).bind(expected_owner).fetch_optional(&mut **tx).await?;
                if valid.is_none() {
                    return Err(denied());
                }
                let deadline = Some(
                    sqlx::query_scalar("SELECT expires_at FROM pack_candidate_context WHERE id=$1")
                        .bind(test_job)
                        .fetch_one(&mut **tx)
                        .await?,
                );
                Ok(SourceProof {
                    sources: vec![("owner", expected_owner), ("install", *install)],
                    deadline,
                    retained: None,
                    image: None,
                })
            }
            LaunchTarget::Setup { admission, attempt }
            | LaunchTarget::RuntimeProbe { admission, attempt } => {
                let purpose = if matches!(target, LaunchTarget::Setup { .. }) {
                    "setup"
                } else {
                    "runtime_probe"
                };
                // Resolve immutable source identity without taking an admission
                // lock ahead of its owner/execution/worker source locks.
                let source: Option<(Json<LaunchTarget>,Json<Vec<SourceEpoch>>,i64)>=sqlx::query_as("SELECT source_target,source_epochs,image FROM workload_launch_prerequisite WHERE id=$1 AND attempt=$2 AND purpose=$3 AND revoked_at IS NULL AND ($4 OR expires_at>clock_timestamp())")
                    .bind(admission).bind(attempt).bind(purpose).bind(allow_canceling).fetch_optional(&mut **tx).await?;
                let (source, epochs, image) = source.ok_or_else(denied)?;
                let source = source.0;
                if matches!(
                    source,
                    LaunchTarget::Setup { .. } | LaunchTarget::RuntimeProbe { .. }
                ) {
                    return Err(denied());
                }
                let mut sources = Box::pin(Self::source_for(
                    tx,
                    &source,
                    expected_owner,
                    allow_canceling,
                ))
                .await?;
                let locked: Option<PrerequisiteSource> = sqlx::query_as(
                    "SELECT source_target,source_epochs,image,expires_at FROM workload_launch_prerequisite WHERE id=$1 AND attempt=$2 AND purpose=$3 AND revoked_at IS NULL AND ($4 OR expires_at>clock_timestamp()) FOR SHARE",
                ).bind(admission).bind(attempt).bind(purpose).bind(allow_canceling).fetch_optional(&mut **tx).await?;
                let locked = locked.ok_or_else(denied)?;
                let deadline = locked.expires_at;
                if locked.source_target.0 != source
                    || locked.source_epochs.0 != epochs.0
                    || locked.image != image
                {
                    return Err(denied());
                }
                if !allow_canceling {
                    sources.deadline = Some(
                        sources
                            .deadline
                            .map_or(deadline, |existing| existing.min(deadline)),
                    );
                }
                sources.sources.push(("admission", *admission));
                sources.retained = Some(epochs.0);
                sources.image = Some(image);
                Ok(sources)
            }
        }
    }
    async fn worker(tx: &mut Transaction<'_, Postgres>, worker: i64, instance: Uuid) -> Result<()> {
        // Execution registration's incarnation is distinct from log-delivery's
        // restart-persistent source instance. Never interchange the two fences.
        let current: Option<Uuid> = sqlx::query_scalar(
            "SELECT (meta->>'registration_incarnation')::UUID FROM worker WHERE id=$1 AND status IN ('active','busy') AND last_heartbeat>clock_timestamp()-interval '90 seconds' FOR SHARE",
        )
        .bind(worker)
        .fetch_optional(&mut **tx)
        .await?;
        if current != Some(instance) {
            return Err(denied());
        }
        Ok(())
    }
    fn retained_epochs(
        proof: &SourceProof,
        current: &[SourceEpoch],
        image: i64,
        capture: bool,
    ) -> Result<()> {
        if let Some(retained) = &proof.retained {
            let required: Vec<_> = proof
                .sources
                .iter()
                .filter(|(kind, _)| *kind != "admission")
                .collect();
            if retained
                .iter()
                .filter(|entry| !capture || entry.kind != "execution_input")
                .count()
                != required.len()
                || required.iter().any(|(kind, subject)| {
                    !retained
                        .iter()
                        .any(|epoch| epoch.kind == *kind && epoch.subject == *subject)
                })
            {
                return Err(denied());
            }
        }
        if proof.image.is_some_and(|expected| expected != image)
            || proof.retained.as_ref().is_some_and(|retained| {
                retained
                    .iter()
                    .filter(|expected| {
                        !capture
                            || !matches!(
                                expected.kind.as_str(),
                                "image" | "engine" | "execution_input"
                            )
                    })
                    .any(|expected| !current.contains(expected))
            })
        {
            return Err(denied());
        }
        Ok(())
    }
    async fn epochs(
        tx: &mut Transaction<'_, Postgres>,
        record: &Record,
    ) -> Result<(Vec<SourceEpoch>, Option<DateTime<Utc>>)> {
        Self::epochs_for(tx, record, false).await
    }
    async fn epochs_for(
        tx: &mut Transaction<'_, Postgres>,
        record: &Record,
        capture: bool,
    ) -> Result<(Vec<SourceEpoch>, Option<DateTime<Utc>>)> {
        super::schema_type::SchemaTypeRepository::lock_selection(tx).await?;
        let mut sources =
            Self::source_for(tx, &record.manifest.target, record.owner, capture).await?;
        if !capture {
            let image:Option<i64>=sqlx::query_scalar("SELECT i.id FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine WHERE i.id=$1 AND i.engine=$2 AND i.sha256=$3 AND i.reference=$4 AND i.revoked_at IS NULL AND NOT e.disabled AND i.platform=e.platform FOR SHARE OF i,e")
            .bind(record.image).bind(record.engine).bind(record.manifest.image.sha256.as_str()).bind(&record.manifest.image.reference).fetch_optional(&mut **tx).await?;
            if image.is_none() {
                return Err(denied());
            }
        }
        sources.sources.push(("image", record.image));
        sources.sources.push(("engine", record.engine));
        for expected in record
            .source_epochs
            .iter()
            .filter(|epoch| epoch.kind == "admission")
        {
            if sources.sources.contains(&("admission", expected.subject)) {
                continue;
            }
            let root:ActionAdmission=sqlx::query_as("SELECT id,source_target,image,source_epochs,artifact_limits,expires_at,revoked_at FROM workload_launch_prerequisite WHERE id=$1 AND purpose='action' FOR SHARE")
                .bind(expected.subject).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
            let target = match &record.manifest.target {
                LaunchTarget::Setup { admission, .. }
                | LaunchTarget::RuntimeProbe { admission, .. } => {
                    sqlx::query_scalar::<_, Json<LaunchTarget>>(
                        "SELECT source_target FROM workload_launch_prerequisite WHERE id=$1",
                    )
                    .bind(admission)
                    .fetch_one(&mut **tx)
                    .await?
                    .0
                }
                target => target.clone(),
            };
            if root.source_target.0 != target
                || root.image != record.image
                || root.revoked_at.is_some()
                || (!capture && root.expires_at <= Self::now(tx).await?)
            {
                return Err(denied());
            }
            sources.sources.push(("admission", root.id));
        }
        let epochs = workload_source::snapshot(tx, sources.sources.clone()).await?;
        Self::retained_epochs(&sources, &epochs, record.image, capture)?;
        Ok((epochs, sources.deadline))
    }
    fn same_epochs(current: &[SourceEpoch], expected: &[SourceEpoch], capture: bool) -> bool {
        current
            .iter()
            .filter(|entry| {
                !capture || !matches!(entry.kind.as_str(), "image" | "engine" | "execution_input")
            })
            .eq(expected.iter().filter(|entry| {
                !capture || !matches!(entry.kind.as_str(), "image" | "engine" | "execution_input")
            }))
    }
    /// Acquire sources and epochs before locking the operation. Permission loss
    /// closes the operation durably; cleanup may proceed without private context.
    async fn admitted(
        tx: &mut Transaction<'_, Postgres>,
        actor: ControllerActor,
        request: &LaunchRequest,
    ) -> Result<Record> {
        Self::actor(tx, actor).await?;
        let snapshot = Self::record(tx, request, false).await?;
        if snapshot.engine != actor.engine {
            return Err(denied());
        }
        let capture =
            snapshot.operation.cleanup_only() && !snapshot.operation.publication_pending();
        let mut effect_deadline = None;
        let admitted = if snapshot.operation.cleanup_only()
            && !snapshot.operation.publication_pending()
            && !snapshot.operation.capture_pending()
        {
            false
        } else {
            match Self::epochs_for(tx, &snapshot, capture).await {
                Ok((epochs, deadline)) => {
                    effect_deadline = deadline;
                    Self::same_epochs(&epochs, &snapshot.source_epochs.0, capture)
                }
                Err(Error::PermissionDenied(_)) => false,
                Err(error) => return Err(error),
            }
        };
        let mut record = Self::record(tx, request, true).await?;
        record.effect_deadline = effect_deadline;
        record.operation.expire_capture(Self::now(tx).await?);
        let now = Self::now(tx).await?;
        if record
            .effect_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            record
                .operation
                .stop_at(LaunchStopReason::SourceRevoked, Self::now(tx).await?);
            if capture && record.operation.capture_pending() {
                record.operation.status.capture = LaunchCaptureState::Incomplete;
            }
        }
        if !admitted
            && (!record.operation.cleanup_only()
                || record.operation.publication_pending()
                || record.operation.capture_pending())
        {
            record
                .operation
                .stop_at(LaunchStopReason::SourceRevoked, Self::now(tx).await?);
            if capture && record.operation.capture_pending() {
                record.operation.status.capture = LaunchCaptureState::Incomplete;
            }
        }
        if record.operation.lease_generation > 0
            && record.operation.lease_deadline <= Self::now(tx).await?
            && !record.operation.cleanup_only()
        {
            record
                .operation
                .stop_at(LaunchStopReason::LeaseExpired, now);
        }
        Ok(record)
    }
    fn require_lease(
        record: &Record,
        actor: ControllerActor,
        handle: &LaunchHandle,
        token: &LaunchToken,
        now: DateTime<Utc>,
    ) -> Result<()> {
        record.operation.require_handle(handle, now)?;
        if record.controller != Some(actor.controller)
            || record.lease_hash.as_deref() != Some(credential_hash(token).as_str())
        {
            return Err(denied());
        }
        Ok(())
    }
    fn private_context(record: &Record, master: &str) -> Result<FrozenContext> {
        if record.manifest.context.id != record.id
            || record.manifest.context.version != 1
            || record.manifest.context.ciphertext_sha256
                != Sha256Digest::of_bytes(record.encrypted_context.as_bytes())
        {
            return Err(denied());
        }
        let plaintext = crypto::decrypt(&record.encrypted_context, master).map_err(|_| denied())?;
        let context: FrozenContext = serde_json::from_str(&plaintext).map_err(|_| denied())?;
        if context.launch_id != record.id
            || context.owner != record.owner
            || context.purpose != record.manifest.purpose
            || context.target != record.manifest.target
        {
            return Err(denied());
        }
        Ok(context)
    }
    fn invocation(record: &Record, master: &str) -> Result<LaunchInvocation> {
        Ok(Self::private_context(record, master)?.invocation)
    }
    /// Reserve an execution/worker-incarnation admission before any asynchronous
    /// preparation. A retry may recover this exact record, never rebind it after
    /// intervening source revocation or reuse it for a replacement worker.
    pub async fn reserve_action_admission(
        pool: &PgPool,
        target: &LaunchTarget,
        owner: i64,
        image: i64,
    ) -> Result<i64> {
        let mut tx = pool.begin().await?;
        let id = Self::reserve_action_admission_in(
            &mut tx,
            target,
            owner,
            image,
            crate::workload_artifact::ArtifactLimits::default(),
        )
        .await?;
        tx.commit().await?;
        Ok(id)
    }
    pub(crate) async fn reserve_action_admission_in(
        tx: &mut Transaction<'_, Postgres>,
        target: &LaunchTarget,
        owner: i64,
        image: i64,
        artifact_limits: crate::workload_artifact::ArtifactLimits,
    ) -> Result<i64> {
        crate::workload_artifact::validate_inventory(&[], artifact_limits)?;
        let attempt = match target {
            LaunchTarget::Execution { attempt, .. } => *attempt,
            _ => return Err(denied()),
        };
        let mut proof = Self::source(tx, target, owner).await?;
        let engine:Option<i64>=sqlx::query_scalar("SELECT i.engine FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine WHERE i.id=$1 AND i.revoked_at IS NULL AND NOT e.disabled AND i.platform=e.platform FOR SHARE OF i,e")
            .bind(image).fetch_optional(&mut **tx).await?;
        proof.sources.push(("image", image));
        proof.sources.push(("engine", engine.ok_or_else(denied)?));
        let epochs = workload_source::snapshot(tx, proof.sources).await?;
        let expires = proof.deadline.ok_or_else(denied)?;
        if expires <= Self::now(tx).await? {
            return Err(denied());
        }
        sqlx::query("INSERT INTO workload_launch_prerequisite(purpose,attempt,source_target,image,source_epochs,expires_at,artifact_limits) VALUES('action',$1,$2,$3,$4,$5,$6) ON CONFLICT(source_target) WHERE purpose='action' DO NOTHING")
            .bind(attempt).bind(Json(target)).bind(image).bind(Json(&epochs)).bind(expires).bind(Json(artifact_limits)).execute(&mut **tx).await?;
        let admitted:ActionAdmission=sqlx::query_as("SELECT id,source_target,image,source_epochs,artifact_limits,expires_at,revoked_at FROM workload_launch_prerequisite WHERE purpose='action' AND source_target=$1 FOR SHARE")
            .bind(Json(target)).fetch_one(&mut **tx).await?;
        if admitted.image != image
            || admitted.artifact_limits.0 != artifact_limits
            || admitted.source_epochs.0 != epochs
            || admitted.revoked_at.is_some()
            || admitted.expires_at <= Self::now(tx).await?
        {
            return Err(denied());
        }
        Ok(admitted.id)
    }

    /// One Setup admission covers the entire selected setup sequence in a
    /// single container; only its settled successful exit can publish a stage.
    /// Probe admissions retain the same immutable source and image boundary.
    pub async fn prerequisite(
        pool: &PgPool,
        target: &LaunchTarget,
        owner: i64,
        image: i64,
        purpose: LaunchPurpose,
        binding: Option<RuntimeStageBinding>,
        expires_at: DateTime<Utc>,
    ) -> Result<LaunchTarget> {
        if !matches!(purpose, LaunchPurpose::Setup | LaunchPurpose::RuntimeProbe)
            || matches!(
                target,
                LaunchTarget::Setup { .. } | LaunchTarget::RuntimeProbe { .. }
            )
        {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        let mut proof = Self::source(&mut tx, target, owner).await?;
        let (image_digest, engine_id, platform):(String,i64,String)=sqlx::query_as("SELECT i.sha256,i.engine,e.platform FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine WHERE i.id=$1 AND i.revoked_at IS NULL AND NOT e.disabled AND i.platform=e.platform FOR SHARE OF i,e").bind(image).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
        if purpose == LaunchPurpose::Setup {
            let binding = binding.as_ref().ok_or_else(denied)?;
            if binding.image_sha256.as_str() != image_digest
                || binding.engine_id != engine_id
                || binding.platform.as_str() != platform
            {
                return Err(denied());
            }
            let pack:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_launch_object WHERE source_target=$1 AND kind='pack' AND sha256=$2 AND retired_at IS NULL)").bind(Json(target)).bind(binding.pack_sha256.as_str()).fetch_one(&mut *tx).await?;
            if !pack {
                return Err(denied());
            }
        }
        proof.sources.push(("image", image));
        proof.sources.push(("engine", engine_id));
        let epochs = workload_source::snapshot(&mut tx, proof.sources).await?;
        let now = Self::now(&mut tx).await?;
        if expires_at <= now || expires_at > now + chrono::Duration::hours(1) {
            return Err(denied());
        }
        let attempt = Uuid::new_v4();
        let name = if purpose == LaunchPurpose::Setup {
            "setup"
        } else {
            "runtime_probe"
        };
        let id:i64=sqlx::query_scalar("INSERT INTO workload_launch_prerequisite(purpose,attempt,source_target,image,source_epochs,stage_binding,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(name).bind(attempt).bind(Json(target)).bind(image).bind(Json(epochs)).bind(binding.as_ref().map(Json)).bind(expires_at).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(if purpose == LaunchPurpose::Setup {
            LaunchTarget::Setup {
                admission: id,
                attempt,
            }
        } else {
            LaunchTarget::RuntimeProbe {
                admission: id,
                attempt,
            }
        })
    }
    /// Called by trusted CP planners after materializing the exact admitted
    /// archive/context. This is deliberately not a raw-spec HTTP endpoint.
    /// Generated IDs/nonces/context pins override any placeholders in the plan.
    pub async fn freeze(
        pool: &PgPool,
        image_id: i64,
        spec: LaunchSpec,
        master: &str,
    ) -> Result<ReservedLaunch> {
        Self::freeze_with_runtime_binding(pool, image_id, spec, master, None).await
    }
    pub async fn freeze_with_runtime_binding(
        pool: &PgPool,
        image_id: i64,
        spec: LaunchSpec,
        master: &str,
        expected_runtime: Option<&RuntimeStageBinding>,
    ) -> Result<ReservedLaunch> {
        Self::freeze_with_input_binding(pool, image_id, spec, master, expected_runtime, None).await
    }
    /// Trusted producer only. The selected stdin (including a Setup helper
    /// plan) is part of the same immutable encrypted context as the invocation.
    /// Later ready-input issuance must freshly authorize its recorded sources.
    pub async fn freeze_with_input_binding(
        pool: &PgPool,
        image_id: i64,
        spec: LaunchSpec,
        master: &str,
        expected_runtime: Option<&RuntimeStageBinding>,
        stdin_json: Option<String>,
    ) -> Result<ReservedLaunch> {
        Self::freeze_materialized(
            pool,
            image_id,
            spec,
            master,
            expected_runtime,
            PrivateInput::unbound(stdin_json),
            None,
        )
        .await
    }
    /// Product action producers must retain the exact pre-preparation admission.
    /// It also fences setup/probe child preparation through their recorded root.
    pub async fn freeze_admitted_input(
        pool: &PgPool,
        admission: i64,
        image_id: i64,
        spec: LaunchSpec,
        master: &str,
        expected_runtime: Option<&RuntimeStageBinding>,
        stdin_json: Option<String>,
    ) -> Result<ReservedLaunch> {
        Self::freeze_materialized(
            pool,
            image_id,
            spec,
            master,
            expected_runtime,
            PrivateInput::unbound(stdin_json),
            Some(admission),
        )
        .await
    }
    /// Product action producer: bind original selected input AND recorded
    /// origins inside the same randomized encrypted context. No host-submitted
    /// plan or plaintext input digest is accepted.
    pub async fn freeze_selected_execution(
        pool: &PgPool,
        admission: i64,
        image_id: i64,
        spec: LaunchSpec,
        master: &str,
        expected_runtime: Option<&RuntimeStageBinding>,
        selected: SelectedExecutionInput,
    ) -> Result<ReservedLaunch> {
        if spec.manifest.purpose != LaunchPurpose::Action
            || !matches!(spec.manifest.target, LaunchTarget::Execution { execution, .. } if execution == selected.snapshot.execution())
        {
            return Err(denied());
        }
        Self::freeze_materialized(
            pool,
            image_id,
            spec,
            master,
            expected_runtime,
            PrivateInput {
                stdin_json: Some(selected.stdin_json),
                snapshot: Some(selected.snapshot),
            },
            Some(admission),
        )
        .await
    }
    async fn freeze_materialized(
        pool: &PgPool,
        image_id: i64,
        mut spec: LaunchSpec,
        master: &str,
        expected_runtime: Option<&RuntimeStageBinding>,
        selected: PrivateInput,
        admission: Option<i64>,
    ) -> Result<ReservedLaunch> {
        let PrivateInput {
            stdin_json,
            snapshot,
        } = selected;
        if let Some(input) = &stdin_json {
            if input.len() > 8 * 1024 * 1024
                || !serde_json::from_str::<serde_json::Value>(input)
                    .map_err(|_| Error::validation("Invalid admitted stdin JSON"))?
                    .is_object()
            {
                return Err(Error::validation(
                    "Admitted stdin must be a bounded JSON object",
                ));
            }
        }
        let mut tx = pool.begin().await?;
        let sources = Self::source(
            &mut tx,
            &spec.manifest.target,
            spec.manifest.owner_identity_id,
        )
        .await?;
        let input_expiry = if let Some(expected) = &snapshot {
            let mut input = ExecutionInputRepository::restore(
                &mut tx,
                expected.execution(),
                Some(master),
                Some(expected),
            )
            .await?;
            let provided: serde_json::Value =
                serde_json::from_str(stdin_json.as_deref().ok_or_else(denied)?)
                    .map_err(|_| denied())?;
            if &provided != input.value() {
                return Err(denied());
            }
            input.ensure_fresh().await?;
            let (_, _, _, expiry) = input.into_parts();
            Some(expiry)
        } else {
            None
        };
        let selected_input = snapshot.as_ref().map(FrozenInputSnapshot::from);
        let image: Option<i64>=sqlx::query_scalar("SELECT i.id FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine WHERE i.id=$1 AND i.engine=$2 AND i.reference=$3 AND i.sha256=$4 AND i.revoked_at IS NULL AND NOT e.disabled AND i.platform=e.platform FOR SHARE OF i,e")
            .bind(image_id).bind(spec.manifest.engine_id).bind(&spec.manifest.image.reference).bind(spec.manifest.image.sha256.as_str()).fetch_optional(&mut *tx).await?;
        if image.is_none() {
            return Err(denied());
        }
        let input_target = match &spec.manifest.target {
            LaunchTarget::Setup { admission, .. }
            | LaunchTarget::RuntimeProbe { admission, .. } => {
                sqlx::query_scalar::<_, Json<LaunchTarget>>(
                    "SELECT source_target FROM workload_launch_prerequisite WHERE id=$1",
                )
                .bind(admission)
                .fetch_one(&mut *tx)
                .await?
                .0
            }
            target => target.clone(),
        };
        for input in &spec.manifest.inputs {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_launch_object WHERE retired_at IS NULL AND id=$1 AND source_target=$2 AND kind=$3 AND provider_version=$4 AND size_bytes=$5 AND sha256=$6 AND max_expanded_bytes=$7 AND max_files=$8)")
                .bind(input.object_id).bind(Json(&input_target)).bind(serde_json::to_value(input.kind)?.as_str().ok_or_else(denied)?)
                .bind(&input.object_version).bind(i64::try_from(input.size_bytes).map_err(|_|denied())?).bind(input.sha256.as_str())
                .bind(i64::try_from(input.max_expanded_bytes).map_err(|_|denied())?).bind(i64::try_from(input.max_files).map_err(|_|denied())?).fetch_one(&mut *tx).await?;
            if !valid {
                return Err(denied());
            }
        }
        let platform: String =
            sqlx::query_scalar("SELECT platform FROM workload_engine WHERE id=$1 FOR SHARE")
                .bind(spec.manifest.engine_id)
                .fetch_one(&mut *tx)
                .await?;
        let mut sources = sources;
        sources.sources.push(("image", image_id));
        sources.sources.push(("engine", spec.manifest.engine_id));
        let root: Option<ActionAdmission> = if let Some(admission) = admission {
            let root:ActionAdmission=sqlx::query_as("SELECT id,source_target,image,source_epochs,artifact_limits,expires_at,revoked_at FROM workload_launch_prerequisite WHERE id=$1 AND purpose='action' FOR SHARE")
                .bind(admission).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
            if root.image != image_id
                || root.source_target.0 != input_target
                || root.revoked_at.is_some()
                || root.expires_at <= Self::now(&mut tx).await?
            {
                return Err(denied());
            }
            sources.sources.push(("admission", root.id));
            Some(root)
        } else {
            None
        };
        let epochs = workload_source::snapshot(&mut tx, sources.sources.clone()).await?;
        if root.as_ref().is_some_and(|root| {
            root.source_epochs
                .iter()
                .any(|expected| !epochs.contains(expected))
        }) {
            return Err(denied());
        }
        Self::retained_epochs(&sources, &epochs, image_id, false)?;
        for input in spec
            .manifest
            .inputs
            .iter()
            .filter(|input| input.kind == LaunchInputKind::Runtime)
        {
            let expected = expected_runtime.ok_or_else(denied)?;
            if expected.platform.as_str() != platform
                || expected.engine_id != spec.manifest.engine_id
                || expected.image_sha256 != spec.manifest.image.sha256
                || !spec.manifest.inputs.iter().any(|pin| {
                    pin.kind == LaunchInputKind::Pack && pin.sha256 == expected.pack_sha256
                })
            {
                return Err(denied());
            }
            let build:Option<(Json<RuntimeStageBinding>,Json<Vec<SourceEpoch>>)>=sqlx::query_as("SELECT o.stage_binding,b.source_epochs FROM workload_launch_object o JOIN workload_launch b ON b.id=o.build_launch WHERE o.id=$1 AND o.kind='runtime' AND o.retired_at IS NULL AND o.built_image_sha256=$2 AND b.engine=$3 AND o.stage_binding->>'platform'=$4 AND o.built_image_sha256=b.manifest#>>'{image,sha256}' AND b.manifest->>'purpose'='setup' AND b.operation#>>'{status,state}'='stopped' AND b.operation#>>'{status,runtime_state}'='exited' AND b.operation#>>'{status,publication}'='published' AND b.operation#>'{status,stop_reason}'='null'::jsonb AND b.operation#>>'{status,exit_outcome,exit_code}'='0' AND b.operation#>>'{status,exit_outcome,oom_killed}'='false'")
                .bind(input.object_id).bind(expected.image_sha256.as_str()).bind(expected.engine_id).bind(expected.platform.as_str()).fetch_optional(&mut *tx).await?;
            let (binding, build_epochs) = build.ok_or_else(denied)?;
            if binding.0 != *expected
                || build_epochs
                    .iter()
                    .filter(|epoch| epoch.kind != "admission")
                    .any(|epoch| !epochs.contains(epoch))
            {
                return Err(denied());
            }
        }
        if let LaunchTarget::Setup { admission, attempt } = &spec.manifest.target {
            let binding:Json<RuntimeStageBinding>=sqlx::query_scalar("SELECT stage_binding FROM workload_launch_prerequisite WHERE id=$1 AND attempt=$2 AND purpose='setup'")
                .bind(admission).bind(attempt).fetch_one(&mut *tx).await?;
            let pack = spec
                .manifest
                .inputs
                .iter()
                .find(|input| input.kind == LaunchInputKind::Pack)
                .ok_or_else(denied)?;
            if binding.platform.as_str() != platform
                || binding.engine_id != spec.manifest.engine_id
                || binding.image_sha256 != spec.manifest.image.sha256
                || binding.pack_sha256 != pack.sha256
            {
                return Err(denied());
            }
            let bytes =
                serde_json::to_vec(&("attune-runtime-stage-v1", admission, attempt, binding.0))?;
            spec.manifest.stage.as_mut().ok_or_else(denied)?.cache_key =
                Sha256Digest::of_bytes(&bytes).as_str().into();
        }
        // Transaction-owned target admission, after live sources and epochs.
        // The hash covers public identity only; collisions merely serialize.
        // A retained unique index is the final fence even across restarts.
        let target_key = serde_json::to_string(&(spec.manifest.purpose, &spec.manifest.target))?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,71387301))")
            .bind(target_key)
            .execute(&mut *tx)
            .await?;
        let existing: Option<ReservationReplay> =
            sqlx::query_as("SELECT manifest,encrypted_context,source_epochs,encrypted_dispatch,admitted_digest FROM workload_launch WHERE manifest->>'purpose'=$1 AND manifest->'target'=$2 FOR UPDATE")
                .bind(serde_json::to_value(spec.manifest.purpose)?.as_str().ok_or_else(denied)?)
                .bind(Json(&spec.manifest.target)).fetch_optional(&mut *tx).await?;
        if let Some(ReservationReplay {
            manifest,
            encrypted_context: encrypted,
            source_epochs: old_epochs,
            encrypted_dispatch: dispatch,
            admitted_digest: digest,
        }) = existing
        {
            let context: FrozenContext =
                serde_json::from_str(&crypto::decrypt(&encrypted, master).map_err(|_| denied())?)?;
            if !Self::same_epochs(&epochs, &old_epochs, false)
                || manifest.owner_identity_id != spec.manifest.owner_identity_id
                || manifest.engine_id != spec.manifest.engine_id
                || manifest.image != spec.manifest.image
                || manifest.inputs != spec.manifest.inputs
                || manifest.pack_access != spec.manifest.pack_access
                || manifest.stage != spec.manifest.stage
                || manifest.timeout_ms != spec.manifest.timeout_ms
                || context.launch_id != manifest.launch_id
                || context.owner != manifest.owner_identity_id
                || context.purpose != manifest.purpose
                || context.target != manifest.target
                || context.invocation != spec.invocation
                || !same_input(context.stdin_json.as_deref(), stdin_json.as_deref())?
                || context.selected_input != selected_input
            {
                return Err(denied());
            }
            if sources
                .deadline
                .is_some_and(|deadline| deadline <= Utc::now())
            {
                return Err(denied());
            }
            let result = ReservedLaunch {
                request: LaunchRequest {
                    launch_id: manifest.launch_id,
                    admitted_digest: Sha256Digest::parse(digest)?,
                },
                dispatch_token: LaunchToken::new(
                    crypto::decrypt(&dispatch, master).map_err(|_| denied())?,
                )?,
            };
            if let Some(expiry) = &input_expiry {
                expiry.ensure_fresh_on(&mut tx).await?;
            }
            tx.commit().await?;
            if let Some(expiry) = &input_expiry {
                expiry.ensure_fresh()?;
            }
            return Ok(result);
        }
        let id: i64 =
            sqlx::query_scalar("SELECT nextval(pg_get_serial_sequence('workload_launch','id'))")
                .fetch_one(&mut *tx)
                .await?;
        spec.manifest.launch_id = id;
        spec.manifest.engine_name_nonce = Uuid::new_v4();
        let plaintext = serde_json::to_string(&FrozenContext {
            launch_id: id,
            owner: spec.manifest.owner_identity_id,
            purpose: spec.manifest.purpose,
            target: spec.manifest.target.clone(),
            invocation: spec.invocation.clone(),
            stdin_json,
            selected_input,
        })?;
        let encrypted_context = crypto::encrypt(&plaintext, master)?;
        spec.manifest.context = FrozenContextRef {
            id,
            version: 1,
            ciphertext_sha256: Sha256Digest::of_bytes(encrypted_context.as_bytes()),
        };
        spec.validate_shape()?;
        let now = Self::now(&mut tx).await?;
        if spec.manifest.lease_deadline <= now
            || spec.manifest.lease_deadline > now + chrono::Duration::minutes(5)
        {
            return Err(denied());
        }
        if sources
            .deadline
            .is_some_and(|deadline| deadline <= Utc::now())
        {
            return Err(denied());
        }
        let operation = LaunchOperation::reserved(&spec.manifest)?;
        let token = new_token()?;
        sqlx::query("INSERT INTO workload_launch(id,engine,image,owner,manifest,admitted_digest,encrypted_context,source_epochs,operation,dispatch_hash,encrypted_dispatch) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(id).bind(spec.manifest.engine_id).bind(image_id).bind(spec.manifest.owner_identity_id).bind(Json(&spec.manifest))
            .bind(operation.status.request.admitted_digest.as_str()).bind(encrypted_context).bind(Json(&epochs)).bind(Json(&operation)).bind(credential_hash(&token)).bind(crypto::encrypt(token.as_str(),master)?).execute(&mut *tx).await?;
        if matches!(
            spec.manifest.purpose,
            LaunchPurpose::Action | LaunchPurpose::Sensor | LaunchPurpose::CandidateTest
        ) {
            let limits = root
                .as_ref()
                .map_or_else(crate::workload_artifact::ArtifactLimits::default, |root| {
                    root.artifact_limits.0
                });
            crate::workload_artifact::validate_inventory(&[], limits)?;
            sqlx::query("INSERT INTO workload_artifact_publication(launch,max_total_bytes,max_entries) VALUES($1,$2,$3)")
                .bind(id).bind(i64::try_from(limits.max_total_bytes).map_err(|_|denied())?)
                .bind(i64::try_from(limits.max_entries).map_err(|_|denied())?).execute(&mut *tx).await?;
        }
        if let Some(expiry) = &input_expiry {
            expiry.ensure_fresh_on(&mut tx).await?;
        }
        tx.commit().await?;
        if let Some(expiry) = &input_expiry {
            expiry.ensure_fresh()?;
        }
        Ok(ReservedLaunch {
            request: operation.status.request,
            dispatch_token: token,
        })
    }
    /// Exact bound object metadata; provider credentials/keys remain API-only.
    pub async fn input(
        pool: &PgPool,
        actor: ControllerActor,
        handle: &LaunchHandle,
        token: &LaunchToken,
        pin_id: i64,
    ) -> Result<LaunchInputBody> {
        let mut tx = pool.begin().await?;
        let key = LaunchRequest {
            launch_id: handle.launch_id,
            admitted_digest: handle.admitted_digest.clone(),
        };
        let record = Self::admitted(&mut tx, actor, &key).await?;
        Self::require_lease(&record, actor, handle, token, Self::now(&mut tx).await?)?;
        if record.operation.cleanup_only() {
            return Err(denied());
        }
        let input = record
            .manifest
            .inputs
            .iter()
            .find(|pin| pin.pin_id == pin_id)
            .ok_or_else(denied)?;
        let body:LaunchInputBody=sqlx::query_as("SELECT object_key,provider_version,size_bytes,sha256 FROM workload_launch_object WHERE id=$1 AND retired_at IS NULL")
            .bind(input.object_id).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
        if body.provider_version != input.object_version
            || body.size_bytes < 0
            || body.size_bytes as u64 != input.size_bytes
            || body.sha256 != input.sha256.as_str()
        {
            return Err(denied());
        }
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(body)
    }
    pub async fn reconcile(
        pool: &PgPool,
        actor: ControllerActor,
        request: &ReconcileLaunchesRequest,
        master: &str,
    ) -> Result<ReconcileLaunchesPage> {
        if request.limit == 0 || request.limit > 100 || request.after.is_some_and(|id| id <= 0) {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        Self::actor(&mut tx, actor).await?;
        let mut rows:Vec<(i64,String,String)>=sqlx::query_as("SELECT l.id,l.admitted_digest,l.encrypted_dispatch FROM workload_launch l WHERE l.engine=$1 AND l.id>$2 AND (l.operation#>>'{status,state}'<>'stopped' OR l.operation#>>'{status,publication}' IN ('pending','uploading') OR l.operation#>>'{status,capture}'='pending' OR EXISTS(SELECT 1 FROM workload_artifact_publication p WHERE p.launch=l.id AND p.state<>'open' AND p.cleanup_receipt IS NULL)) ORDER BY l.id LIMIT $3")
            .bind(actor.engine).bind(request.after.unwrap_or(0)).bind(i64::from(request.limit)+1).fetch_all(&mut *tx).await?;
        let more = rows.len() > request.limit as usize;
        rows.truncate(request.limit as usize);
        let mut items = Vec::with_capacity(rows.len());
        for (id, digest, encrypted) in rows {
            let dispatch_token =
                LaunchToken::new(crypto::decrypt(&encrypted, master).map_err(|_| denied())?)?;
            items.push(ReservedLaunch {
                request: LaunchRequest {
                    launch_id: id,
                    admitted_digest: Sha256Digest::parse(digest)?,
                },
                dispatch_token,
            });
        }
        let next_cursor = if more {
            items.last().map(|item| item.request.launch_id)
        } else {
            None
        };
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(ReconcileLaunchesPage { items, next_cursor })
    }
    pub async fn resolve(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchRequest,
        dispatch: &LaunchToken,
    ) -> Result<LaunchManifest> {
        let mut tx = pool.begin().await?;
        let record = Self::admitted(&mut tx, actor, request).await?;
        if record.dispatch_hash != credential_hash(dispatch) {
            return Err(denied());
        }
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(record.manifest.0)
    }
    pub async fn claim(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchRequest,
        dispatch: &LaunchToken,
        master: &str,
    ) -> Result<ClaimedLaunch> {
        let mut tx = pool.begin().await?;
        let mut record = Self::admitted(&mut tx, actor, request).await?;
        if record.dispatch_hash != credential_hash(dispatch) {
            return Err(denied());
        }
        if record.operation.status.state == LaunchState::Stopped
            && !record.operation.publication_pending()
            && !record.operation.capture_pending()
        {
            Self::save(&mut tx, &record).await?;
            tx.commit().await?;
            return Ok(ClaimedLaunch::Stopped {
                status: record.operation.status.clone(),
            });
        }
        let now = Self::now(&mut tx).await?;
        if record.operation.lease_generation == 0 && record.manifest.lease_deadline <= now {
            record
                .operation
                .stop_at(LaunchStopReason::LeaseExpired, now);
        }
        let deadline = (now + chrono::Duration::seconds(30))
            .min(DateTime::from_timestamp(actor.expires_at, 0).ok_or_else(denied)?);
        let deadline = if record.operation.publication_pending() || !record.operation.cleanup_only()
        {
            record
                .effect_deadline
                .map_or(deadline, |source| source.min(deadline))
        } else {
            deadline
        };
        let deadline = if record.operation.status.state == LaunchState::Stopped
            && !record.operation.publication_pending()
        {
            record
                .operation
                .status
                .capture_deadline
                .map_or(deadline, |capture| capture.min(deadline))
        } else {
            deadline
        };
        let same_cleanup_owner = record.controller == Some(actor.controller);
        let purpose = record.operation.claim(deadline, now, same_cleanup_owner)?;
        let token = new_token()?;
        record.lease_hash = Some(credential_hash(&token));
        record.controller = Some(actor.controller);
        let lease = LaunchLease {
            handle: record.operation.handle(),
            purpose,
            token,
        };
        let response = if purpose == LaunchLeasePurpose::Capture {
            ClaimedLaunch::Capture {
                manifest: Box::new(record.manifest.0.clone()),
                status: record.operation.status.clone(),
                lease,
            }
        } else if purpose == LaunchLeasePurpose::Publication {
            ClaimedLaunch::Publication {
                manifest: Box::new(record.manifest.0.clone()),
                status: record.operation.status.clone(),
                lease,
            }
        } else if purpose == LaunchLeasePurpose::Cleanup {
            ClaimedLaunch::Cleanup {
                manifest: Box::new(record.manifest.0.clone()),
                status: record.operation.status.clone(),
                lease,
            }
        } else {
            let spec = LaunchSpec {
                manifest: record.manifest.0.clone(),
                invocation: Self::invocation(&record, master)?,
            };
            spec.validate_shape()?;
            ClaimedLaunch::Active {
                spec: Box::new(spec),
                lease,
            }
        };
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(response)
    }
    pub async fn status(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchRequest,
        dispatch: &LaunchToken,
    ) -> Result<LaunchStatus> {
        let mut tx = pool.begin().await?;
        let record = Self::admitted(&mut tx, actor, request).await?;
        if record.dispatch_hash != credential_hash(dispatch) {
            return Err(denied());
        }
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(record.operation.status.clone())
    }
    pub async fn stop(
        pool: &PgPool,
        actor: ControllerActor,
        request: &StopLaunchRequest,
        dispatch: &LaunchToken,
    ) -> Result<LaunchStatus> {
        let mut tx = pool.begin().await?;
        Self::actor(&mut tx, actor).await?;
        // Cancellation only closes authority. It never acquires mutable source
        // locks after the operation and cannot revive a revoked source.
        let mut record = Self::record(&mut tx, &request.request, true).await?;
        if record.engine != actor.engine || record.dispatch_hash != credential_hash(dispatch) {
            return Err(denied());
        }
        record
            .operation
            .stop_at(request.reason, Self::now(&mut tx).await?);
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(record.operation.status.clone())
    }
    pub async fn create_intent(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchLeaseRequest,
        token: &LaunchToken,
    ) -> Result<LaunchCreateIntent> {
        let mut tx = pool.begin().await?;
        let key = LaunchRequest {
            launch_id: request.handle.launch_id,
            admitted_digest: request.handle.admitted_digest.clone(),
        };
        let mut record = Self::admitted(&mut tx, actor, &key).await?;
        Self::require_lease(
            &record,
            actor,
            &request.handle,
            token,
            Self::now(&mut tx).await?,
        )?;
        let result = record.operation.create_intent()?;
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn start_intent(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchLeaseRequest,
        token: &LaunchToken,
    ) -> Result<LaunchStartIntent> {
        let mut tx = pool.begin().await?;
        let key = LaunchRequest {
            launch_id: request.handle.launch_id,
            admitted_digest: request.handle.admitted_digest.clone(),
        };
        let mut record = Self::admitted(&mut tx, actor, &key).await?;
        Self::require_lease(
            &record,
            actor,
            &request.handle,
            token,
            Self::now(&mut tx).await?,
        )?;
        let result = record.operation.start_intent()?;
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn observe(
        pool: &PgPool,
        actor: ControllerActor,
        request: &ObserveLaunchRequest,
        token: &LaunchToken,
    ) -> Result<LaunchStatus> {
        let mut tx = pool.begin().await?;
        let key = LaunchRequest {
            launch_id: request.handle.launch_id,
            admitted_digest: request.handle.admitted_digest.clone(),
        };
        let mut record = Self::admitted(&mut tx, actor, &key).await?;
        Self::require_lease(
            &record,
            actor,
            &request.handle,
            token,
            Self::now(&mut tx).await?,
        )?;
        record
            .operation
            .observe_at(&request.observation, Self::now(&mut tx).await?)?;
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(record.operation.status.clone())
    }
    pub async fn renew(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchLeaseRequest,
        token: &LaunchToken,
    ) -> Result<LaunchLease> {
        let mut tx = pool.begin().await?;
        let key = LaunchRequest {
            launch_id: request.handle.launch_id,
            admitted_digest: request.handle.admitted_digest.clone(),
        };
        let mut record = Self::admitted(&mut tx, actor, &key).await?;
        let now = Self::now(&mut tx).await?;
        Self::require_lease(&record, actor, &request.handle, token, now)?;
        let deadline = (now + chrono::Duration::seconds(30))
            .min(DateTime::from_timestamp(actor.expires_at, 0).ok_or_else(denied)?);
        let deadline = if record.operation.publication_pending() || !record.operation.cleanup_only()
        {
            record
                .effect_deadline
                .map_or(deadline, |source| source.min(deadline))
        } else {
            deadline
        };
        let deadline = if record.operation.status.state == LaunchState::Stopped
            && !record.operation.publication_pending()
        {
            record
                .operation
                .status
                .capture_deadline
                .map_or(deadline, |capture| capture.min(deadline))
        } else {
            deadline
        };
        record.operation.renew(&request.handle, deadline, now)?;
        let lease = LaunchLease {
            handle: record.operation.handle(),
            purpose: record.operation.lease_purpose,
            token: token.clone(),
        };
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(lease)
    }
    pub async fn stop_ack(
        pool: &PgPool,
        actor: ControllerActor,
        request: &StopLaunchAcknowledgement,
        token: &LaunchToken,
    ) -> Result<LaunchStatus> {
        let mut tx = pool.begin().await?;
        Self::actor(&mut tx, actor).await?;
        let key = LaunchRequest {
            launch_id: request.handle.launch_id,
            admitted_digest: request.handle.admitted_digest.clone(),
        };
        let mut record = Self::record(&mut tx, &key, true).await?;
        if record.engine != actor.engine {
            return Err(denied());
        }
        Self::require_lease(
            &record,
            actor,
            &request.handle,
            token,
            Self::now(&mut tx).await?,
        )?;
        record.operation.acknowledge_removed(request)?;
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(record.operation.status.clone())
    }
    /// Finite raw I/O grants are independent of log encryption keys. Controller
    /// and manager credentials never enter the pack invocation/environment.
    pub async fn io_grant(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchIoGrantRequest,
        dispatch: &LaunchToken,
    ) -> Result<LaunchIoGrant> {
        let mut tx = pool.begin().await?;
        let record = if request.channel == LaunchIoChannel::Stdin {
            Self::admitted(&mut tx, actor, &request.request).await?
        } else {
            Self::admitted_for_capture(&mut tx, actor, &request.request).await?
        };
        if record.dispatch_hash != credential_hash(dispatch)
            || record.operation.status.capture == LaunchCaptureState::Incomplete
        {
            return Err(denied());
        }
        if request.channel == LaunchIoChannel::Stdin
            && (record.operation.cleanup_only() || record.operation.stdin_authorized)
        {
            return Err(denied());
        }
        let now = Self::now(&mut tx).await?;
        let expires_at = (now + chrono::Duration::seconds(30))
            .min(DateTime::from_timestamp(actor.expires_at, 0).ok_or_else(denied)?);
        let expires_at = record
            .effect_deadline
            .map_or(expires_at, |source| source.min(expires_at));
        // Each grant is bounded independently; a sensor's growing stream is read
        // in finite slices. Offset addition is checked again by authorize_io.
        if request.channel != LaunchIoChannel::Stdin && !record.operation.capture_pending() {
            return Err(denied());
        }
        let expires_at = record
            .operation
            .status
            .capture_deadline
            .map_or(expires_at, |capture| capture.min(expires_at));
        let max_bytes = if request.channel == LaunchIoChannel::Stdin {
            crate::workload_stdin::MAX_PRIVATE_STDIN_BYTES as u64
        } else {
            1024 * 1024
        };
        let token = new_token()?;
        sqlx::query("INSERT INTO workload_launch_io_grant(launch,channel,token_hash,max_bytes,expires_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(launch,channel) DO UPDATE SET token_hash=EXCLUDED.token_hash,max_bytes=EXCLUDED.max_bytes,expires_at=EXCLUDED.expires_at")
            .bind(record.id).bind(serde_json::to_value(request.channel)?.as_str().ok_or_else(denied)?)
            .bind(credential_hash(&token)).bind(max_bytes as i64).bind(expires_at).execute(&mut *tx).await?;
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(LaunchIoGrant {
            request: request.request.clone(),
            channel: request.channel,
            token,
            max_bytes,
            expires_at,
        })
    }
    async fn admitted_for_capture(
        tx: &mut Transaction<'_, Postgres>,
        actor: ControllerActor,
        request: &LaunchRequest,
    ) -> Result<Record> {
        Self::fresh_record(tx, actor, request, true).await
    }
    async fn admitted_for_publication(
        tx: &mut Transaction<'_, Postgres>,
        actor: ControllerActor,
        request: &LaunchRequest,
    ) -> Result<Record> {
        Self::fresh_record(tx, actor, request, false).await
    }
    async fn fresh_record(
        tx: &mut Transaction<'_, Postgres>,
        actor: ControllerActor,
        request: &LaunchRequest,
        capture: bool,
    ) -> Result<Record> {
        Self::actor(tx, actor).await?;
        let snapshot = Self::record(tx, request, false).await?;
        if snapshot.engine != actor.engine {
            return Err(denied());
        }
        let (epochs, deadline) = Self::epochs_for(tx, &snapshot, capture).await?;
        let mut record = Self::record(tx, request, true).await?;
        record.effect_deadline = deadline;
        record.operation.expire_capture(Self::now(tx).await?);
        if !Self::same_epochs(&epochs, &record.source_epochs.0, capture)
            || (capture && record.operation.status.capture == LaunchCaptureState::Incomplete)
        {
            return Err(denied());
        }
        if deadline.is_some_and(|deadline| deadline <= Utc::now()) {
            return Err(denied());
        }
        Ok(record)
    }
    pub async fn authorize_io(
        pool: &PgPool,
        actor: ControllerActor,
        request: &AuthorizeLaunchIoRequest,
        lease: &LaunchToken,
        grant: &LaunchToken,
    ) -> Result<LaunchIoAuthorization> {
        // Byte counts cannot authenticate the selected private stdin. No grant
        // can opt out of the dedicated exact-payload/origin admission path.
        if request.channel == LaunchIoChannel::Stdin {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        let record = Self::admitted_for_capture(&mut tx, actor, &request.request).await?;
        let now = Self::now(&mut tx).await?;
        Self::require_lease(&record, actor, &record.operation.handle(), lease, now)?;
        let permission:Option<(i64,DateTime<Utc>)>=sqlx::query_as("SELECT max_bytes,expires_at FROM workload_launch_io_grant WHERE launch=$1 AND channel=$2 AND token_hash=$3 AND expires_at>clock_timestamp() FOR SHARE")
            .bind(record.id).bind(serde_json::to_value(request.channel)?.as_str().ok_or_else(denied)?)
            .bind(credential_hash(grant)).fetch_optional(&mut *tx).await?;
        let (max_bytes, expires_at) = permission.ok_or_else(denied)?;
        if request.bytes == 0
            || request.bytes > max_bytes as u64
            || request.offset.checked_add(request.bytes).is_none()
        {
            return Err(denied());
        }
        if !record.operation.capture_pending()
            || record
                .operation
                .status
                .capture_deadline
                .is_some_and(|deadline| deadline <= now)
        {
            return Err(denied());
        }
        let expires_at = record
            .effect_deadline
            .map_or(expires_at, |source| source.min(expires_at))
            .min(record.operation.lease_deadline);
        let expires_at = record
            .operation
            .status
            .capture_deadline
            .map_or(expires_at, |capture| capture.min(expires_at));
        let result = LaunchIoAuthorization {
            handle: record.operation.handle(),
            channel: request.channel,
            max_bytes: request.bytes,
            expires_at,
        };
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(result)
    }
}
