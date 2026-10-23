//! Replay the original cache activation, never its selectors or a new frontier.
use super::super::{
    accepted_input::AcceptedInputRepository,
    requested_execution_publication::{
        PreparedRequestedPublication, RequestedExecutionPublicationRepository as Requested,
    },
    workflow_cache_iteration::WorkflowCacheIterationRepository as Iterations,
};
use super::*;

pub(super) struct Replay {
    iteration: crate::models::WorkflowCacheIteration,
    frontiers: Vec<(Id, i64, String)>,
    dispatches: Vec<(i32, Id)>,
    children: Vec<Execution>,
    requested: Vec<PreparedRequestedPublication>,
    fences: Vec<ApprovalEffectFence>,
    sources: Vec<super::super::native_input_snapshot::NativeSourceEvidence>,
    receipts: Vec<ReceiptContext>,
}

impl Replay {
    pub(super) async fn prepare(
        conn: &mut PgConnection,
        context: &Context,
        name: &str,
        key: &str,
    ) -> Result<Option<Self>> {
        let Some(iteration) =
            Iterations::find_by_workflow_task(&mut *conn, context.workflow.id, name).await?
        else {
            return Ok(None);
        };
        // Pin the actual stream before target locks. Its creation epoch selects
        // the first activation's original pages, not later refill publications.
        let _: Id =
            sqlx::query_scalar("SELECT id FROM workflow_cache_iteration WHERE id=$1 FOR SHARE")
                .bind(iteration.id)
                .fetch_one(&mut *conn)
                .await?;
        let frontiers = frontiers(conn, iteration.id).await?;
        if frontiers.is_empty() || frontiers.len() > 256 || frontiers[0].1 != 0 {
            return Err(denied());
        }
        let graph = &context
            .parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let task = super::continuation::unique_task(graph, name)?;
        let action = task
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        let task_path =
            Materialize::workflow_task_site_path(conn, context.parent.id, name, action).await?;
        let path = format!("{task_path}/params");
        let parent_admission = Authority::execution_admission(conn, context.parent.id)
            .await?
            .ok_or_else(denied)?;
        let mut replay = Self {
            iteration,
            frontiers,
            dispatches: vec![],
            children: vec![],
            requested: vec![],
            fences: vec![],
            sources: vec![],
            receipts: vec![],
        };
        for (position, (snapshot, ordinal, _)) in replay.frontiers.iter().enumerate() {
            if *ordinal != i64::try_from(position).map_err(|_| denied())? {
                return Err(denied());
            }
            let storage = Snapshots::storage_evidence(conn, *snapshot).await?;
            let cache = storage.cache().ok_or_else(denied)?;
            if cache.iteration() != replay.iteration.id
                || cache.workflow() != context.workflow.id
                || cache.namespace() != replay.iteration.namespace
                || cache.generation() != replay.iteration.generation
                || cache.ordinal() != *ordinal
                || cache.page_size() != replay.iteration.page_size
                || cache.batch_size() != replay.iteration.batch_size
            {
                return Err(denied());
            }
            let evidence = Snapshots::evidence(
                conn,
                *snapshot,
                parent_admission.id,
                &path,
                "/item",
                key,
                false,
            )
            .await?;
            replay.fences.push(evidence.fence);
            for (receipt, site) in evidence.parents {
                let origin = crate::secret_provenance::SecretOrigin::Entity {
                    entity_type: site.kind.origin_kind(site.purpose),
                    entity_id: receipt,
                    path: site.path.clone(),
                };
                replay
                    .receipts
                    .push(Inputs::receipt_context(conn, &site, &origin, key).await?);
            }
            replay
                .sources
                .push(Snapshots::source_evidence(conn, storage.metadata(), key).await?);
            if cache.entries().is_empty() {
                if !cache.exhausted() || position + 1 != replay.frontiers.len() {
                    return Err(denied());
                }
                continue;
            }
            let index = i32::try_from(*ordinal).map_err(|_| denied())?;
            let execution:Id=sqlx::query_scalar("SELECT execution_id FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name=$2 AND task_index=$3 FOR SHARE")
                .bind(context.workflow.id).bind(name).bind(index).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
            let child = Executions::find_by_id(&mut *conn, execution)
                .await?
                .ok_or_else(denied)?;
            let metadata = child.workflow_task.as_ref().ok_or_else(denied)?;
            if child.parent != Some(context.parent.id)
                || child.action_ref != action
                || metadata.workflow_execution != context.workflow.id
                || metadata.task_name != name
                || metadata.task_index != Some(index)
                || metadata
                    .task_batch
                    .and_then(|count| usize::try_from(count).ok())
                    != Some(cache.entries().len())
                || metadata.retry_count != 0
                || child.retry_count != 0
                || child.original_execution.is_some()
            {
                return Err(denied());
            }
            replay
                .fences
                .push(AcceptedInputRepository::revalidate_execution(conn, child.id, key).await?);
            replay
                .receipts
                .extend(Materialize::prepare_execution_input_receipts(conn, child.id, key).await?);
            if child.status == ExecutionStatus::Requested {
                replay.requested.push(
                    Requested::prepare(conn, &child, key)
                        .await?
                        .ok_or_else(denied)?,
                );
            }
            replay.dispatches.push((index, child.id));
            replay.children.push(child);
        }
        if replay.children.is_empty()
            && (replay.iteration.dispatched_count != 0
                || replay.iteration.state != crate::models::WorkflowCacheIterationState::Completed)
        {
            return Err(denied());
        }
        replay.ensure_fresh_on(conn).await?;
        Ok(Some(replay))
    }
    pub(super) fn task(&self) -> &str {
        &self.iteration.task_name
    }
    pub(super) fn receipts(&self) -> &[ReceiptContext] {
        &self.receipts
    }
    pub(super) async fn lock_and_pending(&self, conn: &mut PgConnection) -> Result<Vec<Execution>> {
        self.ensure_fresh_on(conn).await?;
        for requested in &self.requested {
            requested.lock_target(conn).await?;
        }
        for child in &self.children {
            let actual = Executions::find_by_id(&mut *conn, child.id)
                .await?
                .ok_or_else(denied)?;
            if serde_json::to_value(actual)? != serde_json::to_value(child)? {
                return Err(denied());
            }
        }
        self.ensure_fresh_on(conn).await?;
        Ok(self
            .requested
            .iter()
            .map(|requested| requested.execution().clone())
            .collect())
    }
    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        let actual = Iterations::find_by_workflow_task(
            &mut *conn,
            self.iteration.workflow_execution,
            &self.iteration.task_name,
        )
        .await?
        .ok_or_else(denied)?;
        if serde_json::to_value(actual)? != serde_json::to_value(&self.iteration)?
            || frontiers(conn, self.iteration.id).await? != self.frontiers
        {
            return Err(denied());
        }
        for (index, execution) in &self.dispatches {
            let assigned:Option<Id>=sqlx::query_scalar("SELECT execution_id FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name=$2 AND task_index=$3")
                .bind(self.iteration.workflow_execution).bind(&self.iteration.task_name).bind(index).fetch_optional(&mut *conn).await?;
            if assigned != Some(*execution) {
                return Err(denied());
            }
        }
        for requested in &self.requested {
            requested.ensure_fresh_on(conn).await?;
        }
        for fence in &self.fences {
            fence.ensure_fresh_on(conn).await?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        self.ensure_fresh()
    }
    pub(super) fn ensure_fresh(&self) -> Result<()> {
        for fence in &self.fences {
            fence.ensure_fresh()?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        for source in &self.sources {
            source
                .cache_read_policy()
                .ok_or_else(denied)?
                .ensure_fresh()?;
        }
        Ok(())
    }
}

async fn frontiers(conn: &mut PgConnection, iteration: Id) -> Result<Vec<(Id, i64, String)>> {
    Ok(sqlx::query_as("SELECT f.snapshot,f.ordinal,f.bound_transaction FROM runtime_native_cache_frontier f JOIN runtime_cache_iteration_epoch e ON e.iteration=f.bound_iteration WHERE f.bound_iteration=$1 AND f.bound_transaction=f.prepared_transaction AND f.bound_transaction=e.creation_transaction ORDER BY f.ordinal LIMIT 257 FOR SHARE OF f,e")
        .bind(iteration).fetch_all(&mut *conn).await?)
}
