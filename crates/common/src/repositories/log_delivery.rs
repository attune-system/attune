//! Exact, bounded custody delegated before an execution's live attempt closes.
//! Does not authorize action API access or allocate arbitrary new log versions.
use super::delegated_execution::{DelegatedExecutionRepository, LogProducerBinding};
pub use crate::models::log_delivery::{DeliverySegment, LogDeliveryIntent};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use sqlx::{Executor, PgConnection, Postgres};
use uuid::Uuid;

const SELECT_COLUMNS:&str="id,execution,artifact_version,stream,worker,source_instance,delivery_instance,attempt,owner,custody,state,revision,segments,total_bytes,sha256,truncated,authenticated_manifest,finalize_by,failure,created,updated";
#[derive(Clone)]
pub struct PrepareLogDelivery {
    pub execution: i64,
    pub version: i64,
    pub stream: i64,
    pub worker: i64,
    pub instance: Uuid,
    pub custody: Uuid,
    pub segments: Vec<DeliverySegment>,
    pub total_bytes: i64,
    pub sha256: String,
    pub truncated: bool,
    pub authenticated_manifest: Option<String>,
    pub finalize_by: DateTime<Utc>,
}
fn denied() -> Error {
    Error::PermissionDenied("Log delivery custody is unavailable".into())
}
fn valid_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl PrepareLogDelivery {
    pub fn validate(&self) -> Result<()> {
        if [self.execution, self.version, self.stream, self.worker]
            .iter()
            .any(|n| *n <= 0)
            || self.instance.is_nil()
            || self.custody.is_nil()
            || self.segments.is_empty()
            || self.segments.len() > 16_384
            || !valid_digest(&self.sha256)
            || self.total_bytes <= 0
        {
            return Err(denied());
        }
        let mut size = 0i64;
        for (n, s) in self.segments.iter().enumerate() {
            if s.sequence != n as i64 || s.size_bytes <= 0 || !valid_digest(&s.sha256) {
                return Err(denied());
            }
            size = size.checked_add(s.size_bytes).ok_or_else(denied)?;
        }
        if size != self.total_bytes {
            return Err(denied());
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct LogDeliveryFence {
    pub intent: LogDeliveryIntent,
}
impl LogDeliveryFence {
    pub fn ensure_fresh(&self) -> Result<()> {
        if self.intent.finalize_by <= Utc::now() {
            Err(denied())
        } else {
            Ok(())
        }
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM log_delivery_intent i JOIN artifact_version v ON v.id=i.artifact_version JOIN log_stream s ON s.id=i.stream AND s.artifact_version=v.id JOIN artifact a ON a.id=v.artifact JOIN artifact_runtime_owner_binding b ON b.artifact=a.id JOIN execution e ON e.id=i.execution JOIN action ac ON ac.id=e.action WHERE i.id=$1 AND i.revision=$2 AND i.state IN ('prepared','active') AND i.finalize_by>clock_timestamp() AND v.execution=i.execution AND s.backend='object_segments' AND a.classification='runtime_log' AND a.visibility='private' AND b.owner_type='action' AND b.owner_id=ac.id AND b.managing_pack=ac.pack AND ac.id=(e.executable_snapshot#>>'{executable,action,id}')::bigint AND ac.pack=(e.executable_snapshot#>>'{executable,action,pack}')::bigint)").bind(self.intent.id).bind(self.intent.revision).fetch_one(conn).await?;
        if !valid {
            return Err(denied());
        }
        self.ensure_fresh()
    }
    pub fn producer(&self) -> LogProducerBinding {
        LogProducerBinding::Worker {
            execution: self.intent.execution,
            worker: self.intent.worker,
            attempt: self.intent.attempt,
        }
    }
    pub fn append(&self, sequence: i64, bytes: i64, sha256: &str) -> Result<()> {
        if self
            .intent
            .segments
            .0
            .get(usize::try_from(sequence).map_err(|_| denied())?)
            .is_none_or(|s| s.sequence != sequence || s.size_bytes != bytes || s.sha256 != sha256)
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn seal(&self, sequences: i64, bytes: i64) -> Result<()> {
        if sequences != self.intent.segments.len() as i64 || bytes != self.intent.total_bytes {
            return Err(denied());
        }
        Ok(())
    }
}
#[derive(sqlx::FromRow)]
struct AvailabilityRow {
    version: i64,
    stream: i64,
    body: String,
    sealed: bool,
    intent: Option<String>,
    revision: Option<i64>,
    finalize_by: Option<DateTime<Utc>>,
    future: bool,
}
pub struct LogDeliveryRepository;
impl LogDeliveryRepository {
    /// Reconcile a restart orphan against any producer COMMIT still settling.
    /// Every prepare holds shared graph admission; exclusive admission therefore
    /// waits for those transactions before absence becomes deletion evidence.
    pub async fn custody_absent(conn: &mut PgConnection, version: i64) -> Result<bool> {
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(conn)
            .await?;
        Ok(Self::find(conn, version).await?.is_none())
    }

    /// Caller has already authorized this execution's operational metadata.
    pub async fn availability(
        conn: &mut PgConnection,
        execution: &crate::models::Execution,
    ) -> Result<std::collections::BTreeMap<String, crate::models::log_availability::LogAvailability>>
    {
        use crate::models::log_availability::{LogAvailability, LogAvailabilityState as State};
        let mut result = std::collections::BTreeMap::new();
        for name in ["stdout", "stderr"] {
            let row:Option<AvailabilityRow>=sqlx::query_as("SELECT v.id AS version,s.id AS stream,v.body_state::text AS body,s.sealed,i.state AS intent,i.revision,i.finalize_by,COALESCE(i.finalize_by>clock_timestamp(),FALSE) AS future FROM artifact_version v JOIN log_stream s ON s.artifact_version=v.id LEFT JOIN log_delivery_intent i ON i.artifact_version=v.id WHERE v.execution=$1 AND v.meta->>'stream'=$2 ORDER BY v.id DESC LIMIT 1").bind(execution.id).bind(name).fetch_optional(&mut *conn).await?;
            let availability = if let Some(AvailabilityRow {
                version,
                stream,
                body,
                sealed,
                intent,
                revision,
                finalize_by,
                future,
            }) = row
            {
                let state = if body == "ready" && sealed {
                    State::Ready
                } else if matches!(intent.as_deref(), Some("prepared" | "active")) && future {
                    State::Finalizing
                } else if intent.is_some()
                    || matches!(
                        execution.status,
                        crate::models::ExecutionStatus::Completed
                            | crate::models::ExecutionStatus::Failed
                            | crate::models::ExecutionStatus::Cancelled
                            | crate::models::ExecutionStatus::Timeout
                            | crate::models::ExecutionStatus::Abandoned
                    )
                {
                    State::Incomplete
                } else {
                    State::Finalizing
                };
                LogAvailability {
                    state,
                    artifact_version: Some(version),
                    stream: Some(stream),
                    revision,
                    finalize_by,
                    retry_after_ms: (state == State::Finalizing).then_some(500),
                }
            } else {
                LogAvailability {
                    state: if crate::runtime_log_completion::execution_stream_is_empty(
                        execution, name,
                    ) {
                        State::Empty
                    } else if execution
                        .result
                        .as_ref()
                        .and_then(|r| r.get("runtime_log_capture"))
                        .and_then(|c| c.get(name))
                        .and_then(|s| s.as_str())
                        == Some("incomplete")
                    {
                        State::Incomplete
                    } else {
                        State::Unknown
                    },
                    artifact_version: None,
                    stream: None,
                    revision: None,
                    finalize_by: None,
                    retry_after_ms: None,
                }
            };
            result.insert(name.to_owned(), availability);
        }
        Ok(result)
    }
    pub async fn pending_for_worker(
        conn: &mut PgConnection,
        worker: i64,
    ) -> Result<Vec<LogDeliveryIntent>> {
        Ok(sqlx::query_as(&format!("SELECT {SELECT_COLUMNS} FROM log_delivery_intent WHERE worker=$1 AND state IN ('prepared','active') ORDER BY id LIMIT 4096")).bind(worker).fetch_all(conn).await?)
    }

    pub async fn find<'e, E: Executor<'e, Database = Postgres>>(
        executor: E,
        version: i64,
    ) -> Result<Option<LogDeliveryIntent>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM log_delivery_intent WHERE artifact_version=$1"
        ))
        .bind(version)
        .fetch_optional(executor)
        .await?)
    }
    /// Call in an outer SQLx transaction after graph/source admission and before
    /// target locks. Byte custody must already be synchronized by the worker.
    pub async fn prepare(
        conn: &mut PgConnection,
        mut input: PrepareLogDelivery,
    ) -> Result<LogDeliveryIntent> {
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
        Self::require_instance(conn, input.worker, input.instance).await?;
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
        let intent = Self::find(&mut *conn, input.version)
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
    pub async fn require_instance(
        conn: &mut PgConnection,
        worker: i64,
        instance: Uuid,
    ) -> Result<()> {
        if instance.is_nil() {
            return Err(denied());
        }
        // Fence incarnation/eligibility changes without locking the worker row:
        // heartbeats and active/busy updates must remain independent of delivery.
        // The matching worker trigger takes this lock exclusively on replacement.
        sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended(current_schema() || ':log-delivery-worker:' || $1::text, 0))")
            .bind(worker).execute(&mut *conn).await?;
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worker WHERE id=$1 AND capabilities->>'log_delivery_instance'=$2 AND status IN ('active','busy'))").bind(worker).bind(instance.to_string()).fetch_one(conn).await?;
        if !valid {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn require(
        conn: &mut PgConnection,
        version: i64,
        worker: i64,
        instance: Uuid,
    ) -> Result<LogDeliveryFence> {
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_capture(conn).await?;
        let initial = Self::find(&mut *conn, version).await?.ok_or_else(denied)?;
        let selected = DelegatedExecutionRepository::validate(conn, initial.execution).await?;
        if !selected.explicit || selected.owner != Some(initial.owner) {
            return Err(denied());
        }
        Self::require_instance(conn, worker, instance).await?;
        let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=$1 AND e.worker=$2 AND a.worker=e.worker AND a.attempt=$3 AND a.producer_kind='worker' FOR SHARE OF e,a)").bind(initial.execution).bind(worker).bind(initial.attempt).fetch_one(&mut *conn).await?;
        if !bound {
            return Err(denied());
        }
        let intent: LogDeliveryIntent = sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM log_delivery_intent WHERE artifact_version=$1 FOR SHARE"
        ))
        .bind(version)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        if intent.worker != worker
            || intent.delivery_instance != instance
            || intent.state != "active"
        {
            return Err(denied());
        }
        let fence = LogDeliveryFence { intent };
        fence.ensure_fresh_on(conn).await?;
        Ok(fence)
    }
    pub async fn claim(
        conn: &mut PgConnection,
        version: i64,
        worker: i64,
        instance: Uuid,
        custody: Uuid,
    ) -> Result<LogDeliveryIntent> {
        super::file_evidence_retention::FileEvidenceRetentionRepository::lock_capture(conn).await?;
        Self::require_instance(conn, worker, instance).await?;
        let initial = Self::find(&mut *conn, version).await?.ok_or_else(denied)?;
        let selected = DelegatedExecutionRepository::validate(conn, initial.execution).await?;
        if selected.owner != Some(initial.owner) || !selected.explicit {
            return Err(denied());
        }
        let updated=sqlx::query("UPDATE log_delivery_intent i SET delivery_instance=$3 WHERE artifact_version=$1 AND worker=$2 AND custody=$4 AND state IN ('prepared','active') AND finalize_by>clock_timestamp() AND EXISTS(SELECT 1 FROM execution e JOIN delegated_execution_attempt a ON a.execution=e.id WHERE e.id=i.execution AND e.worker=i.worker AND a.worker=i.worker AND a.attempt=i.attempt AND a.producer_kind='worker')").bind(version).bind(worker).bind(instance).bind(custody).execute(&mut *conn).await?.rows_affected();
        if updated != 1 {
            return Err(denied());
        }
        Self::find(conn, version).await?.ok_or_else(denied)
    }
    pub async fn mark_ready(conn: &mut PgConnection, version: i64) -> Result<()> {
        let updated=sqlx::query("UPDATE log_delivery_intent SET state='ready' WHERE artifact_version=$1 AND state IN ('prepared','active','ready')").bind(version).execute(conn).await?.rows_affected();
        if updated != 1 {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn mark_incomplete(
        conn: &mut PgConnection,
        version: i64,
        worker: i64,
        custody: Uuid,
        reason: &str,
    ) -> Result<()> {
        if !matches!(reason, "deadline" | "custody_lost" | "delivery_failed") {
            return Err(denied());
        }
        sqlx::query("UPDATE log_delivery_intent SET state='incomplete',failure=$4 WHERE artifact_version=$1 AND worker=$2 AND custody=$3 AND state IN ('prepared','active') AND NOT EXISTS(SELECT 1 FROM artifact_version WHERE id=$1 AND body_state='ready')").bind(version).bind(worker).bind(custody).bind(reason).execute(conn).await?;
        Ok(())
    }
}
