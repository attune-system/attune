//! Owning fan-out boundary for B's sealed ordinary transition frame. The frame
//! publishes once; all list/input preparation happens before outer target locks.
use super::*;
use crate::repositories::native_transition_materialization::{
    NativeTransitionMaterializationRepository as Transitions, PreparedNativeEmptyTransitions,
    PreparedNativeIterationTransitions, PreparedNativeOrdinaryTransitions,
    RecordedNativeTransitions,
};

pub(super) enum ActivationFrame {
    Empty(Arc<PreparedNativeEmptyTransitions>),
    Ordinary(Arc<PreparedNativeOrdinaryTransitions>),
    Iteration(Arc<PreparedNativeIterationTransitions>),
}
impl ActivationFrame {
    fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        match self {
            Self::Empty(frame) => frame.held_deadline(),
            Self::Ordinary(frame) => frame.held_deadline(),
            Self::Iteration(frame) => frame.held_deadline(),
        }
    }
    fn workflow(&self) -> &WorkflowExecution {
        match self {
            Self::Empty(frame) => frame.workflow(),
            Self::Ordinary(frame) => frame.workflow(),
            Self::Iteration(frame) => frame.workflow(),
        }
    }
    fn as_frame(&self) -> &dyn NativeTransitionRenderFrame {
        match self {
            Self::Empty(frame) => frame.as_ref(),
            Self::Ordinary(frame) => frame.as_ref(),
            Self::Iteration(frame) => frame.as_ref(),
        }
    }
    fn render_frame(&self) -> Arc<dyn NativeTransitionRenderFrame> {
        match self {
            Self::Empty(frame) => frame.clone(),
            Self::Ordinary(frame) => frame.clone(),
            Self::Iteration(frame) => frame.clone(),
        }
    }
    fn target_path(&self, name: &str) -> Result<&str> {
        match self {
            Self::Empty(frame) => frame.target_path(name),
            Self::Ordinary(frame) => frame.target_path(name),
            Self::Iteration(frame) => frame.target_path(name),
        }
    }
    async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        match self {
            Self::Empty(frame) => frame.require_source_current(conn).await,
            Self::Ordinary(frame) => frame.require_source_current(conn).await,
            Self::Iteration(frame) => frame.require_source_current(conn).await,
        }
    }
    async fn record(
        &self,
        conn: &mut PgConnection,
        actual: &WorkflowExecution,
        key: &str,
    ) -> Result<RecordedNativeTransitions> {
        match self {
            Self::Empty(frame) => frame.record(conn, actual, key).await,
            Self::Ordinary(frame) => frame.record(conn, actual, key).await,
            Self::Iteration(frame) => frame.record(conn, actual, key).await,
        }
    }
    async fn require_members_current(&self, conn: &mut PgConnection) -> Result<()> {
        match self {
            Self::Empty(frame) => frame.require_members_current(conn).await,
            Self::Ordinary(frame) => frame.as_ref().ensure_fresh_on(conn).await,
            Self::Iteration(frame) => frame.require_members_current(conn).await,
        }
    }
}

pub struct PreparedNativeIterationActivation {
    parent: Execution,
    workflow: WorkflowExecution,
    completed: Execution,
    source_status: ExecutionStatus,
    fields: Vec<StoredSecretValue>,
    parent_fence: ApprovalEffectFence,
    frame: Option<ActivationFrame>,
    targets: Vec<PreparedNativeIteration>,
    applied: bool,
}

impl NativeIterationMaterializationRepository {
    /// Syntax-only fast routing, never a source capability. Cache/wait graphs
    /// stay with their owners before D opens a preparation transaction.
    pub fn is_ordinary_successor_source(parent: &Execution, completed: &Execution) -> bool {
        let Some(metadata) = &completed.workflow_task else {
            return false;
        };
        if completed.id <= 0
            || completed.status != ExecutionStatus::Completed
            || metadata.task_index.is_some()
        {
            return false;
        }
        let Some(tasks) = parent
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
            .and_then(|workflow| workflow.definition.get("tasks"))
            .and_then(Value::as_array)
        else {
            return false;
        };
        tasks.iter().all(|task| {
            ["iterate_cache", "wait_for", "tasks", "join"]
                .iter()
                .all(|field| task.get(*field).is_none_or(Value::is_null))
                && task.get("retry").is_none_or(|retry| {
                    retry.is_null() || retry.get("count").and_then(Value::as_i64) == Some(0)
                })
        }) && authored_ordinary_successor(parent, &metadata.task_name)
    }
    pub async fn prepare_successors(
        conn: &mut PgConnection,
        parent: &Execution,
        completed: &Execution,
        key: &str,
    ) -> Result<Option<PreparedNativeIterationActivation>> {
        let Some(metadata) = &completed.workflow_task else {
            return Ok(None);
        };
        // This branch is not a whole-iteration outcome. No indexed child may
        // enter the transition factory as if it represented its siblings.
        if completed.id <= 0
            || metadata.task_index.is_some()
            || completed.status != ExecutionStatus::Completed
        {
            return Ok(None);
        }
        if !Self::is_ordinary_successor_source(parent, completed) {
            return Ok(None);
        }
        let actual = Executions::find_by_id(&mut *conn, completed.id)
            .await?
            .ok_or_else(changed)?;
        if actual.parent != Some(parent.id)
            || serde_json::to_value(&actual)? != serde_json::to_value(completed)?
        {
            return Err(changed());
        }
        let frame = Transitions::prepare_ordinary_successors(conn, actual.id, key)
            .await
            .map_err(|error| preparation_error(error, "predecessor-frame"))?
            .map(|frame| ActivationFrame::Ordinary(Arc::new(frame)));
        let workflow = Workflows::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(changed)?;
        if workflow.id != metadata.workflow_execution || workflow.status != ExecutionStatus::Running
        {
            return Ok(None);
        }
        let names = if let Some(frame) = &frame {
            if serde_json::to_value(frame.workflow())? != serde_json::to_value(&workflow)? {
                return Err(changed());
            }
            frame.as_frame().selected_targets().to_vec()
        } else if workflow.completed_tasks.contains(&metadata.task_name) {
            // Recover only actual committed original children attributed to this
            // predecessor. Do not rerender its publications or selection table.
            committed_successors(conn, parent.id, workflow.id, &metadata.task_name).await?
        } else {
            return Ok(None);
        };
        if names.is_empty() {
            if workflow.completed_tasks.contains(&metadata.task_name)
                && authored_ordinary_successor(parent, &metadata.task_name)
            {
                // Lost retained evidence must not become a fresh activation.
                return Err(changed());
            }
            return Ok(None);
        }
        let fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?;
        let parent_fence = activation_parent_fence(conn, parent.id).await?;
        let targets = prepare_targets(
            conn,
            parent,
            &metadata.task_name,
            names,
            frame.as_ref(),
            key,
        )
        .await?;
        let prepared = PreparedNativeIterationActivation {
            parent: parent.clone(),
            workflow,
            completed: actual,
            source_status: ExecutionStatus::Completed,
            fields,
            parent_fence,
            frame,
            targets,
            applied: false,
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(Some(prepared))
    }

    /// Indexed completion routes only through the genuine whole-source factory.
    /// The delivered item's status is never the group's lifecycle decision.
    pub async fn prepare_iteration_activation(
        conn: &mut PgConnection,
        parent: &Execution,
        completed: &Execution,
        key: &str,
    ) -> Result<Option<PreparedNativeIterationActivation>> {
        if !Self::is_iteration_successor_source(parent, completed) {
            return Ok(None);
        }
        let metadata = completed.workflow_task.as_ref().ok_or_else(changed)?;
        let actual = Executions::find_by_id(&mut *conn, completed.id)
            .await?
            .ok_or_else(changed)?;
        if actual.parent != Some(parent.id)
            || serde_json::to_value(&actual)? != serde_json::to_value(completed)?
        {
            return Err(changed());
        }
        let workflow = Workflows::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(changed)?;
        if workflow.id != metadata.workflow_execution
            || workflow.status != ExecutionStatus::Running
            || workflow.paused
            || parent.status != ExecutionStatus::Running
        {
            return Ok(None);
        }
        let processed = workflow.completed_tasks.contains(&metadata.task_name)
            || workflow.failed_tasks.contains(&metadata.task_name);
        if workflow.completed_tasks.contains(&metadata.task_name)
            && workflow.failed_tasks.contains(&metadata.task_name)
        {
            return Err(changed());
        }
        let (frame, names, source_status) = if processed {
            // No new D/A group or publication capture on duplicate delivery.
            let names =
                committed_successors(conn, parent.id, workflow.id, &metadata.task_name).await?;
            let status = if workflow.failed_tasks.contains(&metadata.task_name) {
                ExecutionStatus::Failed
            } else {
                ExecutionStatus::Completed
            };
            (None, names, status)
        } else {
            let Some(source) =
                Self::prepare_completion(conn, parent.id, &metadata.task_name, key).await?
            else {
                return Ok(None);
            };
            let source = Arc::new(source);
            let status = source.status();
            let Some(frame) = Transitions::prepare_iteration_successors(conn, source, key)
                .await
                .map_err(|error| preparation_error(error, "whole-iteration-frame"))?
            else {
                return Ok(None);
            };
            if serde_json::to_value(frame.workflow())? != serde_json::to_value(&workflow)? {
                return Err(changed());
            }
            let names = frame.selected_targets().to_vec();
            (
                Some(ActivationFrame::Iteration(Arc::new(frame))),
                names,
                status,
            )
        };
        let fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?;
        let parent_fence = activation_parent_fence(conn, parent.id).await?;
        let targets = prepare_targets(
            conn,
            parent,
            &metadata.task_name,
            names,
            frame.as_ref(),
            key,
        )
        .await?;
        let prepared = PreparedNativeIterationActivation {
            parent: parent.clone(),
            workflow,
            completed: actual,
            source_status,
            fields,
            parent_fence,
            frame,
            targets,
            applied: false,
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(Some(prepared))
    }

    /// Syntax-only routing; all authentication remains in D/A/B factories.
    pub fn is_iteration_successor_source(parent: &Execution, completed: &Execution) -> bool {
        let Some(metadata) = &completed.workflow_task else {
            return false;
        };
        if completed.id <= 0
            || metadata.task_index.is_none()
            || !matches!(
                completed.status,
                ExecutionStatus::Completed
                    | ExecutionStatus::Failed
                    | ExecutionStatus::Timeout
                    | ExecutionStatus::Cancelled
                    | ExecutionStatus::Abandoned
            )
        {
            return false;
        }
        let Some(tasks) = parent
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
            .and_then(|workflow| workflow.definition.get("tasks"))
            .and_then(Value::as_array)
        else {
            return false;
        };
        Self::is_ordinary_task(parent, &metadata.task_name)
            && tasks.iter().all(|task| {
                ["iterate_cache", "wait_for", "tasks", "join"]
                    .iter()
                    .all(|field| task.get(*field).is_none_or(Value::is_null))
                    && (task.get("name").and_then(Value::as_str)
                        == Some(metadata.task_name.as_str())
                        || task.get("retry").is_none_or(|retry| {
                            retry.is_null() || retry.get("count").and_then(Value::as_i64) == Some(0)
                        }))
            })
            && tasks.iter().any(|task| {
                task.get("name").and_then(Value::as_str) == Some(metadata.task_name.as_str())
                    && task
                        .get("next")
                        .and_then(Value::as_array)
                        .is_some_and(|next| !next.is_empty())
            })
    }
}

pub(super) async fn prepare_targets(
    conn: &mut PgConnection,
    parent: &Execution,
    source: &str,
    names: Vec<String>,
    frame: Option<&ActivationFrame>,
    key: &str,
) -> Result<Vec<PreparedNativeIteration>> {
    let mut targets = Vec::new();
    let mut total = 0usize;
    let mut bytes = 0usize;
    for name in names {
        if !NativeIterationMaterializationRepository::is_ordinary_task(parent, &name) {
            return Err(changed());
        }
        let target = NativeIterationMaterializationRepository::prepare_inner(
            conn,
            parent,
            &name,
            Some(source),
            key,
            frame.map(ActivationFrame::render_frame),
            frame
                .map(ActivationFrame::held_deadline)
                .transpose()?
                .flatten(),
        )
        .await?;
        if let Some(frame) = frame {
            if frame.target_path(&name)? != target.context.path {
                return Err(changed());
            }
        }
        if frame.is_some() && !target.original_children.is_empty() {
            return Err(changed());
        }
        total = total.checked_add(target.total()).ok_or_else(changed)?;
        if let Some(array) = &target.array {
            bytes = bytes
                .checked_add(
                    serde_json::to_vec(array.value())?
                        .len()
                        .checked_mul(target.total())
                        .ok_or_else(changed)?,
                )
                .ok_or_else(changed)?;
        }
        for child in &target.children {
            bytes = bytes
                .checked_add(serde_json::to_vec(&child.value)?.len())
                .ok_or_else(changed)?;
        }
        if total > MAX_CHILDREN || bytes > MAX_MATERIALIZED_BYTES {
            return Err(Error::validation(
                "Ordinary successor fan-out exceeds its source/materialization bound",
            ));
        }
        // An actual empty Control now issues a distinct constructive recipe.
        // This boundary settles leaf empties, not an invented member group.
        if target.total() == 0
            && (target.empty_outcome.is_none()
                || target.context.task.get("next").is_some_and(|next| {
                    !next.is_null() && !next.as_array().is_some_and(Vec::is_empty)
                }))
        {
            return Err(Error::validation(
                "Empty successor transition program requires its empty-source frame",
            ));
        }
        targets.push(target);
    }
    Ok(targets)
}

impl PreparedNativeIterationActivation {
    pub fn iteration_source(&self) -> Option<&Arc<PreparedNativeIterationCompletion>> {
        match &self.frame {
            Some(ActivationFrame::Iteration(frame)) => Some(frame.source()),
            _ => None,
        }
    }
    pub fn workflow(&self) -> Id {
        self.workflow.id
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.parent_fence.ensure_fresh_on(conn).await?;
        if let Some(frame) = &self.frame {
            frame.as_frame().ensure_fresh_on(conn).await?;
        }
        for target in &self.targets {
            target.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if !self.applied {
            return Err(changed());
        }
        self.parent_fence.ensure_fresh()?;
        if let Some(frame) = &self.frame {
            frame.as_frame().ensure_fresh()?;
        }
        for target in &self.targets {
            target.ensure_fresh()?;
        }
        Ok(())
    }
    pub async fn apply(&mut self, conn: &mut PgConnection, key: &str) -> Result<Vec<Execution>> {
        if self.applied {
            return Err(changed());
        }
        self.applied = true;
        let mut baseline = Workflows::find_by_id_for_update(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(changed)?;
        if serde_json::to_value(&baseline)? != serde_json::to_value(&self.workflow)?
            || serde_json::to_value(
                Executions::find_by_id(&mut *conn, self.parent.id)
                    .await?
                    .ok_or_else(changed)?,
            )? != serde_json::to_value(&self.parent)?
            || serde_json::to_value(
                Executions::find_by_id(&mut *conn, self.completed.id)
                    .await?
                    .ok_or_else(changed)?,
            )? != serde_json::to_value(&self.completed)?
            || Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, self.parent.id)
                .await?
                != self.fields
        {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        for target in &self.targets {
            if let Some(empty) = &target.empty_outcome {
                empty.guard_original(conn).await?;
            }
            if let Some(control) = &target.iteration_control {
                control.require_original_workflow(&baseline)?;
            }
        }
        if let Some(frame) = &self.frame {
            frame.require_source_current(conn).await?;
            let recorded = frame.record(conn, &baseline, key).await?;
            Secrets::replace_entity_with_conn(
                conn,
                ENTITY_WORKFLOW_VARIABLES,
                self.parent.id,
                &recorded.secrets,
            )
            .await?;
            baseline = Workflows::update(
                &mut *conn,
                baseline.id,
                UpdateWorkflowExecutionInput {
                    variables: Some(recorded.variables),
                    ..Default::default()
                },
            )
            .await?;
            // Authenticate the private publication seal before any task/list or
            // dispatch effect. Never feed a caller buffer back into that seal.
            frame.as_frame().require_published(conn).await?;
        }
        for target in &self.targets {
            if let Some(control) = &target.iteration_control {
                control.finalize(conn).await?;
            }
        }
        let fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, self.parent.id)
                .await?;
        let source = &self
            .completed
            .workflow_task
            .as_ref()
            .ok_or_else(changed)?
            .task_name;
        let mut completed = baseline.completed_tasks.clone();
        let mut failed = baseline.failed_tasks.clone();
        if self.source_status == ExecutionStatus::Completed && !completed.contains(source) {
            completed.push(source.clone());
        }
        if self.source_status == ExecutionStatus::Failed && !failed.contains(source) {
            failed.push(source.clone());
        }
        let mut current = baseline.current_tasks.clone();
        current.retain(|name| name != source);
        let mut requested = Vec::new();
        for target in &mut self.targets {
            // This private baseline is solely the writer result authenticated
            // above. Sibling child writes do not mutate workflow variables/state.
            let result = target
                .apply_inner(conn, key, Some((&baseline, &fields)))
                .await?;
            if result.completed_parent.is_some() {
                return Err(changed());
            }
            if result.remaining > 0 && !current.contains(&target.context.task_name) {
                current.push(target.context.task_name.clone());
            }
            if target.total() == 0 {
                target
                    .empty_outcome
                    .as_ref()
                    .ok_or_else(changed)?
                    .require_released(conn)
                    .await?;
                if !completed.contains(&target.context.task_name) {
                    completed.push(target.context.task_name.clone());
                }
                current.retain(|name| name != &target.context.task_name);
            }
            requested.extend(result.requested);
        }
        Workflows::update(
            &mut *conn,
            baseline.id,
            UpdateWorkflowExecutionInput {
                completed_tasks: Some(completed),
                failed_tasks: Some(failed),
                current_tasks: Some(current),
                ..Default::default()
            },
        )
        .await?;
        if let Some(frame) = &self.frame {
            frame.require_members_current(conn).await?;
        }
        self.ensure_fresh_on(conn).await?;
        Ok(requested)
    }
}

pub(super) async fn committed_successors(
    conn: &mut PgConnection,
    parent: Id,
    workflow: Id,
    source: &str,
) -> Result<Vec<String>> {
    let names:Vec<String>=sqlx::query_scalar("SELECT DISTINCT e.workflow_task->>'task_name' AS name FROM workflow_task_dispatch d JOIN execution e ON e.id=d.execution_id WHERE d.workflow_execution=$1 AND d.task_index IS NOT NULL AND e.parent=$2 AND e.original_execution IS NULL AND e.workflow_task->>'triggered_by'=$3 AND e.workflow_task->>'workflow_execution'=$1::TEXT AND e.workflow_task->>'task_name'=d.task_name AND (e.workflow_task->>'task_index')::INT=d.task_index ORDER BY name LIMIT 257")
        .bind(workflow).bind(parent).bind(source).fetch_all(conn).await?;
    if names.len() > 256 {
        return Err(changed());
    }
    Ok(names)
}

async fn activation_parent_fence(
    conn: &mut PgConnection,
    parent: Id,
) -> Result<ApprovalEffectFence> {
    let admission = Admissions::require(conn, RuntimeOwner::Execution(parent)).await?;
    Authority::retained_admission_fence(conn, admission.id).await
}

fn authored_ordinary_successor(parent: &Execution, source: &str) -> bool {
    let Some(tasks) = parent
        .executable_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
        .and_then(|workflow| workflow.definition.get("tasks"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    // Routing/denial only. This syntax check never selects or authorizes a task.
    tasks
        .iter()
        .filter(|task| task.get("name").and_then(Value::as_str) == Some(source))
        .filter_map(|task| task.get("next").and_then(Value::as_array))
        .flatten()
        .filter_map(|transition| transition.get("do").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .any(|name| NativeIterationMaterializationRepository::is_ordinary_task(parent, name))
}
