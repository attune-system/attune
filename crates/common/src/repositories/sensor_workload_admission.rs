use sqlx::{PgConnection, PgPool};

use crate::{
    models::{Id, OwnedSensorWorkload, SensorWorkloadAssignment},
    repositories::{
        sensor_admission::SensorAdmissionRepository,
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadLeaseInput,
            SensorWorkloadRepository,
        },
    },
    Result,
};

#[derive(Debug, Clone)]
pub enum AcquireEligibleSensorWorkloadOutcome {
    Acquired(OwnedSensorWorkload),
    Ineligible,
    HeldByOther(SensorWorkloadAssignment),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewEligibleSensorWorkloadOutcome {
    Renewed(OwnedSensorWorkload),
    Ineligible,
    OwnershipLost,
}

pub struct SensorWorkloadAdmissionRepository;

impl SensorWorkloadAdmissionRepository {
    pub async fn acquire(
        pool: &PgPool,
        input: AcquireSensorWorkloadInput,
    ) -> Result<AcquireEligibleSensorWorkloadOutcome> {
        let mut tx = pool.begin().await?;
        SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
        if !Self::lock_worker_capacity(&mut tx, input.worker_id, input.sensor_id).await? {
            tx.commit().await?;
            return Ok(AcquireEligibleSensorWorkloadOutcome::Ineligible);
        }
        if !SensorAdmissionRepository::worker_is_eligible_for_active_workload(
            &mut tx,
            input.sensor_id,
            input.worker_id,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(AcquireEligibleSensorWorkloadOutcome::Ineligible);
        }

        let outcome = SensorWorkloadRepository::acquire_or_renew_with_conn(&mut tx, input).await?;
        let outcome = match outcome {
            AcquireSensorWorkloadOutcome::Acquired(lease) => {
                match SensorWorkloadRepository::begin_process_with_conn(&mut tx, lease).await? {
                    Some(workload) => AcquireEligibleSensorWorkloadOutcome::Acquired(workload),
                    None => {
                        tx.rollback().await?;
                        return Ok(AcquireEligibleSensorWorkloadOutcome::Ineligible);
                    }
                }
            }
            AcquireSensorWorkloadOutcome::HeldByOther(assignment) => {
                AcquireEligibleSensorWorkloadOutcome::HeldByOther(assignment)
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }

    async fn lock_worker_capacity(
        connection: &mut PgConnection,
        worker_id: Id,
        sensor_id: Id,
    ) -> Result<bool> {
        let capabilities = sqlx::query_scalar::<_, Option<serde_json::Value>>(
            "SELECT capabilities FROM worker WHERE id = $1 FOR UPDATE",
        )
        .bind(worker_id)
        .fetch_optional(&mut *connection)
        .await?
        .flatten();
        let Some(capabilities) = capabilities else {
            return Ok(false);
        };
        let max_concurrent = capabilities
            .get("max_concurrent_sensors")
            .and_then(|value| value.as_u64())
            .unwrap_or(10);
        let active_assignments = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sensor_workload_assignment AS assignment \
             JOIN sensor_workload AS workload ON workload.id = assignment.workload \
             WHERE assignment.worker = $1 \
               AND assignment.lease_expires_at > clock_timestamp() \
               AND workload.sensor <> $2",
        )
        .bind(worker_id)
        .bind(sensor_id)
        .fetch_one(connection)
        .await?;

        Ok(u64::try_from(active_assignments).unwrap_or(u64::MAX) < max_concurrent)
    }

    pub async fn renew(
        pool: &PgPool,
        sensor_id: Id,
        input: SensorWorkloadLeaseInput,
    ) -> Result<RenewEligibleSensorWorkloadOutcome> {
        let mut tx = pool.begin().await?;
        SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
        if !SensorAdmissionRepository::worker_is_eligible_for_active_workload(
            &mut tx,
            sensor_id,
            input.fence.worker_id,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(RenewEligibleSensorWorkloadOutcome::Ineligible);
        }

        let outcome = SensorWorkloadRepository::renew_with_conn(&mut tx, input)
            .await?
            .map(RenewEligibleSensorWorkloadOutcome::Renewed)
            .unwrap_or(RenewEligibleSensorWorkloadOutcome::OwnershipLost);
        tx.commit().await?;
        Ok(outcome)
    }
}
