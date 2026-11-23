//! API-owned sensor starts share the existing assignment and admission tables.
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    models::{Id, SensorProcess, SensorProcessStatus, SensorWorkloadFence, WorkerRole},
    Error, Result,
};

use super::{
    manager_runtime::ManagerRuntimeRepository,
    manager_session::ManagerSessionRecord,
    sensor_admission::SensorAdmissionRepository,
    sensor_process::{
        MarkSensorProcessFailedInput, MarkSensorProcessStoppedInput, SensorProcessRepository,
        UpsertSensorProcessStartInput,
    },
    sensor_schema::SensorSchemaRepository,
    sensor_workload::{
        AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadLeaseInput,
        SensorWorkloadRepository,
    },
    FindById,
};

const LEASE_SECONDS: i64 = 30;

pub struct ManagerSensorRepository;

fn denied() -> Error {
    Error::PermissionDenied("Sensor generation is outside the manager's live workload scope".into())
}

impl ManagerSensorRepository {
    pub async fn start(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        sensor: Id,
        request: Uuid,
        encryption_key: Option<&str>,
    ) -> Result<SensorWorkloadFence> {
        if manager.role != WorkerRole::Sensor || request.is_nil() {
            return Err(denied());
        }
        let worker = ManagerRuntimeRepository::lock_worker(tx, manager).await?;
        if !SensorAdmissionRepository::worker_is_eligible_for_active_workload(tx, sensor, worker)
            .await?
        {
            return Err(denied());
        }
        let workload = SensorWorkloadRepository::ensure_default_with_conn(tx, sensor).await?;
        let assignment = SensorWorkloadRepository::lock_assignment(tx, workload.id).await?;
        let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **tx)
            .await?;
        let live_owned = assignment.worker == Some(worker)
            && assignment.worker_instance == Some(manager.instance)
            && assignment
                .lease_expires_at
                .is_some_and(|deadline| deadline > now);
        let prior: Option<(Id,i64)> = sqlx::query_as(
            "SELECT workload,generation FROM manager_sensor_generation WHERE manager_session=$1 AND request=$2",
        ).bind(manager.id).bind(request).fetch_optional(&mut **tx).await?;
        let generation = if let Some(prior) = prior {
            if !live_owned || prior != (workload.id, assignment.generation) {
                return Err(denied());
            }
            prior.1
        } else {
            let already_started: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM manager_sensor_generation WHERE workload=$1 AND generation=$2 AND manager_session=$3)",
            ).bind(workload.id).bind(assignment.generation).bind(manager.id).fetch_one(&mut **tx).await?;
            if live_owned && already_started {
                return Err(denied());
            }
            if let (Some(holder), Some(instance)) = (assignment.worker, assignment.worker_instance)
            {
                let revoked: bool = sqlx::query_scalar(
                    "SELECT s.closed_at IS NOT NULL OR s.expires_at<=clock_timestamp()
                     FROM manager_sensor_generation g JOIN manager_session s ON s.id=g.manager_session
                     WHERE g.workload=$1 AND g.generation=$2 AND g.instance=$3 FOR SHARE OF s",
                ).bind(workload.id).bind(assignment.generation).bind(instance)
                    .fetch_optional(&mut **tx).await?.unwrap_or(false);
                if revoked {
                    SensorWorkloadRepository::release_with_conn(
                        &mut **tx,
                        SensorWorkloadFence {
                            workload_id: workload.id,
                            generation: assignment.generation,
                            worker_id: holder,
                            worker_instance: instance,
                        },
                    )
                    .await?;
                }
            }
            let desired = SensorWorkloadRepository::refresh_default_with_conn(tx, sensor).await?;
            let acquired = SensorWorkloadRepository::acquire_or_renew_with_conn(
                tx,
                AcquireSensorWorkloadInput {
                    sensor_id: sensor,
                    worker_id: worker,
                    worker_instance: manager.instance,
                    lease_seconds: LEASE_SECONDS,
                },
            )
            .await?;
            let AcquireSensorWorkloadOutcome::Acquired(lease) = acquired else {
                return Err(denied());
            };
            let owned = SensorWorkloadRepository::begin_process_with_conn(tx, lease)
                .await?
                .ok_or_else(denied)?;
            SensorSchemaRepository::admit(tx, &owned, encryption_key).await?;
            sqlx::query(
                "INSERT INTO manager_sensor_generation
                   (workload,generation,request,manager_session,instance,worker,definition_updated,release)
                 SELECT $1,$2,$3,$4,$5,$6,updated,$7 FROM sensor WHERE id=$8",
            ).bind(workload.id).bind(owned.generation).bind(request).bind(manager.id)
                .bind(manager.instance).bind(worker).bind(desired.pack_release).bind(sensor)
                .execute(&mut **tx).await?;
            owned.generation
        };
        let fence = SensorWorkloadFence {
            workload_id: workload.id,
            generation,
            worker_id: worker,
            worker_instance: manager.instance,
        };
        if !SensorWorkloadRepository::lock_current_fence(tx, sensor, fence).await? {
            return Err(denied());
        }
        Ok(fence)
    }

    pub async fn renew(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        workload: Id,
        generation: i64,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let fence = Self::owned_fence(tx, manager, workload, generation).await?;
        let workload = SensorWorkloadRepository::find_by_id(&mut **tx, workload)
            .await?
            .ok_or_else(denied)?;
        if !SensorAdmissionRepository::worker_is_eligible_for_active_workload(
            tx,
            workload.sensor,
            fence.worker_id,
        )
        .await?
        {
            return Ok(None);
        }
        let same_definition: bool = sqlx::query_scalar(
            "SELECT g.definition_updated IS NOT DISTINCT FROM s.updated
                AND g.release IS NOT DISTINCT FROM s.managed_release
             FROM sensor s JOIN manager_sensor_generation g ON g.workload=$1 AND g.generation=$2
             WHERE s.id=$3",
        )
        .bind(fence.workload_id)
        .bind(fence.generation)
        .bind(workload.sensor)
        .fetch_one(&mut **tx)
        .await?;
        if !same_definition {
            return Ok(None);
        }
        Ok(SensorWorkloadRepository::renew_with_conn(
            tx,
            SensorWorkloadLeaseInput {
                fence,
                lease_seconds: LEASE_SECONDS,
            },
        )
        .await?
        .map(|row| row.lease_expires_at))
    }

    pub async fn release(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        workload: Id,
        generation: i64,
    ) -> Result<()> {
        let fence = Self::owned_fence(tx, manager, workload, generation).await?;
        SensorWorkloadRepository::release_with_conn(&mut **tx, fence).await?;
        Ok(())
    }

    pub async fn owned_fence(
        conn: &mut PgConnection,
        manager: &ManagerSessionRecord,
        workload: Id,
        generation: i64,
    ) -> Result<SensorWorkloadFence> {
        if manager.role != WorkerRole::Sensor {
            return Err(denied());
        }
        let worker = ManagerRuntimeRepository::lock_worker(conn, manager).await?;
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM manager_sensor_generation WHERE workload=$1 AND generation=$2
                     AND manager_session=$3 AND instance=$4 AND worker=$5)",
        ).bind(workload).bind(generation).bind(manager.id).bind(manager.instance).bind(worker)
            .fetch_one(conn).await?;
        if !bound {
            return Err(denied());
        }
        Ok(SensorWorkloadFence {
            workload_id: workload,
            generation,
            worker_id: worker,
            worker_instance: manager.instance,
        })
    }

    pub async fn report_process(
        tx: &mut Transaction<'_, Postgres>,
        manager: &ManagerSessionRecord,
        workload: Id,
        generation: i64,
        state: crate::manager_control::SensorProcessReport,
    ) -> Result<Option<SensorProcess>> {
        use crate::manager_control::SensorProcessReport;
        let fence = Self::owned_fence(tx, manager, workload, generation).await?;
        let sensor_id = SensorWorkloadRepository::find_by_id(&mut **tx, workload)
            .await?
            .ok_or_else(denied)?
            .sensor;
        // A terminal replay acknowledges an immutable receipt without touching
        // the newer generation's process row or incrementing failures again.
        let prior: Option<String> = sqlx::query_scalar(
            "SELECT finished_as FROM manager_sensor_generation WHERE workload=$1 AND generation=$2",
        )
        .bind(workload)
        .bind(generation)
        .fetch_one(&mut **tx)
        .await?;
        if let Some(prior) = prior {
            return if Some(prior.as_str()) == state.outcome() {
                Ok(None)
            } else {
                Err(denied())
            };
        }
        if !SensorWorkloadRepository::lock_current_fence(tx, sensor_id, fence).await? {
            return Err(denied());
        }
        let deadline = SensorWorkloadRepository::retained_deadline(tx, fence).await?;
        let (started, finished): (Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT started_pid, finished_as FROM manager_sensor_generation
             WHERE workload=$1 AND generation=$2 FOR UPDATE",
        )
        .bind(workload)
        .bind(generation)
        .fetch_one(&mut **tx)
        .await?;
        if let Some(finished) = finished {
            return if Some(finished.as_str()) == state.outcome() {
                Ok(None)
            } else {
                Err(denied())
            };
        }
        let sensor = super::SensorRepository::find_by_id(&mut **tx, sensor_id)
            .await?
            .ok_or_else(denied)?;
        let eligibility = SensorAdmissionRepository::worker_eligibility_by_sensor(
            tx,
            std::slice::from_ref(&sensor),
            fence.worker_id,
        )
        .await?;
        let active_rule_count = eligibility
            .get(&sensor.id)
            .and_then(|entry| i32::try_from(entry.active_rule_count).ok())
            .unwrap_or(0);
        if let SensorProcessReport::Running { pid } = state {
            if started == Some(pid) {
                return Ok(None);
            }
            if started.is_some() || pid <= 0 {
                return Err(denied());
            }
        }
        let worker = super::WorkerRepository::find_by_id(&mut **tx, fence.worker_id)
            .await?
            .ok_or_else(denied)?;
        if started.is_none() {
            let pid = match state {
                SensorProcessReport::Running { pid } => Some(pid),
                _ => None,
            };
            SensorProcessRepository::upsert_starting_or_running(
                &mut **tx,
                UpsertSensorProcessStartInput {
                    sensor: sensor.id,
                    sensor_ref: sensor.r#ref.clone(),
                    worker: worker.id,
                    worker_name: worker.name,
                    status: if pid.is_some() {
                        SensorProcessStatus::Running
                    } else {
                        SensorProcessStatus::Starting
                    },
                    pid,
                    started_at: None,
                    active_rule_count,
                    log_artifact_ref: Some(format!("sensor.{}.stderr", sensor.r#ref)),
                    meta: Some(
                        serde_json::json!({"manager": "attune-sensor", "generation": generation}),
                    ),
                    reset_failure_count: false,
                },
            )
            .await?;
            sqlx::query("UPDATE manager_sensor_generation SET started_pid=$3 WHERE workload=$1 AND generation=$2")
                .bind(workload).bind(generation).bind(pid).execute(&mut **tx).await?;
        }
        let Some(outcome) = state.outcome() else {
            super::manager_assignment::ManagerAssignmentRepository::check_deadline(tx, deadline)
                .await?;
            return Ok(None);
        };
        let failed = if let SensorProcessReport::Failed { exit_code } = state {
            SensorProcessRepository::mark_failed_or_backoff(
                &mut **tx,
                MarkSensorProcessFailedInput {
                    sensor: sensor.id,
                    worker: fence.worker_id,
                    status: SensorProcessStatus::Backoff,
                    exit_code,
                    signal: None,
                    stopped_at: None,
                    stderr_excerpt: None,
                    active_rule_count,
                    next_restart_at: None,
                },
            )
            .await?
        } else {
            SensorProcessRepository::mark_stopped(
                &mut **tx,
                MarkSensorProcessStoppedInput {
                    sensor: sensor.id,
                    worker: fence.worker_id,
                    stopped_at: None,
                    active_rule_count: Some(active_rule_count),
                },
            )
            .await?;
            None
        };
        sqlx::query("UPDATE manager_sensor_generation SET finished_as=$3 WHERE workload=$1 AND generation=$2")
            .bind(workload).bind(generation).bind(outcome).execute(&mut **tx).await?;
        SensorWorkloadRepository::release_with_conn(&mut **tx, fence).await?;
        super::manager_assignment::ManagerAssignmentRepository::check_deadline(tx, deadline)
            .await?;
        Ok(failed)
    }

    pub async fn existing_log(
        conn: &mut PgConnection,
        fence: SensorWorkloadFence,
        request: Uuid,
        channel: &str,
    ) -> Result<Option<Id>> {
        Ok(sqlx::query_scalar(
            "SELECT version FROM manager_sensor_log WHERE workload=$1 AND generation=$2 AND request=$3 AND channel=$4",
        ).bind(fence.workload_id).bind(fence.generation).bind(request).bind(channel)
            .fetch_optional(conn).await?)
    }

    pub async fn bind_log(
        conn: &mut PgConnection,
        fence: SensorWorkloadFence,
        request: Uuid,
        channel: &str,
        version: Id,
    ) -> Result<()> {
        if request.is_nil() {
            return Err(denied());
        }
        sqlx::query(
            "INSERT INTO manager_sensor_log(workload,generation,request,channel,version) VALUES($1,$2,$3,$4,$5)",
        ).bind(fence.workload_id).bind(fence.generation).bind(request).bind(channel).bind(version)
            .execute(conn).await?;
        Ok(())
    }

    /// Called by current-generation consumers while holding the existing
    /// workload fence. Keep retired session bindings for stale-token denial.
    pub async fn retain_owner(
        conn: &mut PgConnection,
        workload: Id,
        generation: i64,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let session: Option<(chrono::DateTime<chrono::Utc>, bool)> = sqlx::query_as(
            "SELECT s.expires_at,s.closed_at IS NULL AND s.role='sensor' AND s.instance=g.instance
                AND EXISTS(SELECT 1 FROM manager_runtime_report r JOIN worker w ON w.id=r.worker
                     WHERE r.worker=g.worker AND r.manager_session=s.id
                       AND w.meta->>'registration_incarnation'=g.instance::text)
             FROM manager_sensor_generation g JOIN manager_session s ON s.id=g.manager_session
              WHERE g.workload=$1 AND g.generation=$2 FOR SHARE OF s",
        )
        .bind(workload)
        .bind(generation)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((deadline, live)) = session {
            if !live {
                return Err(denied());
            }
            super::manager_assignment::ManagerAssignmentRepository::check_deadline(conn, deadline)
                .await?;
            return Ok(Some(deadline));
        }
        Ok(None)
    }
}
