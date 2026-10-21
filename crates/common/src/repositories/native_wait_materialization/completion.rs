//! Original wait-output SOURCE only. This token is neither a generated VALUE
//! receipt nor an ordinary child/iteration completion witness.
use super::super::{
    accepted_input::AcceptedInputRepository, native_cache_materialization::NativeCacheRenderFrame,
    schema_contract::ActionSchemaBinding,
};
use super::*;
use std::sync::Arc;

/// Liveness only. The current Wait's authenticated observation remains the
/// sole failure VALUE source. Other task names do not grant authority.
pub(super) async fn can_finish_root(
    conn: &mut PgConnection,
    context: &Context,
    wait: Id,
) -> Result<bool> {
    let task = context
        .task
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    no_other_work(conn, &context.workflow, task, wait).await
}

async fn no_other_work(
    conn: &mut PgConnection,
    workflow: &WorkflowExecution,
    task: &str,
    wait: Id,
) -> Result<bool> {
    if wait <= 0
        || workflow.current_tasks.iter().any(|name| name != task)
        || workflow.completed_tasks.iter().any(|name| name == task)
        || workflow.failed_tasks.iter().any(|name| name == task)
    {
        return Ok(false);
    }
    let mut applied = workflow.completed_tasks.clone();
    applied.extend(workflow.failed_tasks.iter().cloned());
    applied.push(task.into());
    // Match the scheduler's running-child policy. A terminal wait which is
    // already represented in the projected task lists is applied bookkeeping,
    // even if its post-commit delivery marker has not yet been written.
    sqlx::query_scalar(
        "SELECT NOT (EXISTS (
            SELECT 1 FROM execution
            WHERE workflow_task->>'workflow_execution' = $1::bigint::text
              AND status::text NOT IN ('completed','failed','timeout','cancelled')
              AND COALESCE(workflow_task->>'task_name','') <> ALL($2::text[])
        ) OR EXISTS (
            SELECT 1 FROM workflow_task_wait
            WHERE workflow_execution=$1 AND id<>$3
              AND (state::text='waiting'
                OR (state::text IN ('failed','timed_out')
                  AND (result->>'_delivery_complete') IS DISTINCT FROM 'true'
                  AND task_name <> ALL($2::text[])))
        ))",
    )
    .bind(workflow.id)
    .bind(&applied)
    .bind(wait)
    .fetch_one(&mut *conn)
    .await
    .map_err(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitCompletionOutcome {
    Failed,
    TimedOut,
    Cancelled,
}

/// Only the owning wait preparation path constructs this capability. Its recipe
/// has an unhandled genuine terminal observation and no selected successor.
/// Output JSON, a claimed status or a caller-selected wait ID cannot issue it.
pub struct PreparedNativeWaitCompletion {
    predecessor:
        Option<Arc<super::super::native_transition_materialization::PreparedNativeWaitPredecessor>>,
    parent: Execution,
    workflow: WorkflowExecution,
    task_path: String,
    task: String,
    wait: Id,
    snapshot: Id,
    renderer: Id,
    outcome: WaitCompletionOutcome,
    contract: ActionSchemaBinding,
    parent_fence: ApprovalEffectFence,
    program: Arc<staged::Program>,
    frame: Arc<cache_frame::Frame>,
}
impl PreparedNativeWaitCompletion {
    pub(super) fn require_writer(
        &self,
        program: &Arc<staged::Program>,
        frame: &Arc<cache_frame::Frame>,
    ) -> Result<()> {
        if !Arc::ptr_eq(&self.program, program) || !Arc::ptr_eq(&self.frame, frame) {
            return Err(denied());
        }
        Ok(())
    }
    pub(super) async fn prepare(
        conn: &mut PgConnection,
        context: &Context,
        observation: &Observation,
        program: Arc<staged::Program>,
        frame: Arc<cache_frame::Frame>,
        key: &str,
    ) -> Result<Self> {
        let task = context
            .task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        if !program.successors.is_empty()
            || context
                .workflow
                .failed_tasks
                .iter()
                .any(|name| name == task)
            || context
                .workflow
                .completed_tasks
                .iter()
                .any(|name| name == task)
            || context
                .workflow
                .current_tasks
                .iter()
                .any(|name| context.task.get("name").and_then(Value::as_str) != Some(name.as_str()))
        {
            return Err(denied());
        }
        let binding = match observation {
            Observation::Pending(wait) => wait.outcome_binding()?,
            Observation::Released(wait) => wait.outcome_binding()?,
            Observation::Prospective(registration) => registration
                .prepared_input()
                .ok_or_else(denied)?
                .outcome_binding()?,
        };
        let (wait, snapshot) = frame.source_identity();
        if !can_finish_root(conn, context, wait).await? {
            return Err(denied());
        }
        let renderer = Authority::execution_admission(conn, context.parent.id)
            .await?
            .ok_or_else(denied)?
            .id;
        if wait <= 0
            || binding.renderer != renderer
            || binding.snapshot != snapshot
            || binding.task_path != context.path
        {
            return Err(denied());
        }
        // Classification is from A's validated original WaitSource. These
        // booleans select a kind; they are never persisted as a VALUE proof.
        let outcome = match binding.values.map(|value| value.as_bool()) {
            [Some(false), Some(true), Some(false), Some(false)] => WaitCompletionOutcome::Failed,
            [Some(false), Some(false), Some(true), Some(false)] => WaitCompletionOutcome::TimedOut,
            [Some(false), Some(false), Some(false), Some(true)] => WaitCompletionOutcome::Cancelled,
            _ => return Err(denied()),
        };
        let contract = Schemas::find(conn, BindingOwner::Execution, context.parent.id)
            .await?
            .ok_or_else(denied)?;
        let parent_fence =
            AcceptedInputRepository::revalidate_execution(conn, context.parent.id, key).await?;
        let source = Self {
            predecessor: context.predecessor.clone(),
            parent: context.parent.clone(),
            workflow: context.workflow.clone(),
            task_path: context.path.clone(),
            task: context
                .task
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(denied)?
                .into(),
            wait,
            snapshot,
            renderer,
            outcome,
            contract,
            parent_fence,
            program,
            frame,
        };
        source.require_source_current(conn).await?;
        Ok(source)
    }
    pub fn execution(&self) -> Id {
        self.parent.id
    }
    pub fn workflow(&self) -> Id {
        self.workflow.id
    }
    pub fn wait(&self) -> Id {
        self.wait
    }
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub fn renderer_admission(&self) -> Id {
        self.renderer
    }
    pub fn task_name(&self) -> &str {
        &self.task
    }
    pub fn task_path(&self) -> &str {
        &self.task_path
    }
    pub fn outcome(&self) -> WaitCompletionOutcome {
        self.outcome
    }
    pub fn original_parent(&self) -> &Execution {
        &self.parent
    }
    pub fn original_workflow(&self) -> &WorkflowExecution {
        &self.workflow
    }
    pub fn original_output_contract(&self) -> &ActionSchemaBinding {
        &self.contract
    }
    pub fn condition_sources(&self) -> &[super::super::inherited_input::PreparedInputSource] {
        self.program.controls()
    }
    pub fn issued_values(
        &self,
    ) -> impl Iterator<Item = &super::super::inherited_input::PreparedStagedValue> {
        self.program.issued_values()
    }
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(self
            .parent_fence
            .approval_deadline()
            .into_iter()
            .chain(self.frame.held_deadline()?)
            .chain(
                self.predecessor
                    .as_ref()
                    .map(|source| source.held_deadline())
                    .transpose()?
                    .flatten(),
            )
            .min())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(predecessor) = &self.predecessor {
            predecessor.ensure_fresh_on(conn).await?;
        }
        self.parent_fence.ensure_fresh_on(conn).await?;
        self.program.ensure_fresh_on(conn).await?;
        self.frame.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Some(predecessor) = &self.predecessor {
            predecessor.ensure_fresh()?;
        }
        self.parent_fence.ensure_fresh()?;
        self.program.ensure_fresh()?;
        self.frame.ensure_fresh()
    }
    pub(crate) async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        let workflow = Workflows::find_by_id(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        if let Some(predecessor) = &self.predecessor {
            predecessor.ensure_fresh_on(conn).await?;
            predecessor.require_source_workflow(&workflow)?;
            if serde_json::to_value(predecessor.projected())?
                != serde_json::to_value(&self.workflow)?
            {
                return Err(denied());
            }
        } else if serde_json::to_value(&workflow)? != serde_json::to_value(&self.workflow)? {
            return Err(denied());
        }
        if serde_json::to_value(
            Executions::find_by_id(&mut *conn, self.parent.id)
                .await?
                .ok_or_else(denied)?,
        )? != serde_json::to_value(&self.parent)?
            || Schemas::find(conn, BindingOwner::Execution, self.parent.id)
                .await?
                .as_ref()
                != Some(&self.contract)
        {
            return Err(denied());
        }
        // Recheck liveness under the writer's original-row guard. This does
        // not reopen protected sources or reconstruct an approval.
        if !no_other_work(conn, &self.workflow, &self.task, self.wait).await? {
            return Err(denied());
        }
        Ok(())
    }
    /// Public observation access stays closed until the owning writer has bound
    /// and released its exact reservation and sealed its ordered publication.
    /// A's future kind-specific output recipe must borrow this token itself,
    /// not treat the returned observation/value or snapshot ID as authority.
    pub async fn released_observation(
        &self,
        conn: &mut PgConnection,
    ) -> Result<&ReleasedWaitInput> {
        self.ensure_fresh_on(conn).await?;
        self.frame.require_published(conn).await?;
        if Schemas::find(conn, BindingOwner::Execution, self.parent.id)
            .await?
            .as_ref()
            != Some(&self.contract)
        {
            return Err(denied());
        }
        self.frame.released_observation()
    }
}
