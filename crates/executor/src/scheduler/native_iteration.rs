//! Ordinary iteration entry, refill and status-only terminal aggregation. Cache,
//! wait, retry and generated-output callers are not routed through this driver.
use super::*;
use attune_common::repositories::native_iteration_materialization::NativeIterationMaterializationRepository as Iteration;

impl ExecutionScheduler {
    pub(super) async fn dispatch_native_iteration(
        pool: &PgPool,
        publisher: &Publisher,
        parent: &Execution,
        task: &str,
        triggered_by: Option<&str>,
        encryption_key: Option<&str>,
        completed: Option<&Execution>,
    ) -> Result<bool> {
        if !Iteration::is_ordinary_task(parent, task) {
            return Ok(false);
        }
        let mut tx = pool.begin().await?;
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
            .await?;
        let actual = ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Ordinary iteration parent disappeared"))?;
        if attune_common::repositories::execution_runtime_admission::ExecutionAdmissionRepository::find(
            &mut tx, attune_common::repositories::runtime_authority::RuntimeOwner::Execution(actual.id),
        ).await?.is_none() {
            tx.rollback().await?;
            return Ok(false);
        }
        if actual.status != ExecutionStatus::Running {
            tx.rollback().await?;
            return Ok(completed.is_none());
        }
        let key = encryption_key.ok_or_else(|| {
            anyhow::anyhow!("Ordinary iteration receipt encryption is unavailable")
        })?;
        if let Some(completed) = completed {
            let current = ExecutionRepository::find_by_id(&mut *tx, completed.id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Ordinary completion source disappeared"))?;
            let workflow = WorkflowExecutionRepository::find_by_execution(&mut *tx, actual.id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Ordinary completion workflow disappeared"))?;
            if current.parent != Some(actual.id)
                || !matches!(
                    current.status,
                    ExecutionStatus::Completed
                        | ExecutionStatus::Failed
                        | ExecutionStatus::Timeout
                        | ExecutionStatus::Cancelled
                        | ExecutionStatus::Abandoned
                )
                || current.workflow_task.as_ref().is_none_or(|metadata| {
                    metadata.workflow_execution != workflow.id
                        || metadata.task_name != task
                        || metadata.task_index.is_none()
                })
            {
                return Err(anyhow::anyhow!(
                    "Ordinary completion does not match its retained task/index"
                ));
            }
        }
        if let Some(completed) = completed {
            if Iteration::is_iteration_successor_source(&actual, completed) {
                if let Some(mut activation) =
                    Iteration::prepare_iteration_activation(&mut tx, &actual, completed, key)
                        .await?
                {
                    // The factory already holds the one genuine D/A/B group.
                    // No list render, group capture or Key read occurs behind
                    // this outer workflow target lock.
                    sqlx::query("SELECT pg_advisory_xact_lock($1)")
                        .bind(activation.workflow())
                        .execute(&mut *tx)
                        .await?;
                    let requested = activation.apply(&mut tx, key).await?;
                    activation.ensure_fresh_on(&mut tx).await?;
                    tx.commit().await?;
                    activation.ensure_fresh()?;
                    for child in requested {
                        activation.ensure_fresh()?;
                        Self::publish_native_iteration_child(
                            pool, publisher, actual.id, child, key,
                        )
                        .await?;
                        activation.ensure_fresh()?;
                    }
                    activation.ensure_fresh()?;
                    return Ok(true);
                }
            }
        }
        if let Some(mut recovery) =
            Iteration::recover_empty_activation(&mut tx, &actual, task, key).await?
        {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(recovery.workflow())
                .execute(&mut *tx)
                .await?;
            let requested = recovery.apply(&mut tx, key).await?;
            recovery.ensure_fresh_on(&mut tx).await?;
            tx.commit().await?;
            recovery.ensure_fresh()?;
            for child in requested {
                recovery.ensure_fresh()?;
                Self::publish_native_iteration_child(pool, publisher, actual.id, child, key)
                    .await?;
                recovery.ensure_fresh()?;
            }
            recovery.ensure_fresh()?;
            return Ok(true);
        }
        let mut prepared = Iteration::prepare(&mut tx, &actual, task, triggered_by, key).await?;
        if prepared.needs_empty_activation() {
            let mut activation =
                Iteration::prepare_empty_activation(&mut tx, prepared, key).await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(activation.workflow())
                .execute(&mut *tx)
                .await?;
            let requested = activation.apply(&mut tx, key).await?;
            activation.ensure_fresh_on(&mut tx).await?;
            tx.commit().await?;
            activation.ensure_fresh()?;
            for child in requested {
                activation.ensure_fresh()?;
                Self::publish_native_iteration_child(pool, publisher, actual.id, child, key)
                    .await?;
                activation.ensure_fresh()?;
            }
            activation.ensure_fresh()?;
            return Ok(true);
        }
        let whole = if completed.is_some() {
            Iteration::prepare_completion(&mut tx, actual.id, task, key).await?
        } else {
            None
        };
        // The constructive empty recipe authorizes lifecycle progress. Root
        // generated result Values still belong to the A/H finalizer.
        if prepared.total() == 0 && !prepared.can_complete_empty() {
            return Err(anyhow::anyhow!(
                "Empty ordinary iteration root output requires the typed root finalizer"
            ));
        }
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(prepared.workflow())
            .execute(&mut *tx)
            .await?;
        if let Some(whole) = &whole {
            whole.require_source_current(&mut tx).await?;
        }
        let outcome = prepared.apply(&mut tx, key).await?;
        if let Some(parent) = &outcome.completed_parent {
            if completed.is_some()
                && whole
                    .as_ref()
                    .is_none_or(|whole| whole.status() != parent.status)
            {
                return Err(anyhow::anyhow!(
                    "Ordinary terminal status lacks complete authenticated ordinal coverage"
                ));
            }
        }
        // Status-only terminal aggregation is owned here. Rich result views and
        // successor stages use their typed source frames, never legacy prepass.
        let handled =
            completed.is_none() || outcome.remaining > 0 || outcome.completed_parent.is_some();
        let completed_parent = outcome.completed_parent;
        prepared.ensure_fresh_on(&mut tx).await?;
        if let Some(whole) = &whole {
            whole.ensure_fresh_on(&mut tx).await?;
        }
        tx.commit().await?;
        prepared.ensure_fresh()?;
        if let Some(whole) = &whole {
            whole.ensure_fresh()?;
        }
        for child in outcome.requested {
            prepared.ensure_fresh()?;
            Self::publish_native_iteration_child(pool, publisher, actual.id, child, key).await?;
            prepared.ensure_fresh()?;
        }
        if let Some(completed) = completed_parent {
            let mut delivery = pool.begin().await?;
            CacheEntryRepository::protect_transaction(
                &mut delivery,
                CacheTransactionMode::PinMutation,
            )
            .await?;
            attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(
                &mut delivery,
            )
            .await?;
            let admission = attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::execution_admission(&mut delivery,completed.id).await?
                .ok_or_else(||anyhow::anyhow!("Ordinary terminal admission is absent"))?;
            let fence = attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(&mut delivery,admission.id).await?;
            let controls = prepared
                .prepare_control_delivery(&mut delivery, key)
                .await?;
            let current = ExecutionRepository::find_by_id_for_update(&mut *delivery, completed.id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Ordinary terminal completion is absent"))?;
            if serde_json::to_value(&current)? != serde_json::to_value(&completed)? {
                return Err(anyhow::anyhow!(
                    "Ordinary terminal completion changed before publication"
                ));
            }
            controls.ensure_fresh_on(&mut delivery).await?;
            fence.ensure_fresh_on(&mut delivery).await?;
            prepared.ensure_fresh()?;
            Self::publish_execution_completed_payload(
                publisher,
                PendingExecutionCompleted {
                    execution_id: current.id,
                    action_id: current
                        .action
                        .ok_or_else(|| anyhow::anyhow!("Ordinary terminal action is absent"))?,
                    action_ref: current.action_ref,
                    status: current.status,
                    result: current.result,
                    completed_at: current.updated,
                },
            )
            .await?;
            controls.ensure_fresh_on(&mut delivery).await?;
            fence.ensure_fresh_on(&mut delivery).await?;
            delivery.commit().await?;
            controls.ensure_fresh()?;
            fence.ensure_fresh()?;
            prepared.ensure_fresh()?;
        }
        prepared.ensure_fresh()?;
        if let Some(whole) = &whole {
            whole.ensure_fresh()?;
        }
        Ok(handled)
    }

    /// Non-indexed predecessor activation only. A whole iteration enters through
    /// its own authenticated aggregate factory, never through an item ID here.
    pub(super) async fn activate_native_iteration_successors(
        pool: &PgPool,
        publisher: &Publisher,
        completed: &Execution,
        encryption_key: Option<&str>,
    ) -> Result<bool> {
        if completed.id <= 0
            || completed.status != ExecutionStatus::Completed
            || completed
                .workflow_task
                .as_ref()
                .is_none_or(|task| task.task_index.is_some())
        {
            return Ok(false);
        }
        let Some(parent_id) = completed.parent else {
            return Ok(false);
        };
        let Some(route_parent) = ExecutionRepository::find_by_id(pool, parent_id).await? else {
            return Ok(false);
        };
        if !Iteration::is_ordinary_successor_source(&route_parent, completed) {
            return Ok(false);
        }
        let mut tx = pool.begin().await?;
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut tx)
            .await?;
        let parent = ExecutionRepository::find_by_id(
            &mut *tx,
            completed
                .parent
                .ok_or_else(|| anyhow::anyhow!("Ordinary predecessor parent is absent"))?,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Ordinary predecessor parent disappeared"))?;
        if parent.status!=ExecutionStatus::Running
            || attune_common::repositories::execution_runtime_admission::ExecutionAdmissionRepository::find(
                &mut tx,attune_common::repositories::runtime_authority::RuntimeOwner::Execution(parent.id)).await?.is_none() {
            tx.rollback().await?;
            return Ok(false);
        }
        let Some(key) = encryption_key else {
            tx.rollback().await?;
            return Ok(false);
        };
        let Some(mut prepared) =
            Iteration::prepare_successors(&mut tx, &parent, completed, key).await?
        else {
            tx.rollback().await?;
            return Ok(false);
        };
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(prepared.workflow())
            .execute(&mut *tx)
            .await?;
        let requested = prepared.apply(&mut tx, key).await?;
        prepared.ensure_fresh_on(&mut tx).await?;
        tx.commit().await?;
        prepared.ensure_fresh()?;
        for child in requested {
            prepared.ensure_fresh()?;
            Self::publish_native_iteration_child(pool, publisher, parent.id, child, key).await?;
            prepared.ensure_fresh()?;
        }
        prepared.ensure_fresh()?;
        Ok(true)
    }

    async fn publish_native_iteration_child(
        pool: &PgPool,
        publisher: &Publisher,
        parent: i64,
        child: Execution,
        key: &str,
    ) -> Result<()> {
        // Reopen the SAME sealed graph before targets and broker I/O. There is
        // no rerender, first-seal replay or new human authorization expansion.
        let mut delivery = pool.begin().await?;
        CacheEntryRepository::protect_transaction(&mut delivery, CacheTransactionMode::PinMutation)
            .await?;
        attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(
            &mut delivery,
        )
        .await?;
        let admission=attune_common::repositories::execution_runtime_admission::ExecutionAdmissionRepository::require(
            &mut delivery,attune_common::repositories::runtime_authority::RuntimeOwner::Execution(child.id)).await?;
        let fence=attune_common::repositories::runtime_authority::RuntimeAuthorityRepository::retained_admission_fence(&mut delivery,admission.id).await?;
        let receipts=attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::prepare_execution_input_receipts(
            &mut delivery,child.id,key).await?;
        let current_parent = ExecutionRepository::find_by_id_for_update(&mut *delivery, parent)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Ordinary iteration publication parent disappeared"))?;
        if current_parent.status != ExecutionStatus::Running {
            delivery.rollback().await?;
            return Ok(());
        }
        let current = ExecutionRepository::find_by_id_for_update(&mut *delivery, child.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Ordinary iteration publication child disappeared"))?;
        if current.status != ExecutionStatus::Requested {
            delivery.rollback().await?;
            return Ok(());
        }
        if current.config != child.config
            || current.parent != child.parent
            || current.enforcement != child.enforcement
            || current.pack_release != child.pack_release
            || current.pack_release_digest != child.pack_release_digest
            || current.workflow_task != child.workflow_task
            || current.action != child.action
            || current.action_ref != child.action_ref
            || current.permission_set_refs != child.permission_set_refs
            || serde_json::to_value(&current.executable_snapshot)?
                != serde_json::to_value(&child.executable_snapshot)?
        {
            return Err(anyhow::anyhow!(
                "Ordinary iteration child changed before broker publication"
            ));
        }
        for receipt in &receipts {
            receipt.ensure_fresh_on(&mut delivery).await?;
        }
        fence.ensure_fresh_on(&mut delivery).await?;
        Self::publish_execution_requested_payload(
            publisher,
            PendingExecutionRequested {
                execution_id: child.id,
                action_id: child
                    .action
                    .ok_or_else(|| anyhow::anyhow!("Ordinary child action is absent"))?,
                action_ref: child.action_ref,
                parent_id: parent,
                enforcement_id: child.enforcement,
                config: child.config,
                release_id: child.pack_release,
                release_digest: child.pack_release_digest,
            },
        )
        .await?;
        for receipt in &receipts {
            receipt.ensure_fresh_on(&mut delivery).await?;
        }
        fence.ensure_fresh_on(&mut delivery).await?;
        delivery.commit().await?;
        for receipt in &receipts {
            receipt.ensure_fresh()?;
        }
        fence.ensure_fresh()?;
        Ok(())
    }
}
