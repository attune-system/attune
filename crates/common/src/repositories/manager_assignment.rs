//! Executor-owned placement and attempt-fenced API transitions.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    db::SessionLease,
    models::{
        Execution, ExecutionStatus, Id, ManagerExecutionAssignment, ManagerExecutionCompletion,
    },
    mq::ExecutionCompletedPayload,
    Error, Result,
};

use super::{
    delegated_execution::DelegatedExecutionRepository,
    execution::{
        ExecutionRepository, ExecutionTransition, UpdateExecutionInput, WorkerExecutionAssignment,
    },
    execution_runtime_admission::ExecutionAdmissionRepository,
    manager_session::ManagerSessionRecord,
    runtime_read::RuntimeReadRepository,
    schema_type::SchemaTypeRepository,
    worker_delivery::WorkerDeliveryRepository,
};

const ASSIGNMENT_COLUMNS: &str = "id, execution, worker, manager_session, instance, scheduled_at, \
    pack_release, pack_release_digest, deadline, claim_request, closed_at, finish_digest, final_status";
const LEASE_SECONDS: i32 = 30;
const PUBLISH_LEASE_SECONDS: i32 = 45;
pub const MANAGER_WORK_WAKEUP: &str = "manager_work_available";
pub const MANAGER_COMPLETION_WAKEUP: &str = "manager_completion_available";
pub const MANAGER_COMPLETION_SOURCE: &str = "manager-assignment";

#[derive(Clone, Copy)]
pub struct ManagerAssignmentScope {
    pub session: Id,
    pub instance: Uuid,
    pub bearer_expires_at: DateTime<Utc>,
}

pub enum ManagerClaim {
    Started(ManagerExecutionAssignment),
    Replay(ManagerExecutionAssignment),
}

pub enum ManagerLeaseRenewal {
    Extended(DateTime<Utc>),
    Cancel,
}

pub enum ManagerAssignmentRecovery {
    Reschedule(Id),
    Closed,
}

pub enum ManagerCompletionReceipt {
    Process(ManagerCompletionProcessing),
    AlreadyProcessed,
    Busy,
}

pub struct ManagerCompletionProcessing {
    lease: SessionLease,
    assignment: Id,
    lock: i64,
}

impl ManagerCompletionProcessing {
    pub async fn release(mut self, processed: bool) -> Result<()> {
        if processed {
            sqlx::query(
                "UPDATE manager_execution_completion SET processed_at = clock_timestamp()
                 WHERE assignment = $1 AND processed_at IS NULL",
            )
            .bind(self.assignment)
            .execute(&mut *self.lease)
            .await?;
        }
        unlock_completion(self.lease, self.lock).await
    }
}

async fn unlock_completion(mut lease: SessionLease, lock: i64) -> Result<()> {
    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(lock)
        .fetch_one(&mut *lease)
        .await?;
    if !unlocked {
        return Err(Error::InvalidState("Manager completion lock lost".into()));
    }
    lease.release_clean();
    Ok(())
}

enum ManagerFinish {
    Pending(ManagerExecutionAssignment, ExecutionStatus),
    Replay(ExecutionStatus),
}

pub struct ManagerAssignmentRepository;

fn denied() -> Error {
    Error::PermissionDenied("Manager assignment is stale or unavailable".into())
}

impl ManagerAssignmentRepository {
    /// Called by the server for a worker registration it already owns.
    /// Scheduling is enabled only after the runtime is ready to accept work.
    pub async fn bind_action_worker(
        tx: &mut Transaction<'_, Postgres>,
        worker: Id,
        manager: &ManagerSessionRecord,
    ) -> Result<()> {
        let bound: Option<Id> = sqlx::query_scalar(
            "INSERT INTO manager_action_worker (worker, manager_session)
             SELECT w.id, s.id FROM worker w JOIN manager_session s ON s.id = $2
             WHERE w.id = $1 AND w.worker_role = 'action' AND s.role = 'action'
               AND s.instance = $3 AND w.meta->>'registration_incarnation' = $3::uuid::text
               AND s.closed_at IS NULL AND s.expires_at > clock_timestamp()
             ON CONFLICT (worker) DO UPDATE SET manager_session = EXCLUDED.manager_session,
                 draining = CASE WHEN manager_action_worker.manager_session = EXCLUDED.manager_session
                                 THEN manager_action_worker.draining ELSE FALSE END
             RETURNING worker",
        )
        .bind(worker)
        .bind(manager.id)
        .bind(manager.instance)
        .fetch_optional(&mut **tx)
        .await?;
        bound.ok_or_else(denied)?;
        Ok(())
    }

    /// Persist an HTTP offer inside the executor's placement transaction.
    /// Unbound workers use ordinary broker dispatch. A stale binding fails
    /// placement so the same execution cannot be sent to both transports.
    pub async fn offer_if_bound(
        tx: &mut Transaction<'_, Postgres>,
        execution: &Execution,
        offer_seconds: i32,
    ) -> Result<bool> {
        let worker = execution.worker.ok_or_else(denied)?;
        if execution.status != ExecutionStatus::Scheduled || offer_seconds <= 0 {
            return Err(denied());
        }

        let binding: Option<(Id, Uuid, DateTime<Utc>, bool)> = sqlx::query_as(
            "SELECT m.manager_session, s.instance, s.expires_at,
                    NOT m.draining AND w.status IS NOT DISTINCT FROM 'active' AND NOT w.cordoned
                    AND w.worker_role = 'action' AND s.role = 'action' AND s.closed_at IS NULL
                    AND s.expires_at > clock_timestamp()
                    AND COALESCE(w.meta->>'registration_incarnation' = s.instance::text, FALSE)
             FROM manager_action_worker m JOIN manager_session s ON s.id = m.manager_session
             JOIN worker w ON w.id = m.worker
             WHERE m.worker = $1",
        )
        .bind(worker)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((session, instance, session_expiry, true)) = binding else {
            return if binding.is_none() {
                Ok(false)
            } else {
                Err(denied())
            };
        };
        let release = execution.pack_release.ok_or_else(denied)?;
        let digest = execution.pack_release_digest.as_ref().ok_or_else(denied)?;
        sqlx::query(
            "INSERT INTO manager_execution_assignment
                (execution, worker, manager_session, instance, scheduled_at, pack_release, pack_release_digest, deadline)
             VALUES ($1, $2, $3, $4, $5, $6, $7,
                     LEAST($9, clock_timestamp() + make_interval(secs => $8::integer)))",
        )
        .bind(execution.id)
        .bind(worker)
        .bind(session)
        .bind(instance)
        .bind(execution.updated)
        .bind(release)
        .bind(digest)
        .bind(offer_seconds)
        .bind(session_expiry)
        .execute(&mut **tx)
        .await?;
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(MANAGER_WORK_WAKEUP)
            .bind(session.to_string())
            .execute(&mut **tx)
            .await?;
        Ok(true)
    }

    /// Retain the registered session and transport through a manager operation.
    /// No executor placement path takes these locks while holding execution.
    async fn lock_worker(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
    ) -> Result<(Id, bool)> {
        sqlx::query_as(
            "SELECT m.worker, (m.draining OR w.cordoned OR w.status IS DISTINCT FROM 'active')
             FROM manager_action_worker m JOIN manager_session s ON s.id = m.manager_session
             JOIN worker w ON w.id = m.worker
             WHERE s.id = $1 AND s.instance = $2 AND s.role = 'action' AND s.closed_at IS NULL
             AND LEAST(s.expires_at, $3) > clock_timestamp()
             AND w.worker_role = 'action' AND w.meta->>'registration_incarnation' = s.instance::text
             FOR SHARE OF s, m, w",
        )
        .bind(scope.session)
        .bind(scope.instance)
        .bind(scope.bearer_expires_at)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)
    }

    async fn lock_assignment(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        id: Id,
        worker: Id,
    ) -> Result<ManagerExecutionAssignment> {
        sqlx::query_as(&format!(
            "SELECT {ASSIGNMENT_COLUMNS} FROM manager_execution_assignment
             WHERE id = $1 AND manager_session = $2 AND instance = $3 AND worker = $4 FOR UPDATE"
        ))
        .bind(id)
        .bind(scope.session)
        .bind(scope.instance)
        .bind(worker)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)
    }

    /// Recheck database time after all row lock waits and immediately before a
    /// claim/lease/settlement effect. The caller retains both session and binding.
    async fn check_live(
        conn: &mut PgConnection,
        scope: ManagerAssignmentScope,
        deadline: DateTime<Utc>,
    ) -> Result<()> {
        let current: bool = sqlx::query_scalar(
            "SELECT LEAST(expires_at, $2, $3) > clock_timestamp() AND closed_at IS NULL
             FROM manager_session WHERE id = $1 AND instance = $4",
        )
        .bind(scope.session)
        .bind(scope.bearer_expires_at)
        .bind(deadline)
        .bind(scope.instance)
        .fetch_optional(conn)
        .await?
        .unwrap_or(false);
        if !current {
            return Err(denied());
        }
        Ok(())
    }

    pub async fn next_offer(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
    ) -> Result<Option<ManagerExecutionAssignment>> {
        let (worker, draining) = Self::lock_worker(tx, scope).await?;
        if draining {
            return Ok(None);
        }
        let columns = ASSIGNMENT_COLUMNS
            .split(", ")
            .map(|column| format!("a.{}", column.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(sqlx::query_as(&format!(
            "SELECT {columns} FROM manager_execution_assignment a
             JOIN execution e ON e.id = a.execution AND e.worker = a.worker
             JOIN worker w ON w.id = a.worker
             WHERE a.manager_session = $1 AND a.worker = $2
             AND a.claim_request IS NULL AND a.closed_at IS NULL AND a.deadline > clock_timestamp()
             AND e.status = 'scheduled' AND e.updated = a.scheduled_at
             AND e.pack_release = a.pack_release AND e.pack_release_digest = a.pack_release_digest
             AND NOT w.cordoned AND w.status = 'active'
             ORDER BY a.id LIMIT 1",
        ))
        .bind(scope.session)
        .bind(worker)
        .fetch_optional(&mut **tx)
        .await?)
    }

    pub async fn claim(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        id: Id,
        request: Uuid,
    ) -> Result<ManagerClaim> {
        SchemaTypeRepository::lock_selection(tx).await?;
        let (worker, draining) = Self::lock_worker(tx, scope).await?;
        let row = Self::lock_assignment(tx, scope, id, worker).await?;
        if request.is_nil() || row.closed_at.is_some() {
            return Err(denied());
        }
        if let Some(current) = row.claim_request {
            let execution = Self::running_execution(tx, &row).await?;
            Self::check_live(tx, scope, row.deadline).await?;
            if current != request || execution.status != ExecutionStatus::Running {
                return Err(denied());
            }
            return Ok(ManagerClaim::Replay(row));
        }
        if draining {
            return Err(denied());
        }

        let recorded = ExecutionAdmissionRepository::is_recorded(tx, row.execution).await?;
        let assignment = WorkerExecutionAssignment {
            instance: row.instance,
            scheduled_at: row.scheduled_at,
            pack_release: Some(row.pack_release),
            pack_release_digest: Some(row.pack_release_digest.clone()),
            recorded,
        };
        let transition = ExecutionRepository::claim_worker_execution(
            tx,
            row.execution,
            worker,
            Some(&assignment),
        )
        .await?;
        if recorded {
            let execution = &transition.execution;
            if !execution.permission_set_refs.is_empty() {
                super::runtime_authority::RuntimeAuthorityRepository::begin_execution_attempt(
                    tx,
                    execution.id,
                    worker,
                    request,
                )
                .await?;
            }
            let action = &execution
                .executable_snapshot
                .as_ref()
                .ok_or_else(denied)?
                .executable
                .action;
            let timeout = Self::action_timeout(execution, action);
            let deadline = i64::try_from(timeout)
                .ok()
                .and_then(|seconds| seconds.checked_add(60))
                .and_then(chrono::Duration::try_seconds)
                .and_then(|duration| Utc::now().checked_add_signed(duration))
                .ok_or_else(denied)?;
            WorkerDeliveryRepository::register(tx, execution.id, worker, row.instance, deadline)
                .await?;
        }
        let row = sqlx::query_as(&format!(
            "WITH session_expiry AS (
                 SELECT expires_at FROM manager_session WHERE id = $5 AND closed_at IS NULL
             )
             UPDATE manager_execution_assignment
             SET claim_request = $2,
                 deadline = LEAST(expires_at, clock_timestamp() + make_interval(secs => $3::integer))
             FROM session_expiry WHERE id = $1 AND LEAST(deadline, $4, expires_at) > clock_timestamp()
             RETURNING {ASSIGNMENT_COLUMNS}",
        ))
        .bind(row.id)
        .bind(request)
        .bind(LEASE_SECONDS)
        .bind(scope.bearer_expires_at)
        .bind(scope.session)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        Ok(ManagerClaim::Started(row))
    }

    pub async fn drain(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
    ) -> Result<()> {
        let affected = sqlx::query(
            "UPDATE manager_action_worker m SET draining = TRUE
             FROM manager_session s, worker w
             WHERE m.manager_session = $1 AND s.id = m.manager_session AND s.instance = $2
             AND s.closed_at IS NULL AND s.role = 'action' AND w.id = m.worker
             AND w.meta->>'registration_incarnation' = s.instance::text
             AND LEAST(s.expires_at, $3) > clock_timestamp()",
        )
        .bind(scope.session)
        .bind(scope.instance)
        .bind(scope.bearer_expires_at)
        .execute(&mut **tx)
        .await?;
        if affected.rows_affected() != 1 {
            return Err(denied());
        }
        Ok(())
    }

    pub async fn renew(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        id: Id,
    ) -> Result<ManagerLeaseRenewal> {
        let (worker, _) = Self::lock_worker(tx, scope).await?;
        let row = Self::lock_assignment(tx, scope, id, worker).await?;
        let execution = Self::running_execution(tx, &row).await?;
        Self::check_live(tx, scope, row.deadline).await?;
        if execution.status == ExecutionStatus::Canceling {
            return Ok(ManagerLeaseRenewal::Cancel);
        }
        let deadline = sqlx::query_scalar(
            "UPDATE manager_execution_assignment a
             SET deadline = LEAST(s.expires_at, clock_timestamp() + make_interval(secs => $2::integer))
             FROM manager_session s WHERE a.id = $1 AND s.id = a.manager_session
             AND LEAST(a.deadline, s.expires_at, $3) > clock_timestamp() RETURNING a.deadline",
        )
        .bind(id)
        .bind(LEASE_SECONDS)
        .bind(scope.bearer_expires_at)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        Ok(ManagerLeaseRenewal::Extended(deadline))
    }

    async fn running_execution(
        tx: &mut Transaction<'_, Postgres>,
        row: &ManagerExecutionAssignment,
    ) -> Result<Execution> {
        if row.closed_at.is_some() || row.claim_request.is_none() {
            return Err(denied());
        }
        ExecutionRepository::find_by_id_for_update(tx, row.execution)
            .await?
            .filter(|execution| {
                execution.worker == Some(row.worker)
                    && execution.pack_release == Some(row.pack_release)
                    && execution.pack_release_digest.as_deref()
                        == Some(row.pack_release_digest.as_str())
                    && matches!(
                        execution.status,
                        ExecutionStatus::Running | ExecutionStatus::Canceling
                    )
            })
            .ok_or_else(denied)
    }

    pub async fn execution_for_transfer(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        id: Id,
    ) -> Result<Execution> {
        let (worker, _) = Self::lock_worker(tx, scope).await?;
        let row = Self::lock_assignment(tx, scope, id, worker).await?;
        let execution = Self::running_execution(tx, &row).await?;
        Self::check_live(tx, scope, row.deadline).await?;
        if execution.status != ExecutionStatus::Running {
            return Err(denied());
        }
        Ok(execution)
    }

    pub async fn execution_for_launch(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        id: Id,
        request: Uuid,
    ) -> Result<(Execution, DateTime<Utc>)> {
        let (worker, _) = Self::lock_worker(tx, scope).await?;
        let row = Self::lock_assignment(tx, scope, id, worker).await?;
        let execution = Self::running_execution(tx, &row).await?;
        if row.claim_request != Some(request) || execution.status != ExecutionStatus::Running {
            return Err(denied());
        }
        Self::check_live(tx, scope, row.deadline).await?;
        Ok((execution, row.deadline.min(scope.bearer_expires_at)))
    }

    pub async fn require_log_preparation(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        assignment: Id,
        execution: Id,
    ) -> Result<DateTime<Utc>> {
        let (worker, _) = Self::lock_worker(tx, scope).await?;
        let row = Self::lock_assignment(tx, scope, assignment, worker).await?;
        let current = Self::running_execution(tx, &row).await?;
        if current.id != execution {
            return Err(denied());
        }
        Self::check_live(tx, scope, row.deadline).await?;
        Ok(row.deadline)
    }

    pub async fn log_preparation_open(
        conn: &mut PgConnection,
        assignment: Id,
        session: Id,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT closed_at IS NULL AND deadline>clock_timestamp() AND manager_session=$2
             FROM manager_execution_assignment WHERE id=$1 FOR SHARE",
        )
        .bind(assignment)
        .bind(session)
        .fetch_optional(conn)
        .await?
        .unwrap_or(false))
    }

    pub async fn bind_child_attempt(
        conn: &mut PgConnection,
        assignment: Id,
        attempt: Uuid,
    ) -> Result<()> {
        let updated = sqlx::query(
            "UPDATE manager_execution_assignment SET child_attempt=$2
             WHERE id=$1 AND claim_request IS NOT NULL AND closed_at IS NULL
               AND (child_attempt IS NULL OR child_attempt=$2)",
        )
        .bind(assignment)
        .bind(attempt)
        .execute(conn)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(denied());
        }
        Ok(())
    }

    /// The existing credential consumer owns the execution lock. Retain its
    /// manager session without taking an assignment lock in reverse order.
    pub async fn retain_child_attempt(
        conn: &mut PgConnection,
        execution: Id,
        attempt: Uuid,
    ) -> Result<Option<DateTime<Utc>>> {
        let binding: Option<(DateTime<Utc>, bool)> = sqlx::query_as(
            "SELECT LEAST(a.deadline,s.expires_at), a.closed_at IS NULL AND s.closed_at IS NULL
                AND s.role='action' AND a.instance=s.instance
                AND EXISTS (SELECT 1 FROM manager_action_worker m JOIN worker w ON w.id=m.worker
                    WHERE m.worker=a.worker AND m.manager_session=s.id
                      AND w.meta->>'registration_incarnation'=s.instance::text)
             FROM manager_execution_assignment a JOIN manager_session s ON s.id=a.manager_session
             WHERE a.execution=$1 AND a.child_attempt=$2 FOR SHARE OF s",
        )
        .bind(execution)
        .bind(attempt)
        .fetch_optional(&mut *conn)
        .await?;
        match binding {
            None => Ok(None),
            Some((deadline, true)) => {
                Self::check_deadline(conn, deadline).await?;
                Ok(Some(deadline))
            }
            Some((_, false)) => Err(denied()),
        }
    }

    pub fn action_timeout(execution: &Execution, action: &crate::models::Action) -> u64 {
        [
            execution.timeout_seconds,
            execution
                .workflow_task
                .as_ref()
                .and_then(|task| task.timeout_seconds),
            action.timeout_seconds,
        ]
        .into_iter()
        .flatten()
        .find(|seconds| *seconds > 0)
        .map(|seconds| seconds as u64)
        .unwrap_or_else(crate::config::app_default_execution_timeout_seconds)
    }

    pub async fn check_deadline(conn: &mut PgConnection, deadline: DateTime<Utc>) -> Result<()> {
        if !sqlx::query_scalar::<_, bool>("SELECT $1 > clock_timestamp()")
            .bind(deadline)
            .fetch_one(conn)
            .await?
        {
            return Err(denied());
        }
        Ok(())
    }

    /// Reserve the assignment while the caller settles via the existing
    /// attempt authority and output classification path.
    async fn lock_finish(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        id: Id,
        digest: &str,
    ) -> Result<ManagerFinish> {
        let (worker, _) = Self::lock_worker(tx, scope).await?;
        let row = Self::lock_assignment(tx, scope, id, worker).await?;
        if let (Some(prior), Some(status)) = (&row.finish_digest, row.final_status) {
            Self::check_live(tx, scope, scope.bearer_expires_at).await?;
            return if prior == digest {
                Ok(ManagerFinish::Replay(status))
            } else {
                Err(denied())
            };
        }
        let execution = Self::running_execution(tx, &row).await?;
        Self::check_live(tx, scope, row.deadline).await?;
        Ok(ManagerFinish::Pending(row, execution.status))
    }

    /// Call after the common completion path, and before committing its
    /// transaction. Only the classified terminal row enters the outbox.
    async fn record_finish(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        row: &ManagerExecutionAssignment,
        digest: &str,
        transition: &ExecutionTransition,
    ) -> Result<()> {
        let execution = &transition.execution;
        if execution.id != row.execution
            || execution.worker != Some(row.worker)
            || execution.pack_release != Some(row.pack_release)
            || execution.pack_release_digest.as_deref() != Some(row.pack_release_digest.as_str())
        {
            return Err(denied());
        }
        let closed: Option<Id> = sqlx::query_scalar(
            "UPDATE manager_execution_assignment a SET closed_at = clock_timestamp(),
                 finish_digest = $4, final_status = $5
             FROM manager_session s WHERE a.id = $1 AND a.manager_session = $2 AND a.instance = $3
             AND s.id = a.manager_session AND s.closed_at IS NULL
             AND a.claim_request IS NOT NULL AND a.closed_at IS NULL
             AND LEAST(a.deadline, s.expires_at, $6) > clock_timestamp() RETURNING a.id",
        )
        .bind(row.id)
        .bind(scope.session)
        .bind(scope.instance)
        .bind(digest)
        .bind(execution.status)
        .bind(scope.bearer_expires_at)
        .fetch_optional(&mut **tx)
        .await?;
        closed.ok_or_else(denied)?;
        Self::queue_completion(tx, row.id, execution).await
    }

    pub async fn finish(
        tx: &mut Transaction<'_, Postgres>,
        scope: ManagerAssignmentScope,
        id: Id,
        digest: &str,
        status: ExecutionStatus,
        result: serde_json::Value,
        key: &str,
    ) -> Result<ExecutionStatus> {
        if !matches!(
            status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Timeout
                | ExecutionStatus::Cancelled
        ) {
            return Err(Error::validation("Invalid manager terminal status"));
        }
        SchemaTypeRepository::lock_selection(tx).await?;
        let (assignment, previous) = match Self::lock_finish(tx, scope, id, digest).await? {
            ManagerFinish::Replay(status) => return Ok(status),
            ManagerFinish::Pending(assignment, previous) => (assignment, previous),
        };
        let status = if previous == ExecutionStatus::Canceling {
            ExecutionStatus::Cancelled
        } else {
            status
        };
        let transition = if ExecutionAdmissionRepository::is_recorded(tx, assignment.execution)
            .await?
        {
            let delivery = WorkerDeliveryRepository::for_execution(tx, assignment.execution)
                .await?
                .ok_or_else(denied)?;
            if delivery.worker != assignment.worker
                || delivery.worker_instance != assignment.instance
            {
                return Err(denied());
            }
            WorkerDeliveryRepository::complete_result(tx, &delivery, status, result, key).await?
        } else {
            DelegatedExecutionRepository::require_worker_attempt(
                tx,
                assignment.execution,
                assignment.worker,
                &["running", "canceling"],
            )
            .await?;
            WorkerDeliveryRepository::complete_classified_result(
                tx,
                assignment.execution,
                assignment.worker,
                status,
                result,
                key,
            )
            .await?
        };
        Self::record_finish(tx, scope, &assignment, digest, &transition).await?;
        Ok(status)
    }

    async fn queue_completion(
        tx: &mut Transaction<'_, Postgres>,
        assignment: Id,
        execution: &Execution,
    ) -> Result<()> {
        let action = execution
            .action
            .or_else(|| {
                execution
                    .executable_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.executable.action.id)
            })
            .ok_or_else(denied)?;
        sqlx::query(
            "WITH queued AS (
                 INSERT INTO manager_execution_completion
                     (assignment, message_id, execution, action, action_ref, status, completed_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 RETURNING 1
             )
             SELECT pg_notify($8, '') FROM queued",
        )
        .bind(assignment)
        .bind(Uuid::new_v4())
        .bind(execution.id)
        .bind(action)
        .bind(&execution.action_ref)
        .bind(execution.status)
        .bind(execution.updated)
        .bind(MANAGER_COMPLETION_WAKEUP)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Recover one expired offer or attempt under the executor's ownership.
    /// The same assignment row serializes expiry with claim, renewal and finish.
    pub async fn recover_expired(pool: &PgPool) -> Result<Option<ManagerAssignmentRecovery>> {
        let mut tx = pool.begin().await?;
        let assignment: Option<ManagerExecutionAssignment> = sqlx::query_as(&format!(
            "SELECT {ASSIGNMENT_COLUMNS} FROM manager_execution_assignment
             WHERE closed_at IS NULL AND deadline <= clock_timestamp()
             ORDER BY deadline, id LIMIT 1 FOR UPDATE SKIP LOCKED",
        ))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(assignment) = assignment else {
            tx.commit().await?;
            return Ok(None);
        };
        let execution = ExecutionRepository::find_by_id_for_update(&mut tx, assignment.execution)
            .await?
            .filter(|execution| {
                execution.worker == Some(assignment.worker)
                    && execution.pack_release == Some(assignment.pack_release)
                    && execution.pack_release_digest.as_deref()
                        == Some(assignment.pack_release_digest.as_str())
            });
        let mut outcome = ManagerAssignmentRecovery::Closed;
        let mut final_status = None;
        if let Some(execution) = execution {
            if execution.status == ExecutionStatus::Cancelled {
                // A cancellation can settle an offer before the manager starts.
                // Existing status-change delivery does not cover this outbox.
                Self::queue_completion(&mut tx, assignment.id, &execution).await?;
                final_status = Some(execution.status);
            } else if assignment.claim_request.is_none()
                && execution.status == ExecutionStatus::Scheduled
                && execution.updated == assignment.scheduled_at
            {
                sqlx::query(
                    "UPDATE execution SET status = 'requested', worker = NULL, updated = NOW()
                     WHERE id = $1",
                )
                .bind(execution.id)
                .execute(&mut *tx)
                .await?;
                outcome = ManagerAssignmentRecovery::Reschedule(execution.id);
            } else if assignment.claim_request.is_some()
                && matches!(
                    execution.status,
                    ExecutionStatus::Running | ExecutionStatus::Canceling
                )
            {
                let status = if execution.status == ExecutionStatus::Canceling {
                    ExecutionStatus::Cancelled
                } else {
                    ExecutionStatus::Abandoned
                };
                RuntimeReadRepository::close_execution(&mut tx, execution.id).await?;
                let updated = ExecutionRepository::update_if_status(
                    &mut *tx,
                    execution.id,
                    execution.status,
                    UpdateExecutionInput {
                        status: Some(status),
                        result: Some(
                            serde_json::json!({"error": "Manager assignment lease expired"}),
                        ),
                        ..Default::default()
                    },
                )
                .await?
                .ok_or_else(denied)?;
                Self::queue_completion(&mut tx, assignment.id, &updated).await?;
                final_status = Some(status);
            }
        }
        sqlx::query(
            "UPDATE manager_execution_assignment SET closed_at = clock_timestamp(), final_status = $2
             WHERE id = $1",
        )
        .bind(assignment.id)
        .bind(final_status)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(outcome))
    }

    /// Call after locking the execution to serialize legacy remediation with
    /// placement. An open manager assignment is recovered by its own lease.
    pub(super) async fn owns_remediation(conn: &mut PgConnection, execution: Id) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM manager_execution_assignment
             WHERE execution = $1 AND closed_at IS NULL)",
        )
        .bind(execution)
        .fetch_one(conn)
        .await?)
    }

    /// Serialize concurrent delivery of a stable outbox event across executor
    /// replicas. A crash before the receipt leaves existing completion effects
    /// eligible to retry under their own repository fences.
    pub async fn begin_completion_processing(
        pool: &PgPool,
        message: Uuid,
        payload: &ExecutionCompletedPayload,
    ) -> Result<ManagerCompletionReceipt> {
        let mut lease = SessionLease::acquire(pool).await?;
        let lock: i64 = sqlx::query_scalar(
            "SELECT hashtextextended(current_schema() || ':manager-completion:' || $1::uuid::text, 0)",
        )
        .bind(message)
        .fetch_one(&mut *lease)
        .await?;
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(lock)
            .fetch_one(&mut *lease)
            .await?;
        if !acquired {
            lease.release_clean();
            return Ok(ManagerCompletionReceipt::Busy);
        }
        let record: Option<(Id, ExecutionStatus, bool)> = sqlx::query_as(
            "SELECT assignment, status, processed_at IS NOT NULL
             FROM manager_execution_completion WHERE message_id = $1 AND execution = $2
             AND action = $3 AND action_ref = $4 AND completed_at = $5",
        )
        .bind(message)
        .bind(payload.execution_id)
        .bind(payload.action_id)
        .bind(&payload.action_ref)
        .bind(payload.completed_at)
        .fetch_optional(&mut *lease)
        .await?;
        let Some((assignment, status, processed)) = record else {
            unlock_completion(lease, lock).await?;
            return Err(denied());
        };
        let processing = ManagerCompletionProcessing {
            lease,
            assignment,
            lock,
        };
        if payload.result.is_some() || payload.status != format!("{status:?}") {
            processing.release(false).await?;
            return Err(denied());
        }
        if processed {
            processing.release(false).await?;
            Ok(ManagerCompletionReceipt::AlreadyProcessed)
        } else {
            Ok(ManagerCompletionReceipt::Process(processing))
        }
    }

    /// No database transaction stays open across a broker round trip.
    pub async fn claim_completion(
        pool: &PgPool,
        claim: Uuid,
    ) -> Result<Option<ManagerExecutionCompletion>> {
        Ok(sqlx::query_as(
            "WITH next AS (
                 SELECT assignment FROM manager_execution_completion
                 WHERE processed_at IS NULL AND available_at <= clock_timestamp()
                 ORDER BY available_at, assignment LIMIT 1 FOR UPDATE SKIP LOCKED
             )
             UPDATE manager_execution_completion o
             SET claim = $1, available_at = clock_timestamp() + make_interval(secs => $2::integer)
             FROM next WHERE o.assignment = next.assignment
             RETURNING o.assignment, o.message_id, o.execution, o.action, o.action_ref, o.status, o.completed_at",
        )
        .bind(claim)
        .bind(PUBLISH_LEASE_SECONDS)
        .fetch_optional(pool)
        .await?)
    }

    pub async fn confirm_completion(pool: &PgPool, assignment: Id, claim: Uuid) -> Result<bool> {
        Ok(sqlx::query(
            "UPDATE manager_execution_completion SET published_at = clock_timestamp(), claim = NULL
             WHERE assignment = $1 AND claim = $2",
        )
        .bind(assignment)
        .bind(claim)
        .execute(pool)
        .await?
        .rows_affected()
            == 1)
    }
}
