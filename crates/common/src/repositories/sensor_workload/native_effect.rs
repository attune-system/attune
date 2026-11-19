//! Sensor-owned native effect evidence. Live source locks precede epoch locks
//! and the retained operation. Cleanup can close Run after source revocation.
mod files;
use super::*;
use crate::{crypto, isolated_process::ResourceIdentity, workload_launch::LaunchExitOutcome};
pub use files::SensorFileAllocation;
use serde::{Deserialize, Serialize};
use sqlx::types::Json;

fn denied() -> Error {
    Error::PermissionDenied("Sensor native effect unavailable".into())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorNativeEffect {
    pub intent: Uuid,
    pub generation: i64,
    pub files_bound: bool,
    pub publication_deadline: Option<DateTime<Utc>>,
    pub worker: Id,
    pub worker_instance: Uuid,
    pub encrypted_context: String,
    epochs: Vec<super::super::workload_source::SourceEpoch>,
    pub create_submitted: bool,
    pub stdin_consumed: bool,
    pub stop_requested: bool,
    pub resource: Option<ResourceIdentity>,
    pub exit: Option<LaunchExitOutcome>,
    pub stdout_complete: bool,
    pub stderr_complete: bool,
}
impl SensorNativeEffect {
    pub fn capture_complete(&self) -> bool {
        self.stdout_complete && self.stderr_complete
    }
}
impl SensorWorkloadRepository {
    pub async fn native_effect(pool: &PgPool, workload: Id) -> Result<Option<SensorNativeEffect>> {
        Ok(sqlx::query_scalar::<_, Option<Json<SensorNativeEffect>>>(
            "SELECT native_effect FROM sensor_workload_assignment WHERE workload=$1",
        )
        .bind(workload)
        .fetch_optional(pool)
        .await?
        .flatten()
        .map(|v| v.0))
    }
    /// Only this incarnation's lost startup or an expired assignment can be
    /// reconciled by a new manager. A live sibling assignment is never stopped.
    pub async fn unsettled_native_for_sensor(
        pool: &PgPool,
        sensor: Id,
        instance: Uuid,
    ) -> Result<Option<(Id, SensorNativeEffect)>> {
        let row: Option<(Id, Json<SensorNativeEffect>)> = sqlx::query_as(
            "SELECT a.workload,a.native_effect FROM sensor_workload_assignment a JOIN sensor_workload w ON w.id=a.workload WHERE w.sensor=$1 AND a.native_effect IS NOT NULL AND (a.worker_instance=$2 OR a.lease_expires_at IS NULL OR a.lease_expires_at<=clock_timestamp())")
            .bind(sensor).bind(instance).fetch_optional(pool).await?;
        Ok(row.map(|(id, effect)| (id, effect.0)))
    }
    async fn native_epochs(
        tx: &mut Transaction<'_, Postgres>,
        sensor: Id,
        fence: SensorWorkloadFence,
    ) -> Result<Vec<super::super::workload_source::SourceEpoch>> {
        if !Self::lock_current_fence(tx, sensor, fence).await? {
            return Err(denied());
        }
        let metadata =
            super::super::managed_sensor_metadata::ManagedSensorMetadataRepository::for_generation(
                tx,
                fence.workload_id,
                fence.generation,
            )
            .await?
            .ok_or_else(denied)?;
        let worker: Option<i64> = sqlx::query_scalar("SELECT id FROM worker WHERE id=$1 AND status IN ('active','busy') AND last_heartbeat>clock_timestamp()-interval '90 seconds' FOR SHARE")
            .bind(fence.worker_id).fetch_optional(&mut **tx).await?;
        if worker != Some(fence.worker_id) {
            return Err(denied());
        }
        super::super::workload_source::snapshot(
            tx,
            vec![
                ("owner", metadata.owner.sponsor_identity_id),
                ("sensor", sensor),
                ("workload", fence.workload_id),
                ("worker", fence.worker_id),
            ],
        )
        .await
    }
    pub async fn reserve_native_effect(
        pool: &PgPool,
        sensor: Id,
        fence: SensorWorkloadFence,
        intent: Uuid,
        context: &str,
        master: &str,
    ) -> Result<()> {
        if intent.is_nil() {
            return Err(denied());
        }
        let encrypted_context = crypto::encrypt(context, master)?;
        let mut tx = pool.begin().await?;
        let epochs = Self::native_epochs(&mut tx, sensor, fence).await?;
        let effect = SensorNativeEffect {
            intent,
            generation: fence.generation,
            files_bound: true,
            publication_deadline: None,
            worker: fence.worker_id,
            worker_instance: fence.worker_instance,
            encrypted_context,
            epochs,
            create_submitted: false,
            stdin_consumed: false,
            stop_requested: false,
            resource: None,
            exit: None,
            stdout_complete: false,
            stderr_complete: false,
        };
        let changed = sqlx::query("UPDATE sensor_workload_assignment SET native_effect=$2 WHERE workload=$1 AND native_effect IS NULL")
            .bind(fence.workload_id).bind(Json(effect)).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        Self::recheck_locked_fence(&mut tx, sensor, fence).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Consume the durable decision before any provider create or stdin call.
    /// A lost commit ACK never permits replay. Source renewal is not admission.
    pub async fn consume_native_effect(
        pool: &PgPool,
        sensor: Id,
        fence: SensorWorkloadFence,
        intent: Uuid,
        stdin: bool,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let epochs = Self::native_epochs(&mut tx, sensor, fence).await?;
        let mut effect = Self::locked_native(&mut tx, fence.workload_id, intent).await?;
        if effect.generation != fence.generation
            || effect.epochs != epochs
            || effect.stop_requested
            || effect.exit.is_some()
            || if stdin {
                effect.stdin_consumed || !effect.create_submitted || effect.resource.is_none()
            } else {
                effect.create_submitted
            }
        {
            return Err(denied());
        }
        if stdin {
            effect.stdin_consumed = true;
        } else {
            effect.create_submitted = true;
        }
        Self::write_native(&mut tx, fence.workload_id, &effect).await?;
        Self::recheck_locked_fence(&mut tx, sensor, fence).await?;
        tx.commit().await?;
        Ok(())
    }
    async fn locked_native(
        tx: &mut Transaction<'_, Postgres>,
        workload: Id,
        intent: Uuid,
    ) -> Result<SensorNativeEffect> {
        let effect: Option<Json<SensorNativeEffect>> = sqlx::query_scalar(
            "SELECT native_effect FROM sensor_workload_assignment WHERE workload=$1 FOR UPDATE",
        )
        .bind(workload)
        .fetch_one(&mut **tx)
        .await?;
        let effect = effect.ok_or_else(denied)?.0;
        if effect.intent != intent {
            return Err(denied());
        }
        Ok(effect)
    }
    async fn write_native(
        tx: &mut Transaction<'_, Postgres>,
        workload: Id,
        effect: &SensorNativeEffect,
    ) -> Result<()> {
        sqlx::query("UPDATE sensor_workload_assignment SET native_effect=$2 WHERE workload=$1")
            .bind(workload)
            .bind(Json(effect))
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    /// Retained facts remain writable after revocation, without restoring Run.
    pub async fn record_native_resource(
        pool: &PgPool,
        workload: Id,
        intent: Uuid,
        resource: &ResourceIdentity,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let mut effect = Self::locked_native(&mut tx, workload, intent).await?;
        if !effect.create_submitted || effect.resource.as_ref().is_some_and(|old| old != resource) {
            return Err(denied());
        }
        effect.resource = Some(resource.clone());
        Self::write_native(&mut tx, workload, &effect).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn stop_native_effect(pool: &PgPool, workload: Id, intent: Uuid) -> Result<()> {
        let mut tx = pool.begin().await?;
        let mut effect = Self::locked_native(&mut tx, workload, intent).await?;
        // Retain the existing lease deadline; stop does not mint or renew
        // publication authority. An already-expired/missing lease stays denied.
        if !effect.stop_requested && effect.publication_deadline.is_none() {
            effect.publication_deadline = sqlx::query_scalar(
                "SELECT lease_expires_at FROM sensor_workload_assignment WHERE workload=$1",
            )
            .bind(workload)
            .fetch_one(&mut *tx)
            .await?;
        }
        // Close the existing API/generation fence BEFORE provider settlement.
        // Historical context/resource remain available for cleanup only.
        sqlx::query("UPDATE sensor_workload_assignment SET lease_expires_at=clock_timestamp() WHERE workload=$1")
            .bind(workload).execute(&mut *tx).await?;
        effect.stop_requested = true;
        Self::write_native(&mut tx, workload, &effect).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn record_native_exit(
        pool: &PgPool,
        workload: Id,
        intent: Uuid,
        resource: &ResourceIdentity,
        exit: LaunchExitOutcome,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let mut effect = Self::locked_native(&mut tx, workload, intent).await?;
        if effect.resource.as_ref() != Some(resource)
            || effect.exit.as_ref().is_some_and(|old| old != &exit)
        {
            return Err(denied());
        }
        effect.exit = Some(exit);
        // Retain the existing lease deadline; stop does not mint or renew
        // publication authority. An already-expired/missing lease stays denied.
        if !effect.stop_requested && effect.publication_deadline.is_none() {
            effect.publication_deadline = sqlx::query_scalar(
                "SELECT lease_expires_at FROM sensor_workload_assignment WHERE workload=$1",
            )
            .bind(workload)
            .fetch_one(&mut *tx)
            .await?;
        }
        // Close the existing API/generation fence BEFORE provider settlement.
        // Historical context/resource remain available for cleanup only.
        sqlx::query("UPDATE sensor_workload_assignment SET lease_expires_at=clock_timestamp() WHERE workload=$1")
            .bind(workload).execute(&mut *tx).await?;
        effect.stop_requested = true;
        Self::write_native(&mut tx, workload, &effect).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn record_native_capture(
        pool: &PgPool,
        workload: Id,
        intent: Uuid,
        stderr: bool,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let mut effect = Self::locked_native(&mut tx, workload, intent).await?;
        if stderr {
            effect.stderr_complete = true;
        } else {
            effect.stdout_complete = true;
        }
        Self::write_native(&mut tx, workload, &effect).await?;
        tx.commit().await?;
        Ok(())
    }
}
