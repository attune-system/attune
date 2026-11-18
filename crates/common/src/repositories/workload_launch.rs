//! Trusted control-plane launch ledger. No provider effect is authorized without
//! source -> epoch -> operation admission in an owning SQLx transaction. Public
//! requests select only a previously frozen launch; they cannot supply a plan.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    delegated_execution::DelegatedExecutionRepository,
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

const COLUMNS: &str = "id,engine,image,owner,manifest,admitted_digest,encrypted_context,source_epochs,operation,dispatch_hash,lease_hash,controller";
fn denied() -> Error {
    Error::PermissionDenied("Workload launch unavailable".into())
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
}

#[derive(sqlx::FromRow)]
pub struct LaunchInputBody {
    pub object_key: String,
    pub provider_version: String,
    pub size_bytes: i64,
    pub sha256: String,
}

struct SourceProof {
    sources: Vec<(&'static str, i64)>,
    deadline: Option<DateTime<Utc>>,
}

pub struct WorkloadLaunchRepository;
impl WorkloadLaunchRepository {
    async fn now(tx: &mut Transaction<'_, Postgres>) -> Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **tx)
            .await?)
    }
    async fn actor(tx: &mut Transaction<'_, Postgres>, actor: ControllerActor) -> Result<()> {
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
        if !record.operation.cleanup_only() || record.operation.publication_pending() {
            if record
                .effect_deadline
                .is_some_and(|deadline| deadline <= now)
            {
                return Err(denied());
            }
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
    async fn source(
        tx: &mut Transaction<'_, Postgres>,
        target: &LaunchTarget,
        expected_owner: i64,
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
                        let current: Option<Uuid>=sqlx::query_scalar("SELECT a.attempt FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=$1 AND e.worker=$2 AND a.worker=$2 AND NOT a.closed AND a.producer_kind='worker' AND e.status='running' FOR SHARE OF e,a")
                            .bind(execution).bind(worker).fetch_optional(&mut **tx).await?;
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
                        let current: Option<Uuid>=sqlx::query_scalar("SELECT f.attempt FROM execution e JOIN runtime_authority_execution_fence f ON f.execution=e.id WHERE e.id=$1 AND e.worker=$2 AND e.status='running' AND f.admission=$3 FOR SHARE OF e,f")
                            .bind(execution).bind(worker).bind(record.id).fetch_optional(&mut **tx).await?;
                        if current != Some(*attempt) {
                            return Err(denied());
                        }
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
                        fence.ensure_fresh_on(tx).await?;
                        deadline = fence.approval_deadline();
                    }
                }
                Self::worker(tx, *worker, *worker_instance).await?;
                Ok(SourceProof {
                    sources: vec![
                        ("owner", expected_owner),
                        ("execution", *execution),
                        ("worker", *worker),
                    ],
                    deadline,
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
                })
            }
            LaunchTarget::Setup { admission, attempt }
            | LaunchTarget::RuntimeProbe { admission, attempt } => {
                let purpose = if matches!(target, LaunchTarget::Setup { .. }) {
                    "setup"
                } else {
                    "runtime_probe"
                };
                let source: Option<Json<LaunchTarget>>=sqlx::query_scalar("SELECT source_target FROM workload_launch_prerequisite WHERE id=$1 AND attempt=$2 AND purpose=$3 AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE")
                    .bind(admission).bind(attempt).bind(purpose).fetch_optional(&mut **tx).await?;
                let source = source.ok_or_else(denied)?.0;
                if matches!(
                    source,
                    LaunchTarget::Setup { .. } | LaunchTarget::RuntimeProbe { .. }
                ) {
                    return Err(denied());
                }
                let mut sources = Box::pin(Self::source(tx, &source, expected_owner)).await?;
                let deadline: DateTime<Utc> = sqlx::query_scalar(
                    "SELECT expires_at FROM workload_launch_prerequisite WHERE id=$1",
                )
                .bind(admission)
                .fetch_one(&mut **tx)
                .await?;
                sources.deadline = Some(
                    sources
                        .deadline
                        .map_or(deadline, |existing| existing.min(deadline)),
                );
                sources.sources.push(("admission", *admission));
                Ok(sources)
            }
        }
    }
    async fn worker(tx: &mut Transaction<'_, Postgres>, worker: i64, instance: Uuid) -> Result<()> {
        // Execution registration's incarnation is distinct from log-delivery's
        // restart-persistent source instance. Never interchange the two fences.
        let current: Option<Uuid> = sqlx::query_scalar(
            "SELECT (meta->>'registration_incarnation')::UUID FROM worker WHERE id=$1 AND status IN ('active','busy') FOR SHARE",
        )
        .bind(worker)
        .fetch_optional(&mut **tx)
        .await?;
        if current != Some(instance) {
            return Err(denied());
        }
        Ok(())
    }
    async fn epochs(
        tx: &mut Transaction<'_, Postgres>,
        record: &Record,
    ) -> Result<(Vec<SourceEpoch>, Option<DateTime<Utc>>)> {
        let mut sources = Self::source(tx, &record.manifest.target, record.owner).await?;
        let image:Option<i64>=sqlx::query_scalar("SELECT i.id FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine WHERE i.id=$1 AND i.engine=$2 AND i.sha256=$3 AND i.reference=$4 AND i.revoked_at IS NULL AND NOT e.disabled FOR SHARE OF i,e")
            .bind(record.image).bind(record.engine).bind(record.manifest.image.sha256.as_str()).bind(&record.manifest.image.reference).fetch_optional(&mut **tx).await?;
        if image.is_none() {
            return Err(denied());
        }
        sources.sources.push(("image", record.image));
        Ok((
            workload_source::snapshot(tx, sources.sources).await?,
            sources.deadline,
        ))
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
        let mut effect_deadline = None;
        let admitted =
            if snapshot.operation.cleanup_only() && !snapshot.operation.publication_pending() {
                false
            } else {
                match Self::epochs(tx, &snapshot).await {
                    Ok((epochs, deadline)) => {
                        effect_deadline = deadline;
                        epochs == snapshot.source_epochs.0
                    }
                    Err(Error::PermissionDenied(_)) => false,
                    Err(error) => return Err(error),
                }
            };
        let mut record = Self::record(tx, request, true).await?;
        record.effect_deadline = effect_deadline;
        let now = Self::now(tx).await?;
        if record
            .effect_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            record.operation.stop(LaunchStopReason::SourceRevoked);
        }
        if !admitted && (!record.operation.cleanup_only() || record.operation.publication_pending())
        {
            record.operation.stop(LaunchStopReason::SourceRevoked);
        }
        if record.operation.lease_generation > 0
            && record.operation.lease_deadline <= Self::now(tx).await?
            && !record.operation.cleanup_only()
        {
            record.operation.stop(LaunchStopReason::LeaseExpired);
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
    fn invocation(record: &Record, master: &str) -> Result<LaunchInvocation> {
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
        Ok(context.invocation)
    }
    /// Called by trusted CP planners after materializing the exact admitted
    /// archive/context. This is deliberately not a raw-spec HTTP endpoint.
    /// Generated IDs/nonces/context pins override any placeholders in the plan.
    pub async fn freeze(
        pool: &PgPool,
        image_id: i64,
        mut spec: LaunchSpec,
        master: &str,
    ) -> Result<ReservedLaunch> {
        let mut tx = pool.begin().await?;
        let sources = Self::source(
            &mut tx,
            &spec.manifest.target,
            spec.manifest.owner_identity_id,
        )
        .await?;
        let image: Option<i64>=sqlx::query_scalar("SELECT i.id FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine WHERE i.id=$1 AND i.engine=$2 AND i.reference=$3 AND i.sha256=$4 AND i.revoked_at IS NULL AND NOT e.disabled FOR SHARE OF i,e")
            .bind(image_id).bind(spec.manifest.engine_id).bind(&spec.manifest.image.reference).bind(spec.manifest.image.sha256.as_str()).fetch_optional(&mut *tx).await?;
        if image.is_none() {
            return Err(denied());
        }
        for input in &spec.manifest.inputs {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_launch_object WHERE id=$1 AND source_target=$2 AND kind=$3 AND provider_version=$4 AND size_bytes=$5 AND sha256=$6 AND max_expanded_bytes=$7 AND max_files=$8)")
                .bind(input.object_id).bind(Json(&spec.manifest.target)).bind(serde_json::to_value(input.kind)?.as_str().ok_or_else(denied)?)
                .bind(&input.object_version).bind(i64::try_from(input.size_bytes).map_err(|_|denied())?).bind(input.sha256.as_str())
                .bind(i64::try_from(input.max_expanded_bytes).map_err(|_|denied())?).bind(i64::try_from(input.max_files).map_err(|_|denied())?).fetch_one(&mut *tx).await?;
            if !valid {
                return Err(denied());
            }
        }
        let mut sources = sources;
        sources.sources.push(("image", image_id));
        let epochs = workload_source::snapshot(&mut tx, sources.sources).await?;
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
        tx.commit().await?;
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
        let body:LaunchInputBody=sqlx::query_as("SELECT object_key,provider_version,size_bytes,sha256 FROM workload_launch_object WHERE id=$1")
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
        let mut rows:Vec<(i64,String,String)>=sqlx::query_as("SELECT id,admitted_digest,encrypted_dispatch FROM workload_launch WHERE engine=$1 AND id>$2 AND (operation#>>'{status,state}'<>'stopped' OR operation#>>'{status,publication}' IN ('pending','uploading')) ORDER BY id LIMIT $3")
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
        {
            Self::save(&mut tx, &record).await?;
            tx.commit().await?;
            return Ok(ClaimedLaunch::Stopped {
                status: record.operation.status.clone(),
            });
        }
        let now = Self::now(&mut tx).await?;
        if record.operation.lease_generation == 0 && record.manifest.lease_deadline <= now {
            record.operation.stop(LaunchStopReason::LeaseExpired);
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
        let response = if purpose == LaunchLeasePurpose::Publication {
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
        record.operation.stop(request.reason);
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
        record.operation.observe(&request.observation)?;
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
}
