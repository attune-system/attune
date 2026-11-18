//! Durable reserve/prepare/cancel protocol. Every entry retains the caller's
//! SQLx transaction through graph admission, source checks and publication.
use super::workload_host::WorkloadHostRepository;
use crate::{
    log_custody::{
        CustodyAbsence, CustodyOperationAck, CustodyOperationState, PrepareCustody, ReserveCustody,
    },
    workload_host::HostActor,
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::{
    delegated_execution::DelegatedExecutionRepository,
    file_evidence_retention::FileEvidenceRetentionRepository as Graph,
    log_delivery::{LogDeliveryRepository, PrepareLogDelivery},
};
use crate::{log_delivery::ControlRequest, Error, Result};

const COLUMNS: &str = "custody, execution, artifact_version, stream, worker, source_instance, attempt, owner, owner_epoch, execution_epoch, worker_epoch, finalize_by, state, payload_sha256";

#[derive(sqlx::FromRow)]
struct Operation {
    custody: Uuid,
    execution: i64,
    artifact_version: i64,
    stream: i64,
    worker: i64,
    source_instance: Uuid,
    attempt: Uuid,
    owner: Option<i64>,
    owner_epoch: i64,
    execution_epoch: i64,
    worker_epoch: i64,
    finalize_by: DateTime<Utc>,
    state: String,
    payload_sha256: Option<String>,
}
fn denied() -> Error {
    Error::PermissionDenied("Custody operation unavailable".into())
}
fn micros(time: DateTime<Utc>) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_micros(time.timestamp_micros()).ok_or_else(denied)
}
impl Operation {
    fn matches(&self, request: &ReserveCustody, worker: i64) -> Result<()> {
        let b = &request.binding;
        if self.worker != worker
            || self.custody != b.custody
            || self.execution != b.execution
            || self.artifact_version != b.version
            || self.stream != b.stream
            || self.source_instance != b.source_instance
            || self.attempt != b.attempt
            || self.finalize_by != micros(request.finalize_by)?
        {
            return Err(denied());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Epochs {
    owner: i64,
    execution: i64,
    worker: i64,
}
impl Operation {
    fn require_epochs(&self, epochs: Epochs) -> Result<()> {
        if (self.owner_epoch, self.execution_epoch, self.worker_epoch)
            != (epochs.owner, epochs.execution, epochs.worker)
        {
            return Err(denied());
        }
        Ok(())
    }
}

pub struct CustodyOperationRepository;
impl CustodyOperationRepository {
    /// Legacy control requests omit the reservation deadline. Only an exact
    /// retained operation supplies that evidence; absence alone never permits
    /// deletion. The exclusive graph lock also covers the lookup and tombstone,
    /// including preparation requests that have not entered admission yet.
    pub(crate) async fn cancel_retained_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        worker: i64,
        current_instance: Uuid,
        binding: &ControlRequest,
    ) -> Result<bool> {
        Graph::lock_reclamation(transaction).await?;
        LogDeliveryRepository::require_instance(transaction, worker, current_instance).await?;
        // Discovery only: do not acquire an operation lock before source locks.
        // Exclusive graph admission excludes concurrent reserve/prepare/retention.
        let deadline: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT finalize_by FROM log_custody_operation WHERE custody=$1")
                .bind(binding.custody)
                .fetch_optional(&mut **transaction)
                .await?;
        let Some(finalize_by) = deadline else {
            return Ok(false);
        };
        Self::cancel_absent_in(
            transaction,
            worker,
            current_instance,
            &ReserveCustody {
                binding: binding.clone(),
                finalize_by,
            },
        )
        .await
    }
    /// Existing in-process preparation uses this SAME canonical protocol. Its
    /// transaction borrows stay owned by the caller; only remote preparation
    /// needs a separately acknowledged reservation before the request starts.
    pub(crate) async fn reserve_and_prepare_local_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: PrepareLogDelivery,
    ) -> Result<()> {
        Graph::lock_capture(transaction).await?;
        // Discovery is not authority. admit_source locks/rechecks this exact
        // attempt after the active owner and before epochs/operation locks.
        let attempt: Uuid = sqlx::query_scalar(
            "SELECT attempt FROM delegated_execution_attempt WHERE execution=$1",
        )
        .bind(input.execution)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(denied)?;
        let request = ReserveCustody {
            binding: ControlRequest {
                execution: input.execution,
                version: input.version,
                stream: input.stream,
                attempt,
                source_instance: input.instance,
                custody: input.custody,
            },
            finalize_by: input.finalize_by,
        };
        Self::reserve_in(transaction, input.worker, input.instance, &request).await?;
        Self::prepare_in(transaction, input, attempt).await
    }
    async fn fresh(
        conn: &mut PgConnection,
        actor: HostActor,
        deadline: DateTime<Utc>,
    ) -> Result<()> {
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(conn)
            .await?;
        if actor.expires_at <= now.timestamp() || deadline <= now {
            return Err(denied());
        }
        Ok(())
    }
    fn after_commit(actor: HostActor, deadline: DateTime<Utc>) -> Result<()> {
        if actor.expires_at <= Utc::now().timestamp() || deadline <= Utc::now() {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn reserve(
        pool: &PgPool,
        actor: HostActor,
        request: &ReserveCustody,
    ) -> Result<CustodyOperationAck> {
        let mut tx = pool.begin().await?;
        Graph::lock_capture(&mut tx).await?;
        let (host_deadline, current_instance) =
            WorkloadHostRepository::admit_custody(&mut tx, actor).await?;
        if request.binding.source_instance != current_instance {
            return Err(denied());
        }
        Self::reserve_in(
            &mut tx,
            actor.worker,
            request.binding.source_instance,
            request,
        )
        .await?;
        let operation = Self::find(&mut tx, request.binding.custody)
            .await?
            .ok_or_else(denied)?;
        let state = match operation.state.as_str() {
            "reserved" => CustodyOperationState::Reserved,
            "prepared" => CustodyOperationState::Prepared,
            _ => return Err(denied()),
        };
        let deadline = host_deadline.min(micros(request.finalize_by)?);
        Self::fresh(&mut tx, actor, deadline).await?;
        tx.commit().await?;
        Self::after_commit(actor, deadline)?;
        Ok(CustodyOperationAck {
            custody: request.binding.custody,
            state,
        })
    }
    pub async fn prepare(
        pool: &PgPool,
        actor: HostActor,
        request: &PrepareCustody,
    ) -> Result<CustodyOperationAck> {
        let b = &request.reservation.binding;
        if request
            .authenticated_manifest
            .as_ref()
            .is_some_and(|value| value.len() > 65536)
        {
            return Err(denied());
        }
        let input = PrepareLogDelivery {
            execution: b.execution,
            version: b.version,
            stream: b.stream,
            worker: actor.worker,
            instance: b.source_instance,
            custody: b.custody,
            segments: request.segments.clone(),
            total_bytes: request.total_bytes,
            sha256: request.sha256.clone(),
            truncated: request.truncated,
            authenticated_manifest: request.authenticated_manifest.clone(),
            finalize_by: request.reservation.finalize_by,
        };
        let mut tx = pool.begin().await?;
        Graph::lock_capture(&mut tx).await?;
        let (host_deadline, current_instance) =
            WorkloadHostRepository::admit_custody(&mut tx, actor).await?;
        if b.source_instance != current_instance {
            return Err(denied());
        }
        Self::prepare_in(&mut tx, input, b.attempt).await?;
        let deadline = host_deadline.min(micros(request.reservation.finalize_by)?);
        Self::fresh(&mut tx, actor, deadline).await?;
        tx.commit().await?;
        Self::after_commit(actor, deadline)?;
        Ok(CustodyOperationAck {
            custody: b.custody,
            state: CustodyOperationState::Prepared,
        })
    }
    pub async fn cancel_absent(
        pool: &PgPool,
        actor: HostActor,
        request: &ReserveCustody,
    ) -> Result<CustodyAbsence> {
        let mut tx = pool.begin().await?;
        Graph::lock_reclamation(&mut tx).await?;
        let (host_deadline, current_instance) =
            WorkloadHostRepository::admit_custody(&mut tx, actor).await?;
        let absent =
            Self::cancel_absent_in(&mut tx, actor.worker, current_instance, request).await?;
        Self::fresh(&mut tx, actor, host_deadline).await?;
        tx.commit().await?;
        Self::after_commit(actor, host_deadline)?;
        Ok(CustodyAbsence {
            custody: request.binding.custody,
            absent,
        })
    }
    async fn find(conn: &mut PgConnection, custody: Uuid) -> Result<Option<Operation>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM log_custody_operation WHERE custody=$1 FOR UPDATE"
        ))
        .bind(custody)
        .fetch_optional(conn)
        .await?)
    }

    /// Sources must already be locked before taking these epoch SHARE locks.
    /// Source revocation takes source locks then increments the epoch. An owner
    /// or source restored later can never recreate the captured epoch values.
    async fn epochs(
        conn: &mut PgConnection,
        owner: i64,
        execution: i64,
        worker: i64,
    ) -> Result<Epochs> {
        let mut result = Epochs {
            owner: 0,
            execution: 0,
            worker: 0,
        };
        for (kind, subject) in [
            ("execution", execution),
            ("owner", owner),
            ("worker", worker),
        ] {
            sqlx::query("INSERT INTO workload_source_epoch(kind,subject) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(kind).bind(subject).execute(&mut *conn).await?;
            let epoch: i64 = sqlx::query_scalar(
                "SELECT epoch FROM workload_source_epoch WHERE kind=$1 AND subject=$2 FOR SHARE",
            )
            .bind(kind)
            .bind(subject)
            .fetch_one(&mut *conn)
            .await?;
            match kind {
                "owner" => result.owner = epoch,
                "execution" => result.execution = epoch,
                _ => result.worker = epoch,
            }
        }
        Ok(result)
    }

    /// Canonical source lock order shared by reservation and raw preparation.
    /// No epoch or operation row may be acquired before all these source locks.
    pub(crate) async fn admit_source(
        conn: &mut PgConnection,
        worker: i64,
        instance: Uuid,
        request: &ReserveCustody,
    ) -> Result<i64> {
        use super::artifact::{ArtifactRepository, ArtifactVersionRepository};
        let b = &request.binding;
        let deadline = micros(request.finalize_by)?;
        if [worker, b.execution, b.version, b.stream]
            .iter()
            .any(|id| *id <= 0)
            || b.custody.is_nil()
            || b.attempt.is_nil()
            || b.source_instance != instance
            || instance.is_nil()
            || deadline <= Utc::now()
        {
            return Err(denied());
        }
        let selected = DelegatedExecutionRepository::validate(conn, b.execution).await?;
        if !selected.explicit {
            return Err(denied());
        }
        let owner = selected.owner.ok_or_else(denied)?;
        let attempt: Option<Uuid> = sqlx::query_scalar("SELECT a.attempt FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=$1 AND e.worker=$2 AND a.worker=e.worker AND a.producer_kind='worker' AND NOT a.closed AND e.status IN ('running','canceling') FOR SHARE OF e,a")
            .bind(b.execution).bind(worker).fetch_optional(&mut *conn).await?;
        if attempt != Some(b.attempt) {
            return Err(denied());
        }
        LogDeliveryRepository::require_instance(conn, worker, instance).await?;
        let schema: Option<i64> = sqlx::query_scalar("SELECT entity_id FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1 FOR SHARE")
            .bind(b.execution).fetch_optional(&mut *conn).await?;
        if schema != Some(b.execution) {
            return Err(denied());
        }
        let version = ArtifactVersionRepository::find_by_id(&mut *conn, b.version)
            .await?
            .ok_or_else(denied)?;
        ArtifactRepository::find_by_id_for_update(conn, version.artifact)
            .await?
            .ok_or_else(denied)?;
        let _: i64 = sqlx::query_scalar("SELECT id FROM artifact_version WHERE id=$1 FOR UPDATE")
            .bind(b.version)
            .fetch_one(&mut *conn)
            .await?;
        let _: i64 = sqlx::query_scalar("SELECT id FROM log_stream WHERE id=$1 FOR UPDATE")
            .bind(b.stream)
            .fetch_one(&mut *conn)
            .await?;
        let binding: Option<i64> = sqlx::query_scalar(
            "SELECT artifact FROM artifact_runtime_owner_binding WHERE artifact=$1 FOR SHARE",
        )
        .bind(version.artifact)
        .fetch_optional(&mut *conn)
        .await?;
        if binding != Some(version.artifact) {
            return Err(denied());
        }
        let bound: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_version v JOIN log_stream s ON s.artifact_version=v.id JOIN artifact a ON a.id=v.artifact JOIN execution e ON e.id=v.execution JOIN action ac ON ac.id=e.action JOIN artifact_runtime_owner_binding b ON b.artifact=a.id WHERE v.id=$1 AND s.id=$2 AND v.execution=$3 AND s.backend='object_segments' AND NOT s.sealed AND s.close_phase IS NULL AND v.body_state='pending' AND a.classification='runtime_log' AND a.visibility='private' AND b.owner_type='action' AND b.owner_id=e.action AND b.managing_pack=ac.pack AND ac.id=(e.executable_snapshot#>>'{executable,action,id}')::bigint AND ac.pack=(e.executable_snapshot#>>'{executable,action,pack}')::bigint AND $4>clock_timestamp() AND $4<=clock_timestamp()+interval '60 seconds')")
            .bind(b.version).bind(b.stream).bind(b.execution).bind(deadline).fetch_one(&mut *conn).await?;
        if !bound {
            return Err(denied());
        }
        Ok(owner)
    }

    /// Reserve must commit and be acknowledged before sending remote prepare.
    /// A delayed reservation conflicts with a cancellation tombstone.
    async fn reserve_in(
        conn: &mut PgConnection,
        worker: i64,
        instance: Uuid,
        request: &ReserveCustody,
    ) -> Result<()> {
        Graph::lock_capture(conn).await?;
        let b = &request.binding;
        let deadline = micros(request.finalize_by)?;
        let owner = Self::admit_source(conn, worker, instance, request).await?;
        let epochs = Self::epochs(conn, owner, b.execution, worker).await?;
        sqlx::query("INSERT INTO log_custody_operation(custody,execution,artifact_version,stream,worker,source_instance,attempt,owner,owner_epoch,execution_epoch,worker_epoch,finalize_by,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'reserved') ON CONFLICT DO NOTHING")
            .bind(b.custody).bind(b.execution).bind(b.version).bind(b.stream).bind(worker)
            .bind(instance).bind(b.attempt).bind(owner).bind(epochs.owner).bind(epochs.execution).bind(epochs.worker).bind(deadline).execute(&mut *conn).await?;
        let operation = Self::find(conn, b.custody).await?.ok_or_else(denied)?;
        operation.matches(request, worker)?;
        operation.require_epochs(epochs)?;
        if operation.owner != Some(owner) || operation.state == "cancelled" {
            return Err(denied());
        }
        Ok(())
    }

    /// This is the only public preparation entry point. Prepared replays compare
    /// the complete immutable payload; they never recreate retained-away output.
    async fn prepare_in(
        conn: &mut PgConnection,
        input: PrepareLogDelivery,
        attempt: Uuid,
    ) -> Result<()> {
        Graph::lock_capture(conn).await?;
        input.validate()?;
        let request = ReserveCustody {
            binding: ControlRequest {
                execution: input.execution,
                version: input.version,
                stream: input.stream,
                attempt,
                source_instance: input.instance,
                custody: input.custody,
            },
            finalize_by: input.finalize_by,
        };
        let owner = Self::admit_source(conn, input.worker, input.instance, &request).await?;
        let epochs = Self::epochs(conn, owner, input.execution, input.worker).await?;
        let operation = Self::find(conn, input.custody).await?.ok_or_else(denied)?;
        operation.matches(&request, input.worker)?;
        operation.require_epochs(epochs)?;
        if operation.owner != Some(owner) {
            return Err(denied());
        }
        let mut canonical = input.clone();
        canonical.finalize_by = micros(canonical.finalize_by)?;
        let mut hash = Sha256::new();
        hash.update(b"attune.log-custody.prepare.v1\0");
        hash.update(serde_json::to_vec(&canonical).map_err(|_| denied())?);
        hash.update(attempt.as_bytes());
        let digest: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if operation.state == "prepared" {
            return if operation.payload_sha256.as_deref() == Some(digest.as_str()) {
                Ok(())
            } else {
                Err(denied())
            };
        }
        if operation.state != "reserved" || operation.finalize_by <= Utc::now() {
            return Err(denied());
        }
        // Creates the intent and the operation acknowledgement in one transaction.
        Self::create_intent(conn, canonical).await?;
        sqlx::query("UPDATE log_custody_operation SET state='prepared',payload_sha256=$2 WHERE custody=$1 AND state='reserved'")
            .bind(input.custody).bind(digest).execute(conn).await?;
        Ok(())
    }

    /// Raw intent publication is private to this owning protocol. No other
    /// repository/service can call it without reservation/epoch admission.
    async fn create_intent(
        conn: &mut PgConnection,
        mut input: PrepareLogDelivery,
    ) -> Result<super::log_delivery::LogDeliveryIntent> {
        input.finalize_by = DateTime::from_timestamp_micros(input.finalize_by.timestamp_micros())
            .ok_or_else(denied)?;
        input.validate()?;
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_capture(conn).await?;
        let selected = DelegatedExecutionRepository::validate(conn, input.execution).await?;
        if !selected.explicit {
            return Err(denied());
        }
        let owner = selected.owner.ok_or_else(denied)?;
        // Cancellation may already have stopped action API access. Custody
        // still belongs to the assigned open attempt until terminal publication.
        let attempt:Uuid=sqlx::query_scalar("SELECT a.attempt FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=$1 AND e.worker=$2 AND a.worker=e.worker AND a.producer_kind='worker' AND NOT a.closed AND e.status IN ('running','canceling') FOR SHARE OF e,a").bind(input.execution).bind(input.worker).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        // Instance is registered independently of the action's revoked API token.
        LogDeliveryRepository::require_instance(conn, input.worker, input.instance).await?;
        let version =
            super::artifact::ArtifactVersionRepository::find_by_id(&mut *conn, input.version)
                .await?
                .ok_or_else(denied)?;
        super::artifact::ArtifactRepository::find_by_id_for_update(conn, version.artifact)
            .await?
            .ok_or_else(denied)?;
        let _: i64 = sqlx::query_scalar("SELECT id FROM artifact_version WHERE id=$1 FOR UPDATE")
            .bind(input.version)
            .fetch_one(&mut *conn)
            .await?;
        // Serialize snapshotting accepted segments with append and close.
        let _: i64 = sqlx::query_scalar("SELECT id FROM log_stream WHERE id=$1 FOR UPDATE")
            .bind(input.stream)
            .fetch_one(&mut *conn)
            .await?;
        let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_version v JOIN log_stream s ON s.artifact_version=v.id JOIN artifact a ON a.id=v.artifact JOIN execution e ON e.id=v.execution JOIN action ac ON ac.id=e.action JOIN artifact_runtime_owner_binding b ON b.artifact=a.id WHERE v.id=$1 AND s.id=$2 AND v.execution=$3 AND s.backend='object_segments' AND NOT s.sealed AND s.close_phase IS NULL AND v.body_state='pending' AND a.classification='runtime_log' AND a.visibility='private' AND b.owner_type='action' AND b.owner_id=e.action AND b.managing_pack=ac.pack AND $4>clock_timestamp() AND $4<=clock_timestamp()+interval '60 seconds')")
   .bind(input.version).bind(input.stream).bind(input.execution).bind(input.finalize_by).fetch_one(&mut *conn).await?;
        if !bound {
            return Err(denied());
        }
        let existing: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT sequence,size_bytes,sha256 FROM log_segment WHERE stream=$1 ORDER BY sequence",
        )
        .bind(input.stream)
        .fetch_all(&mut *conn)
        .await?;
        let grants:Vec<(i64,i64,String)>=sqlx::query_as("SELECT segment_sequence,expected_size,expected_sha256 FROM artifact_upload_grant WHERE artifact_version=$1 AND segment_sequence IS NOT NULL").bind(input.version).fetch_all(&mut *conn).await?;
        for (sequence, size, hash) in existing.into_iter().chain(grants) {
            if input
                .segments
                .get(sequence as usize)
                .is_none_or(|s| s.sequence != sequence || s.size_bytes != size || s.sha256 != hash)
            {
                return Err(denied());
            }
        }
        sqlx::query("INSERT INTO log_delivery_intent(execution,artifact_version,stream,worker,source_instance,delivery_instance,attempt,owner,custody,segments,total_bytes,sha256,truncated,authenticated_manifest,finalize_by) VALUES($1,$2,$3,$4,$5,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) ON CONFLICT(artifact_version) DO NOTHING")
   .bind(input.execution).bind(input.version).bind(input.stream).bind(input.worker).bind(input.instance).bind(attempt).bind(owner).bind(input.custody).bind(sqlx::types::Json(&input.segments)).bind(input.total_bytes).bind(&input.sha256).bind(input.truncated).bind(&input.authenticated_manifest).bind(input.finalize_by).execute(&mut *conn).await?;
        let intent = LogDeliveryRepository::find(&mut *conn, input.version)
            .await?
            .ok_or_else(denied)?;
        if intent.execution != input.execution
            || intent.stream != input.stream
            || intent.worker != input.worker
            || intent.source_instance != input.instance
            || intent.attempt != attempt
            || intent.owner != owner
            || intent.custody != input.custody
            || intent.segments.0 != input.segments
            || intent.total_bytes != input.total_bytes
            || intent.sha256 != input.sha256
            || intent.truncated != input.truncated
            || intent.authenticated_manifest != input.authenticated_manifest
            || intent.finalize_by != input.finalize_by
            || intent.state != "prepared"
        {
            return Err(denied());
        }
        Ok(intent)
    }
    /// Prove absence and publish its cancellation tombstone under exclusive
    /// graph admission. Queued/late reserve and prepare can never revive it.
    async fn cancel_absent_in(
        conn: &mut PgConnection,
        worker: i64,
        current_instance: Uuid,
        request: &ReserveCustody,
    ) -> Result<bool> {
        Graph::lock_reclamation(conn).await?;
        LogDeliveryRepository::require_instance(conn, worker, current_instance).await?;
        let b = &request.binding;
        if let Some(intent) = LogDeliveryRepository::find(&mut *conn, b.version).await? {
            if intent.worker != worker
                || intent.execution != b.execution
                || intent.stream != b.stream
                || intent.source_instance != b.source_instance
                || intent.attempt != b.attempt
                || intent.custody != b.custody
                || intent.finalize_by != micros(request.finalize_by)?
            {
                return Err(denied());
            }
            return Ok(false);
        }
        if let Some(operation) = Self::find(conn, b.custody).await? {
            operation.matches(request, worker)?;
            if operation.state == "reserved" {
                sqlx::query("UPDATE log_custody_operation SET state='cancelled' WHERE custody=$1 AND state='reserved'")
                    .bind(b.custody).execute(conn).await?;
            }
            return Ok(true);
        }
        // Without an existing operation we require exact retained target proof
        // before writing a tombstone. Missing proof denies settlement; a timeout,
        // lost source or changed incarnation is never itself absence evidence.
        if b.source_instance != current_instance {
            return Err(denied());
        }
        let target: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id JOIN artifact_version v ON v.execution=e.id JOIN log_stream s ON s.artifact_version=v.id WHERE e.id=$1 AND e.worker=$2 AND a.worker=$2 AND a.attempt=$3 AND a.producer_kind='worker' AND v.id=$4 AND s.id=$5)")
            .bind(b.execution).bind(worker).bind(b.attempt).bind(b.version).bind(b.stream).fetch_one(&mut *conn).await?;
        if !target {
            return Err(denied());
        }
        sqlx::query("INSERT INTO log_custody_operation(custody,execution,artifact_version,stream,worker,source_instance,attempt,finalize_by,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'cancelled') ON CONFLICT DO NOTHING")
            .bind(b.custody).bind(b.execution).bind(b.version).bind(b.stream).bind(worker)
            .bind(b.source_instance).bind(b.attempt).bind(micros(request.finalize_by)?).execute(&mut *conn).await?;
        let operation = Self::find(conn, b.custody).await?.ok_or_else(denied)?;
        operation.matches(request, worker)?;
        if operation.state != "cancelled" {
            return Err(denied());
        }
        Ok(true)
    }
}
