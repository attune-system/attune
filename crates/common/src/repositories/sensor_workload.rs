use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    models::{
        Id, OwnedSensorWorkload, SensorWorkload, SensorWorkloadAssignment, SensorWorkloadFence,
        SensorWorkloadLease,
    },
    Error, Result,
};

const WORKLOAD_SELECT_COLUMNS: &str = "id, sensor, workload_key, created, updated";
const ASSIGNMENT_SELECT_COLUMNS: &str = "workload, worker, worker_instance, generation, \
     lease_expires_at, assigned_at, renewed_at, created, updated";

pub const DEFAULT_SENSOR_WORKLOAD_KEY: &str = "default";
const MAX_LEASE_SECONDS: i64 = 86_400;

#[derive(Debug, Clone, Copy)]
pub struct AcquireSensorWorkloadInput {
    pub sensor_id: Id,
    pub worker_id: Id,
    pub worker_instance: Uuid,
    pub lease_seconds: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct SensorWorkloadLeaseInput {
    pub fence: SensorWorkloadFence,
    pub lease_seconds: i64,
}

#[derive(Debug, Clone)]
pub enum AcquireSensorWorkloadOutcome {
    Acquired(SensorWorkloadLease),
    HeldByOther(SensorWorkloadAssignment),
}

pub struct SensorWorkloadRepository;

impl SensorWorkloadRepository {
    pub async fn ensure_default_for_sensor(pool: &PgPool, sensor_id: Id) -> Result<SensorWorkload> {
        let mut tx = pool.begin().await?;
        let workload = Self::ensure_default_with_conn(&mut tx, sensor_id).await?;
        tx.commit().await?;
        Ok(workload)
    }

    pub async fn find_assignment(
        pool: &PgPool,
        workload_id: Id,
    ) -> Result<Option<SensorWorkloadAssignment>> {
        let assignment = sqlx::query_as::<_, SensorWorkloadAssignment>(&format!(
            "SELECT {ASSIGNMENT_SELECT_COLUMNS} \
             FROM sensor_workload_assignment WHERE workload = $1"
        ))
        .bind(workload_id)
        .fetch_optional(pool)
        .await?;
        Ok(assignment)
    }

    pub async fn acquire_or_renew(
        pool: &PgPool,
        input: AcquireSensorWorkloadInput,
    ) -> Result<AcquireSensorWorkloadOutcome> {
        validate_lease_seconds(input.lease_seconds)?;

        let mut tx = pool.begin().await?;
        let outcome = Self::acquire_or_renew_with_conn(&mut tx, input).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    pub(super) async fn acquire_or_renew_with_conn(
        connection: &mut PgConnection,
        input: AcquireSensorWorkloadInput,
    ) -> Result<AcquireSensorWorkloadOutcome> {
        validate_lease_seconds(input.lease_seconds)?;
        let workload = Self::ensure_default_with_conn(connection, input.sensor_id).await?;
        sqlx::query(
            "INSERT INTO sensor_workload_assignment (workload) VALUES ($1) \
             ON CONFLICT (workload) DO NOTHING",
        )
        .bind(workload.id)
        .execute(&mut *connection)
        .await?;

        let assignment = sqlx::query_as::<_, SensorWorkloadAssignment>(&format!(
            "SELECT {ASSIGNMENT_SELECT_COLUMNS} FROM sensor_workload_assignment \
             WHERE workload = $1 FOR UPDATE"
        ))
        .bind(workload.id)
        .fetch_one(&mut *connection)
        .await?;

        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *connection)
            .await?;
        let same_owner = assignment.worker == Some(input.worker_id)
            && assignment.worker_instance == Some(input.worker_instance);
        let available = assignment.worker.is_none()
            || assignment
                .lease_expires_at
                .is_some_and(|expiry| expiry <= now)
            || same_owner;

        if !available {
            return Ok(AcquireSensorWorkloadOutcome::HeldByOther(assignment));
        }

        let lease = sqlx::query_as::<_, SensorWorkloadLease>(
            "UPDATE sensor_workload_assignment \
             SET worker = $2, worker_instance = $3, \
                 lease_expires_at = $4 + make_interval(secs => $5::double precision), \
                 assigned_at = CASE \
                     WHEN worker = $2 AND worker_instance = $3 THEN assigned_at \
                     ELSE $4 \
                 END, \
                 renewed_at = $4 \
             WHERE workload = $1 \
             RETURNING workload AS workload_id, $6::BIGINT AS sensor_id, \
                       worker AS worker_id, worker_instance, generation, lease_expires_at",
        )
        .bind(workload.id)
        .bind(input.worker_id)
        .bind(input.worker_instance)
        .bind(now)
        .bind(input.lease_seconds)
        .bind(input.sensor_id)
        .fetch_one(&mut *connection)
        .await?;
        Ok(AcquireSensorWorkloadOutcome::Acquired(lease))
    }

    pub async fn begin_process(
        pool: &PgPool,
        lease: SensorWorkloadLease,
    ) -> Result<Option<OwnedSensorWorkload>> {
        let mut connection = pool.acquire().await?;
        Self::begin_process_with_conn(&mut connection, lease).await
    }

    pub(super) async fn begin_process_with_conn(
        connection: &mut PgConnection,
        lease: SensorWorkloadLease,
    ) -> Result<Option<OwnedSensorWorkload>> {
        let owned = sqlx::query_as::<_, OwnedSensorWorkload>(
            "UPDATE sensor_workload_assignment AS assignment \
             SET generation = generation + 1, renewed_at = clock_timestamp() \
             FROM sensor_workload AS workload \
             WHERE assignment.workload = $1 \
               AND assignment.workload = workload.id \
               AND assignment.worker = $2 \
               AND assignment.worker_instance = $3 \
               AND assignment.generation = $4 \
               AND assignment.lease_expires_at > clock_timestamp() \
             RETURNING assignment.workload AS workload_id, workload.sensor AS sensor_id, \
                       assignment.worker AS worker_id, assignment.worker_instance, \
                       assignment.generation, assignment.lease_expires_at",
        )
        .bind(lease.workload_id)
        .bind(lease.worker_id)
        .bind(lease.worker_instance)
        .bind(lease.generation)
        .fetch_optional(connection)
        .await?;
        Ok(owned)
    }

    pub async fn renew(
        pool: &PgPool,
        input: SensorWorkloadLeaseInput,
    ) -> Result<Option<OwnedSensorWorkload>> {
        let mut connection = pool.acquire().await?;
        Self::renew_with_conn(&mut connection, input).await
    }

    pub(super) async fn renew_with_conn(
        connection: &mut PgConnection,
        input: SensorWorkloadLeaseInput,
    ) -> Result<Option<OwnedSensorWorkload>> {
        validate_lease_seconds(input.lease_seconds)?;
        let owned = sqlx::query_as::<_, OwnedSensorWorkload>(
            "UPDATE sensor_workload_assignment AS assignment \
             SET lease_expires_at = clock_timestamp() + make_interval(secs => $5::double precision), \
                 renewed_at = clock_timestamp() \
             FROM sensor_workload AS workload \
             WHERE assignment.workload = $1 \
               AND assignment.workload = workload.id \
               AND assignment.worker = $2 \
               AND assignment.worker_instance = $3 \
               AND assignment.generation = $4 \
               AND assignment.lease_expires_at > clock_timestamp() \
             RETURNING assignment.workload AS workload_id, workload.sensor AS sensor_id, \
                       assignment.worker AS worker_id, assignment.worker_instance, \
                       assignment.generation, assignment.lease_expires_at",
        )
        .bind(input.fence.workload_id)
        .bind(input.fence.worker_id)
        .bind(input.fence.worker_instance)
        .bind(input.fence.generation)
        .bind(input.lease_seconds)
        .fetch_optional(connection)
        .await?;
        Ok(owned)
    }

    pub async fn release(pool: &PgPool, fence: SensorWorkloadFence) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE sensor_workload_assignment \
             SET worker = NULL, worker_instance = NULL, lease_expires_at = NULL, \
                 assigned_at = NULL, renewed_at = NULL \
             WHERE workload = $1 AND worker = $2 AND worker_instance = $3 \
               AND generation = $4",
        )
        .bind(fence.workload_id)
        .bind(fence.worker_id)
        .bind(fence.worker_instance)
        .bind(fence.generation)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn is_current_fence(pool: &PgPool, fence: SensorWorkloadFence) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM sensor_workload_assignment \
                 WHERE workload = $1 AND worker = $2 AND worker_instance = $3 \
                   AND generation = $4 AND lease_expires_at > clock_timestamp() \
             )",
        )
        .bind(fence.workload_id)
        .bind(fence.worker_id)
        .bind(fence.worker_instance)
        .bind(fence.generation)
        .fetch_one(pool)
        .await?)
    }

    pub async fn lock_current_fence(
        tx: &mut Transaction<'_, Postgres>,
        sensor_id: Id,
        fence: SensorWorkloadFence,
    ) -> Result<bool> {
        let current = sqlx::query_scalar::<_, bool>(
            "SELECT TRUE \
             FROM sensor_workload_assignment AS assignment \
             JOIN sensor_workload AS workload ON workload.id = assignment.workload \
             WHERE assignment.workload = $1 \
               AND workload.sensor = $2 \
               AND assignment.worker = $3 \
               AND assignment.worker_instance = $4 \
               AND assignment.generation = $5 \
               AND assignment.lease_expires_at > clock_timestamp() \
             FOR NO KEY UPDATE OF assignment",
        )
        .bind(fence.workload_id)
        .bind(sensor_id)
        .bind(fence.worker_id)
        .bind(fence.worker_instance)
        .bind(fence.generation)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(false);
        Ok(current)
    }

    pub async fn rule_is_member(
        tx: &mut Transaction<'_, Postgres>,
        sensor_id: Id,
        workload_id: Id,
        rule_id: Id,
        trigger_id: Id,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 \
                 FROM sensor_workload sw \
                 JOIN rule r ON r.id = $3 AND r.enabled = TRUE \
                 JOIN trigger t ON t.id = r.trigger AND t.id = $4 AND t.enabled = TRUE \
                 JOIN sensor s ON s.id = sw.sensor AND s.enabled = TRUE \
                 WHERE sw.id = $2 AND sw.sensor = $1 AND t.sensor = sw.sensor \
             )",
        )
        .bind(sensor_id)
        .bind(workload_id)
        .bind(rule_id)
        .bind(trigger_id)
        .fetch_one(&mut **tx)
        .await?)
    }

    pub async fn trigger_is_member(
        tx: &mut Transaction<'_, Postgres>,
        sensor_id: Id,
        workload_id: Id,
        trigger_id: Id,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 \
                 FROM sensor_workload sw \
                 JOIN trigger t ON t.id = $3 AND t.enabled = TRUE \
                 JOIN sensor s ON s.id = sw.sensor AND s.enabled = TRUE \
                 WHERE sw.id = $2 AND sw.sensor = $1 AND t.sensor = sw.sensor \
             )",
        )
        .bind(sensor_id)
        .bind(workload_id)
        .bind(trigger_id)
        .fetch_one(&mut **tx)
        .await?)
    }

    async fn ensure_default_with_conn(
        conn: &mut PgConnection,
        sensor_id: Id,
    ) -> Result<SensorWorkload> {
        sqlx::query(
            "INSERT INTO sensor_workload (sensor, workload_key) VALUES ($1, $2) \
             ON CONFLICT (sensor, workload_key) DO NOTHING",
        )
        .bind(sensor_id)
        .bind(DEFAULT_SENSOR_WORKLOAD_KEY)
        .execute(&mut *conn)
        .await?;

        let workload = sqlx::query_as::<_, SensorWorkload>(&format!(
            "SELECT {WORKLOAD_SELECT_COLUMNS} FROM sensor_workload \
             WHERE sensor = $1 AND workload_key = $2"
        ))
        .bind(sensor_id)
        .bind(DEFAULT_SENSOR_WORKLOAD_KEY)
        .fetch_one(conn)
        .await?;
        Ok(workload)
    }
}

fn validate_lease_seconds(lease_seconds: i64) -> Result<()> {
    if !(1..=MAX_LEASE_SECONDS).contains(&lease_seconds) {
        return Err(Error::validation(format!(
            "lease_seconds must be between 1 and {MAX_LEASE_SECONDS}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_lease_seconds, MAX_LEASE_SECONDS};

    #[test]
    fn lease_duration_must_be_positive() {
        assert!(validate_lease_seconds(1).is_ok());
        assert!(validate_lease_seconds(0).is_err());
        assert!(validate_lease_seconds(-1).is_err());
        assert!(validate_lease_seconds(MAX_LEASE_SECONDS + 1).is_err());
    }
}
