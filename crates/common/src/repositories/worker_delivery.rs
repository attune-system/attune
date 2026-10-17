//! Bounded delivery capabilities, never execution-token terminal grace.
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::{execution_admission::ExecutionAdmissionRepository, runtime_authority::RuntimeOwner};
use crate::{Error, Result};

/// Validated source-stage liveness, never a stored/serialized permit field.
/// Checking it after target/file waits acquires no authority or approval row lock.
#[derive(Clone)]
pub struct DeliveryEffectFence {
    context: Uuid,
    deadline: DateTime<Utc>,
    approval: super::runtime_authority::ApprovalEffectFence,
}

impl DeliveryEffectFence {
    /// Pure final handoff check on the already-validated independent fences.
    /// A commit/body wait cannot extend either the permit or input consent.
    pub fn ensure_fresh(&self) -> Result<()> {
        self.approval.ensure_fresh()?;
        self.check_at(Utc::now())
    }
    pub async fn ensure_fresh_on(&self, connection: &mut PgConnection) -> Result<()> {
        self.approval.ensure_fresh_on(connection).await?;
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *connection)
            .await?;
        self.check_at(now)
    }
    fn check_at(&self, now: DateTime<Utc>) -> Result<()> {
        // Supplement A's opaque validation, never replace it with a cached
        // timestamp. Test BOTH retained bounds at the last DB sample so expiry
        // between these two clock queries cannot outlive either deadline.
        if now >= self.deadline
            || self
                .approval
                .approval_deadline()
                .is_some_and(|expiry| now >= expiry)
        {
            return Err(denied());
        }
        Ok(())
    }
}

#[derive(Clone, FromRow)]
pub struct DeliveryContext {
    /// Opaque context token, not the table's BIGINT primary key.
    pub id: Uuid,
    pub execution: i64,
    pub admission: i64,
    pub attempt: Uuid,
    pub worker: i64,
    pub worker_instance: Uuid,
    pub deadline: DateTime<Utc>,
}

/// Contains no bytes, pathname, or policy. Do not log or expose the permit ID.
#[derive(Clone, FromRow)]
pub struct DeliveryPermit {
    /// Opaque permit token, not the table's BIGINT primary key.
    pub id: Uuid,
    pub context: Uuid,
    pub artifact_version: i64,
    pub log_stream: Option<i64>,
    pub max_bytes: i64,
    pub max_sequence: i64,
    pub publication_action: String,
    pub max_plaintext_bytes: Option<i64>,
}

#[derive(Clone, Copy)]
pub enum DeliveryOperation {
    Append { sequence: i64, end: i64 },
    Seal { sequences: i64, bytes: i64 },
    Complete { bytes: i64 },
}

fn denied() -> Error {
    Error::PermissionDenied("Worker delivery capability unavailable or outside its bounds".into())
}

impl DeliveryPermit {
    pub fn allows(&self, stream: Option<i64>, operation: DeliveryOperation) -> bool {
        if stream != self.log_stream {
            return false;
        }
        match operation {
            DeliveryOperation::Append { sequence, end } => {
                self.log_stream.is_some()
                    && sequence >= 0
                    && sequence < self.max_sequence
                    && end >= 0
                    && end <= self.max_bytes
            }
            DeliveryOperation::Seal { sequences, bytes } => {
                self.log_stream.is_some()
                    && sequences >= 0
                    && sequences <= self.max_sequence
                    && bytes >= 0
                    && bytes <= self.max_bytes
            }
            DeliveryOperation::Complete { bytes } => {
                self.log_stream.is_none() && bytes >= 0 && bytes <= self.max_bytes
            }
        }
    }
}

pub struct WorkerDeliveryRepository;
impl WorkerDeliveryRepository {
    /// Prelaunch allocation is not terminal delivery. It cannot create fresh
    /// versions after cancellation or under a stale/reassigned launch context.
    pub async fn require_launch_context(
        connection: &mut PgConnection,
        context: &DeliveryContext,
    ) -> Result<(
        super::runtime_authority::RuntimeAdmissionRecord,
        DeliveryEffectFence,
    )> {
        let admission = ExecutionAdmissionRepository::require(
            connection,
            RuntimeOwner::Execution(context.execution),
        )
        .await?;
        let approval =
            super::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(
                connection,
                admission.id,
            )
            .await?;
        let persisted = Self::for_execution(connection, context.execution)
            .await?
            .ok_or_else(denied)?;
        let current: Option<(Option<i64>, String, Uuid)> = sqlx::query_as(
            "SELECT e.worker,e.status::text,f.attempt FROM execution e JOIN runtime_authority_execution_fence f \
             ON f.execution=e.id AND f.admission=$2 WHERE e.id=$1 FOR SHARE OF e,f")
            .bind(context.execution).bind(admission.id).fetch_optional(&mut *connection).await?;
        if persisted.id != context.id
            || persisted.admission != admission.id
            || persisted.attempt != context.attempt
            || persisted.worker != context.worker
            || persisted.worker_instance != context.worker_instance
            || persisted.deadline != context.deadline
            || persisted.deadline <= Utc::now()
            || current != Some((Some(persisted.worker), "running".into(), persisted.attempt))
        {
            return Err(denied());
        }
        let effect = DeliveryEffectFence {
            context: context.id,
            deadline: context.deadline,
            approval,
        };
        effect.ensure_fresh_on(connection).await?;
        Ok((admission, effect))
    }
    pub async fn record_shared_append(
        connection: &mut PgConnection,
        stream: i64,
        end: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE log_stream SET total_bytes=$2,next_sequence=next_sequence+1 WHERE id=$1 AND NOT sealed AND backend='shared_file'")
            .bind(stream).bind(end).execute(connection).await?;
        Ok(())
    }
    /// A delayed Running update cannot resurrect a terminal/canceling execution.
    pub async fn start(connection: &mut PgConnection, execution: i64, worker: i64) -> Result<()> {
        let changed = sqlx::query(
            "UPDATE execution SET status='running',started_at=COALESCE(started_at,NOW()) \
            WHERE id=$1 AND worker=$2 AND status IN ('scheduled','running')",
        )
        .bind(execution)
        .bind(worker)
        .execute(connection)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(denied());
        }
        Ok(())
    }

    /// Register before launching code, including executions with an empty API
    /// selection. The admission-owned attempt is not an API credential.
    pub async fn register(
        connection: &mut PgConnection,
        execution: i64,
        worker: i64,
        instance: Uuid,
        deadline: DateTime<Utc>,
    ) -> Result<DeliveryContext> {
        if instance.is_nil() || deadline <= Utc::now() {
            return Err(denied());
        }
        let admission =
            ExecutionAdmissionRepository::require(connection, RuntimeOwner::Execution(execution))
                .await?;
        let approval =
            super::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(
                connection,
                admission.id,
            )
            .await?;
        let assigned: Option<(Option<i64>, String)> =
            sqlx::query_as("SELECT worker,status::text FROM execution WHERE id=$1 FOR SHARE")
                .bind(execution)
                .fetch_optional(&mut *connection)
                .await?;
        if assigned != Some((Some(worker), "running".into())) {
            return Err(denied());
        }
        let attempt: Uuid = sqlx::query_scalar("SELECT attempt FROM runtime_authority_execution_fence WHERE execution=$1 AND admission=$2 FOR SHARE")
            .bind(execution).bind(admission.id).fetch_one(&mut *connection).await?;
        approval.ensure_fresh_on(connection).await?;
        sqlx::query("INSERT INTO worker_delivery_context(execution,admission,attempt,worker,worker_instance,deadline) \
            VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(execution) DO NOTHING")
            .bind(execution).bind(admission.id).bind(attempt).bind(worker).bind(instance).bind(deadline)
            .execute(&mut *connection).await?;
        let context = Self::for_execution(connection, execution)
            .await?
            .ok_or_else(denied)?;
        if context.admission != admission.id
            || context.attempt != attempt
            || context.worker != worker
            || context.worker_instance != instance
            || context.deadline <= Utc::now()
        {
            return Err(denied());
        }
        DeliveryEffectFence {
            context: context.id,
            deadline: context.deadline,
            approval,
        }
        .ensure_fresh_on(connection)
        .await?;
        Ok(context)
    }

    pub async fn for_execution(
        connection: &mut PgConnection,
        execution: i64,
    ) -> Result<Option<DeliveryContext>> {
        Ok(sqlx::query_as("SELECT token AS id,execution,admission,attempt,worker,worker_instance,deadline FROM worker_delivery_context WHERE execution=$1")
            .bind(execution).fetch_optional(connection).await?)
    }

    /// Invoke in the transaction allocating the exact version/stream. The source
    /// fence must already be held before artifact locks are acquired.
    #[allow(clippy::too_many_arguments)]
    pub async fn bind(
        connection: &mut PgConnection,
        context: &DeliveryContext,
        version: i64,
        stream: Option<i64>,
        max_bytes: i64,
        max_sequence: i64,
        publication_action: crate::rbac::Action,
        effect: &DeliveryEffectFence,
    ) -> Result<DeliveryPermit> {
        Self::bind_limits(
            connection,
            context,
            version,
            stream,
            max_bytes,
            max_sequence,
            publication_action,
            None,
            effect,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn bind_encrypted_log(
        connection: &mut PgConnection,
        context: &DeliveryContext,
        version: i64,
        stream: i64,
        max_bytes: i64,
        max_sequence: i64,
        max_plaintext_bytes: i64,
        effect: &DeliveryEffectFence,
    ) -> Result<DeliveryPermit> {
        Self::bind_limits(
            connection,
            context,
            version,
            Some(stream),
            max_bytes,
            max_sequence,
            crate::rbac::Action::Update,
            Some(max_plaintext_bytes),
            effect,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn bind_limits(
        connection: &mut PgConnection,
        context: &DeliveryContext,
        version: i64,
        stream: Option<i64>,
        max_bytes: i64,
        max_sequence: i64,
        publication_action: crate::rbac::Action,
        max_plaintext_bytes: Option<i64>,
        effect: &DeliveryEffectFence,
    ) -> Result<DeliveryPermit> {
        if effect.context != context.id
            || effect.deadline != context.deadline
            || max_bytes < 0
            || max_sequence < 0
            || max_plaintext_bytes.is_some_and(|cap| cap < 0 || stream.is_none())
            || context.deadline <= Utc::now()
            || !matches!(
                publication_action,
                crate::rbac::Action::Create | crate::rbac::Action::Update
            )
        {
            return Err(denied());
        }
        let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_version v JOIN worker_delivery_context c ON c.token=$4 \
            WHERE v.id=$1 AND v.execution=$2 AND c.execution=$2 AND c.admission=$5 AND c.attempt=$6 \
            AND c.worker=$7 AND c.worker_instance=$8 AND c.deadline=$9 \
            AND ($3::bigint IS NULL OR EXISTS(SELECT 1 FROM log_stream s WHERE s.id=$3 AND s.artifact_version=v.id)))")
            .bind(version).bind(context.execution).bind(stream).bind(context.id).bind(context.admission).bind(context.attempt)
            .bind(context.worker).bind(context.worker_instance).bind(context.deadline).fetch_one(&mut *connection).await?;
        if !matches {
            return Err(denied());
        }
        effect.ensure_fresh_on(connection).await?;
        let permit=sqlx::query_as("INSERT INTO worker_delivery_permit(context,artifact_version,log_stream,max_bytes,max_sequence,owner_digest,publication_action,max_plaintext_bytes) \
            SELECT $1,$2,$3,$4,$5,md5(jsonb_build_array(b.artifact,b.owner_type::text,b.owner_ref,b.owner_id,b.managing_pack)::text),$6,$7 \
            FROM artifact_version v JOIN artifact_runtime_owner_binding b ON b.artifact=v.artifact WHERE v.id=$2 \
            RETURNING token AS id,context,artifact_version,log_stream,max_bytes,max_sequence,publication_action,max_plaintext_bytes")
            .bind(context.id).bind(version).bind(stream).bind(max_bytes).bind(max_sequence)
            .bind(if publication_action == crate::rbac::Action::Create { "create" } else { "update" }).bind(max_plaintext_bytes).fetch_one(&mut *connection).await?;
        effect.ensure_fresh_on(connection).await?;
        Ok(permit)
    }

    pub async fn for_version(
        connection: &mut PgConnection,
        version: i64,
    ) -> Result<Option<DeliveryPermit>> {
        Ok(sqlx::query_as("SELECT token AS id,context,artifact_version,log_stream,max_bytes,max_sequence,publication_action,max_plaintext_bytes FROM worker_delivery_permit WHERE artifact_version=$1")
            .bind(version).fetch_optional(connection).await?)
    }

    /// Call after locking the artifact, before any version/stream locks or write.
    pub async fn require_owner_binding(
        connection: &mut PgConnection,
        permit: Uuid,
        version: i64,
    ) -> Result<()> {
        let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worker_delivery_permit p \
            JOIN artifact_version v ON v.id=p.artifact_version JOIN artifact_runtime_owner_binding b ON b.artifact=v.artifact \
            WHERE p.token=$1 AND p.artifact_version=$2 AND p.owner_digest=md5(jsonb_build_array(b.artifact,b.owner_type::text,b.owner_ref,b.owner_id,b.managing_pack)::text))")
            .bind(permit).bind(version).fetch_one(connection).await?;
        if !matches {
            return Err(denied());
        }
        Ok(())
    }

    pub async fn require_log_owner(
        connection: &mut PgConnection,
        artifact: i64,
        action: i64,
        pack: i64,
    ) -> Result<()> {
        let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_runtime_owner_binding b \
            JOIN artifact a ON a.id=b.artifact WHERE a.id=$1 AND b.owner_type='action' AND b.owner_id=$2 AND b.managing_pack=$3 \
            AND a.classification='runtime_log' AND a.visibility='private')")
            .bind(artifact).bind(action).bind(pack).fetch_one(connection).await?;
        if !matches {
            return Err(denied());
        }
        Ok(())
    }

    /// Revalidates policy/sponsor/approvals/revocation even after execution API
    /// access ends. Keep this fence through the metadata or shared-volume write.
    pub async fn require(
        connection: &mut PgConnection,
        permit: Uuid,
        worker: i64,
        instance: Uuid,
        version: i64,
    ) -> Result<(DeliveryContext, DeliveryPermit, DeliveryEffectFence)> {
        let initial: Option<(i64,)> = sqlx::query_as("SELECT c.execution FROM worker_delivery_permit p JOIN worker_delivery_context c ON c.token=p.context WHERE p.token=$1")
            .bind(permit).fetch_optional(&mut *connection).await?;
        let execution = initial.ok_or_else(denied)?.0;
        let admission =
            ExecutionAdmissionRepository::require(connection, RuntimeOwner::Execution(execution))
                .await?;
        let approval =
            super::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(
                connection,
                admission.id,
            )
            .await?;
        let context = Self::for_execution(connection, execution)
            .await?
            .ok_or_else(denied)?;
        let current: Option<(Option<i64>, Uuid)> = sqlx::query_as("SELECT e.worker,f.attempt FROM execution e \
            JOIN runtime_authority_execution_fence f ON f.execution=e.id AND f.admission=$2 WHERE e.id=$1 FOR SHARE OF e,f")
            .bind(execution).bind(admission.id).fetch_optional(&mut *connection).await?;
        let bound = Self::for_version(connection, version)
            .await?
            .ok_or_else(denied)?;
        if context.admission != admission.id
            || context.worker != worker
            || context.worker_instance != instance
            || context.deadline <= Utc::now()
            || current != Some((Some(worker), context.attempt))
            || bound.id != permit
            || bound.context != context.id
        {
            return Err(denied());
        }
        let effect = DeliveryEffectFence {
            context: context.id,
            deadline: context.deadline,
            approval,
        };
        effect.ensure_fresh_on(connection).await?;
        Ok((context, bound, effect))
    }

    /// Reuse an already-validated PROCESS source after target waits. This does
    /// not validate source rows again or grant terminal process API access.
    pub fn process_effect(
        authority: &super::runtime_authority::ValidatedRuntimeAuthority,
        context: &DeliveryContext,
    ) -> Result<DeliveryEffectFence> {
        let binding = authority.credential_binding();
        if binding.execution_id != context.execution
            || binding.admission_id != context.admission
            || binding.attempt != context.attempt
        {
            return Err(denied());
        }
        Ok(DeliveryEffectFence {
            context: context.id,
            deadline: context.deadline,
            approval: authority.effect_fence().clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synchronous_handoff_preserves_independent_delivery_and_approval_deadlines() {
        let now = Utc::now();
        let approval = super::super::runtime_authority::ApprovalEffectFence::from_input_approval(
            Some(now + chrono::Duration::days(1)),
        )
        .unwrap();
        let effect = DeliveryEffectFence {
            context: Uuid::new_v4(),
            deadline: now + chrono::Duration::hours(1),
            approval,
        };
        effect.ensure_fresh().unwrap();
        assert!(effect.check_at(now + chrono::Duration::hours(1)).is_err());
        let later_delivery = DeliveryEffectFence {
            deadline: now + chrono::Duration::days(2),
            ..effect.clone()
        };
        assert!(later_delivery
            .check_at(now + chrono::Duration::days(1))
            .is_err());
        let expired_delivery = DeliveryEffectFence {
            deadline: now - chrono::Duration::days(1),
            ..effect
        };
        assert!(
            expired_delivery.ensure_fresh().is_err(),
            "post-commit checks cannot extend the native permit after its delivery window"
        );
    }
    #[test]
    fn delivery_operations_do_not_cross_streams_or_bounds() {
        let permit = DeliveryPermit {
            id: Uuid::new_v4(),
            context: Uuid::new_v4(),
            artifact_version: 5,
            log_stream: Some(7),
            max_bytes: 100,
            max_sequence: 4,
            publication_action: "update".into(),
            max_plaintext_bytes: None,
        };
        assert!(permit.allows(
            Some(7),
            DeliveryOperation::Append {
                sequence: 3,
                end: 100
            }
        ));
        for operation in [
            DeliveryOperation::Append {
                sequence: 4,
                end: 100,
            },
            DeliveryOperation::Append {
                sequence: 0,
                end: 101,
            },
            DeliveryOperation::Append {
                sequence: -1,
                end: 1,
            },
            DeliveryOperation::Complete { bytes: 1 },
        ] {
            assert!(!permit.allows(Some(7), operation));
        }
        assert!(!permit.allows(
            Some(8),
            DeliveryOperation::Seal {
                sequences: 4,
                bytes: 100
            }
        ));
        assert!(permit.allows(
            Some(7),
            DeliveryOperation::Seal {
                sequences: 4,
                bytes: 100
            }
        ));
        let file = DeliveryPermit {
            log_stream: None,
            ..permit
        };
        assert!(file.allows(None, DeliveryOperation::Complete { bytes: 100 }));
        assert!(!file.allows(None, DeliveryOperation::Complete { bytes: -1 }));
        assert!(!file.allows(
            None,
            DeliveryOperation::Seal {
                sequences: 0,
                bytes: 0
            }
        ));
    }
}
