//! Allocation is admitted before target locks, and stop closes that same gate
//! before the exact-generation inventory is read. Runtime logs are separate.
use super::*;
use crate::models::artifact_version::ArtifactVersion;
use crate::repositories::artifact::ArtifactVersionRepository;

#[derive(Clone, Copy)]
pub struct SensorFileAllocation {
    sensor: Id,
    fence: SensorWorkloadFence,
    intent: Uuid,
}
impl SensorWorkloadRepository {
    pub async fn lock_native_file_allocation(
        tx: &mut Transaction<'_, Postgres>,
        sensor: Id,
        fence: SensorWorkloadFence,
    ) -> Result<Option<SensorFileAllocation>> {
        if !Self::lock_current_fence(tx, sensor, fence).await? {
            return Err(denied());
        }
        let effect = Self::native_effect_in(tx, fence.workload_id).await?;
        let Some(effect) = effect else {
            return Ok(None);
        };
        let epochs = Self::native_epochs(tx, sensor, fence).await?;
        let effect = Self::locked_native(tx, fence.workload_id, effect.intent).await?;
        if !effect.files_bound
            || effect.epochs != epochs
            || effect.generation != fence.generation
            || effect.stop_requested
            || effect.exit.is_some()
            || !effect.stdin_consumed
        {
            return Err(denied());
        }
        Self::recheck_locked_fence(tx, sensor, fence).await?;
        Ok(Some(SensorFileAllocation {
            sensor,
            fence,
            intent: effect.intent,
        }))
    }
    async fn native_effect_in(
        tx: &mut Transaction<'_, Postgres>,
        workload: Id,
    ) -> Result<Option<SensorNativeEffect>> {
        Ok(sqlx::query_scalar::<_, Option<Json<SensorNativeEffect>>>(
            "SELECT native_effect FROM sensor_workload_assignment WHERE workload=$1",
        )
        .bind(workload)
        .fetch_one(&mut **tx)
        .await?
        .map(|v| v.0))
    }
    pub async fn bind_native_file(
        tx: &mut Transaction<'_, Postgres>,
        allocation: SensorFileAllocation,
        version: Id,
    ) -> Result<()> {
        let changed=sqlx::query("UPDATE artifact_version v SET sensor_workload=$2,sensor_generation=$3,sensor_native_intent=$4,sensor_worker=$5,sensor_worker_instance=$6 FROM artifact a,sensor s WHERE v.id=$1 AND a.id=v.artifact AND s.id=$7 AND a.scope='sensor' AND a.owner=s.ref AND a.classification<>'runtime_log' AND v.file_path IS NOT NULL AND v.execution IS NULL AND v.sensor_native_intent IS NULL")
            .bind(version).bind(allocation.fence.workload_id).bind(allocation.fence.generation).bind(allocation.intent)
            .bind(allocation.fence.worker_id).bind(allocation.fence.worker_instance).bind(allocation.sensor)
            .execute(&mut **tx).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        Self::recheck_locked_fence(tx, allocation.sensor, allocation.fence).await?;
        Ok(())
    }
    /// Bound ordinary files only. Source locks and epochs precede the operation;
    /// the caller subsequently locks artifact/version and rechecks the deadline.
    pub async fn authorize_native_file(
        tx: &mut Transaction<'_, Postgres>,
        version: Id,
        worker: Id,
        instance: Uuid,
        sensor_fence: Option<SensorWorkloadFence>,
    ) -> Result<Option<super::super::super::runtime_authority::ApprovalEffectFence>> {
        let binding:Option<(Id,i64,Uuid,Id,Uuid,Id)> = sqlx::query_as("SELECT v.sensor_workload,v.sensor_generation,v.sensor_native_intent,v.sensor_worker,v.sensor_worker_instance,w.sensor FROM artifact_version v JOIN sensor_workload w ON w.id=v.sensor_workload WHERE v.id=$1 AND v.sensor_native_intent IS NOT NULL")
            .bind(version).fetch_optional(&mut **tx).await?;
        let Some((workload, generation, intent, owner, incarnation, sensor)) = binding else {
            let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id=$1 AND sensor_native_intent IS NOT NULL)")
                .bind(version).fetch_one(&mut **tx).await?;
            if bound {
                return Err(denied());
            }
            return Ok(None);
        };
        if owner != worker || incarnation != instance {
            return Err(denied());
        }
        let fence = SensorWorkloadFence {
            workload_id: workload,
            worker_id: owner,
            worker_instance: incarnation,
            generation,
        };
        super::super::super::schema_type::SchemaTypeRepository::lock_selection(tx).await?;
        let ready: bool =
            sqlx::query_scalar("SELECT body_state='ready' FROM artifact_version WHERE id=$1")
                .bind(version)
                .fetch_one(&mut **tx)
                .await?;
        let mut source_deadline = None;
        if sensor_fence.is_some() && !Self::lock_current_fence(tx, sensor, fence).await? {
            return Err(denied());
        }
        if !ready {
            super::super::super::managed_sensor_metadata::ManagedSensorMetadataRepository::validate_generation(tx,workload,generation).await?;
        }
        // Worker source protection precedes epochs and operation locks.
        let deadline: DateTime<Utc> = sqlx::query_scalar("SELECT last_heartbeat+interval '90 seconds' FROM worker WHERE id=$1 AND status IN ('active','busy') AND last_heartbeat>clock_timestamp()-interval '90 seconds' FOR SHARE")
            .bind(owner).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        if sensor_fence.is_some() {
            source_deadline = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
                "SELECT lease_expires_at FROM sensor_workload_assignment WHERE workload=$1",
            )
            .bind(workload)
            .fetch_one(&mut **tx)
            .await?;
        }
        if let Some(requested) = sensor_fence {
            if requested != fence {
                return Err(denied());
            }
            let admitted = Self::lock_native_file_allocation(tx, sensor, fence)
                .await?
                .ok_or_else(denied)?;
            if admitted.intent != intent {
                return Err(denied());
            }
        } else if !ready {
            // Stopped publication retains original source identity; no transfer
            // to a replacement incarnation is inferred from an expired lease.
            let same:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sensor_workload_assignment WHERE workload=$1 AND generation=$2 AND worker=$3 AND worker_instance=$4)")
                .bind(workload).bind(generation).bind(owner).bind(incarnation).fetch_one(&mut **tx).await?;
            if !same {
                return Err(denied());
            }
            super::super::super::managed_sensor_metadata::ManagedSensorMetadataRepository::validate_generation(tx,workload,generation).await?;
            let metadata=super::super::super::managed_sensor_metadata::ManagedSensorMetadataRepository::for_generation(tx,workload,generation).await?.ok_or_else(denied)?;
            let epochs = super::super::super::workload_source::snapshot(
                tx,
                vec![
                    ("owner", metadata.owner.sponsor_identity_id),
                    ("sensor", sensor),
                    ("workload", workload),
                    ("worker", owner),
                ],
            )
            .await?;
            let effect = Self::locked_native(tx, workload, intent).await?;
            source_deadline = Some(effect.publication_deadline.ok_or_else(denied)?);
            if !effect.files_bound
                || effect.worker != owner
                || effect.worker_instance != incarnation
                || effect.epochs != epochs
                || effect.generation != generation
                || !effect.stop_requested
                || effect.exit.is_none()
                || effect.resource.is_none()
            {
                return Err(denied());
            }
        }

        super::super::super::runtime_authority::ApprovalEffectFence::from_input_approval(Some(
            source_deadline.map_or(deadline, |source| source.min(deadline)),
        ))
        .map(Some)
    }
    /// Recheck mutable target ownership AFTER its artifact lock/waits. Bindings
    /// cannot disappear from the inventory by changing artifact classification.
    pub async fn require_native_file_target(
        tx: &mut Transaction<'_, Postgres>,
        version: Id,
    ) -> Result<()> {
        let valid:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM artifact_version WHERE id=$1 AND sensor_native_intent IS NOT NULL) OR EXISTS(SELECT 1 FROM artifact_version v JOIN artifact a ON a.id=v.artifact JOIN sensor_workload w ON w.id=v.sensor_workload JOIN sensor s ON s.id=w.sensor WHERE v.id=$1 AND a.scope='sensor' AND a.owner=s.ref AND a.classification<>'runtime_log' AND v.file_path IS NOT NULL)")
            .bind(version).fetch_one(&mut **tx).await?;
        if !valid {
            return Err(denied());
        }
        Ok(())
    }
    pub async fn native_file_inventory(
        pool: &PgPool,
        workload: Id,
        intent: Uuid,
    ) -> Result<Vec<ArtifactVersion>> {
        let mut tx = pool.begin().await?;
        let effect = Self::locked_native(&mut tx, workload, intent).await?;
        if !effect.files_bound
            || !effect.stop_requested
            || effect.exit.is_none()
            || effect.resource.is_none()
        {
            return Err(denied());
        }
        let versions = ArtifactVersionRepository::find_sensor_native_files(
            &mut *tx,
            workload,
            effect.generation,
            intent,
        )
        .await?;
        tx.commit().await?;
        Ok(versions)
    }
    /// Only an exact positive API receipt counts, including an explicit zero.
    pub async fn record_native_file_receipt(
        pool: &PgPool,
        workload: Id,
        intent: Uuid,
        version: Id,
        bytes: u64,
    ) -> Result<()> {
        let bytes = i64::try_from(bytes).map_err(|_| denied())?;
        let mut tx = pool.begin().await?;
        let effect = Self::locked_native(&mut tx, workload, intent).await?;
        if !effect.files_bound || !effect.stop_requested || effect.exit.is_none() {
            return Err(denied());
        }
        let changed=sqlx::query("UPDATE artifact_version SET sensor_publication_receipt_bytes=$4 WHERE id=$1 AND sensor_workload=$2 AND sensor_native_intent=$3 AND sensor_generation=$5 AND body_state='ready' AND size_bytes=$4 AND (sensor_publication_receipt_bytes IS NULL OR sensor_publication_receipt_bytes=$4)")
            .bind(version).bind(workload).bind(intent).bind(bytes).bind(effect.generation).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(denied());
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn native_files_complete(pool: &PgPool, workload: Id, intent: Uuid) -> Result<bool> {
        let mut tx = pool.begin().await?;
        let effect = Self::locked_native(&mut tx, workload, intent).await?;
        if !effect.files_bound
            || !effect.stop_requested
            || effect.exit.is_none()
            || !effect.capture_complete()
        {
            return Ok(false);
        }
        let incomplete:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_version WHERE sensor_workload=$1 AND sensor_generation=$2 AND sensor_native_intent=$3 AND (body_state IS DISTINCT FROM 'ready' OR size_bytes IS NULL OR sensor_publication_receipt_bytes IS DISTINCT FROM size_bytes))")
            .bind(workload).bind(effect.generation).bind(intent).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(!incomplete)
    }
    pub async fn acknowledge_native_removed(
        pool: &PgPool,
        workload: Id,
        intent: Uuid,
        worker: Id,
        instance: Uuid,
    ) -> Result<()> {
        // Only the recorded original manager can acknowledge its exact positive
        // provider removal. Expired assignment is never a custody transfer.
        let mut tx = pool.begin().await?;
        let effect = Self::locked_native(&mut tx, workload, intent).await?;
        if effect.worker != worker
            || effect.worker_instance != instance
            || !effect.files_bound
            || !effect.stop_requested
            || effect.exit.is_none()
            || effect.resource.is_none()
            || !effect.capture_complete()
        {
            return Err(denied());
        }
        let incomplete:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifact_version WHERE sensor_workload=$1 AND sensor_generation=$2 AND sensor_native_intent=$3 AND (body_state IS DISTINCT FROM 'ready' OR size_bytes IS NULL OR sensor_publication_receipt_bytes IS DISTINCT FROM size_bytes))")
            .bind(workload).bind(effect.generation).bind(intent).fetch_one(&mut *tx).await?;
        if incomplete {
            return Err(denied());
        }
        sqlx::query("UPDATE sensor_workload_assignment SET native_effect=NULL WHERE workload=$1")
            .bind(workload)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
