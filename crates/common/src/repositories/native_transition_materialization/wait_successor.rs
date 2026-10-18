//! Source-first bridge from an actual ordinary completion to its selected Wait.
//! No synthetic execution, caller-supplied projection or second condition render.
use super::*;
use std::sync::Arc;

pub struct PreparedNativeWaitPredecessor {
    transitions: Arc<PreparedNativeTransitions>,
    completed: Execution,
    projected: WorkflowExecution,
    target: String,
    target_path: String,
    published: std::sync::OnceLock<()>,
}

impl NativeTransitionMaterializationRepository {
    pub async fn prepare_wait_successor(
        conn: &mut PgConnection,
        completed: &Execution,
        key: &str,
    ) -> Result<Option<Arc<PreparedNativeWaitPredecessor>>> {
        let Some(task) = completed.workflow_task.as_ref() else {
            return Ok(None);
        };
        if completed.id <= 0
            || task.task_index.is_some()
            || task.retry_count != 0
            || !matches!(
                completed.status,
                ExecutionStatus::Completed
                    | ExecutionStatus::Failed
                    | ExecutionStatus::Timeout
                    | ExecutionStatus::Cancelled
                    | ExecutionStatus::Abandoned
            )
        {
            return Ok(None);
        }
        let parent =
            ExecutionRepository::find_by_id(&mut *conn, completed.parent.ok_or_else(changed)?)
                .await?
                .ok_or_else(changed)?;
        let graph = &parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(changed)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        let tasks = graph
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(changed)?;
        if graph
            .get("output_map")
            .and_then(Value::as_object)
            .is_some_and(|fields| !fields.is_empty())
        {
            return Ok(None);
        }
        let Some(node) = tasks
            .iter()
            .find(|node| node.get("name").and_then(Value::as_str) == Some(&task.task_name))
        else {
            return Err(changed());
        };
        let Some(next) = node.get("next").and_then(Value::as_array) else {
            return Ok(None);
        };
        if next.len() != 1 {
            return Ok(None);
        }
        let eligible_condition = match next[0].get("when") {
            None | Some(Value::Null) => true,
            Some(Value::String(expression)) => match expression
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect::<String>()
                .as_str()
            {
                "{{succeeded()}}" => {
                    completed.status == ExecutionStatus::Completed && !task.timed_out
                }
                "{{failed()}}" => {
                    !task.timed_out
                        && matches!(
                            completed.status,
                            ExecutionStatus::Failed | ExecutionStatus::Abandoned
                        )
                }
                "{{timed_out()}}" => completed.status == ExecutionStatus::Timeout || task.timed_out,
                "{{cancelled()}}" => completed.status == ExecutionStatus::Cancelled,
                _ => false,
            },
            _ => false,
        };
        if !eligible_condition {
            return Ok(None);
        }
        // This bridge owns a serial Wait handoff. Other transition stages retain
        // their own factories; do not prepare/evaluate them speculatively here.
        let mut authored_targets = BTreeSet::new();
        for transition in next {
            if transition.get("publish").is_some_and(|value| {
                !value.is_null() && !value.as_array().is_some_and(Vec::is_empty)
            }) {
                return Ok(None);
            }
            for target in transition
                .get("do")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = target.as_str().ok_or_else(changed)?;
                let mut found = tasks
                    .iter()
                    .filter(|node| node.get("name").and_then(Value::as_str) == Some(name));
                let target = found.next().ok_or_else(changed)?;
                if found.next().is_some()
                    || target.get("wait_for").is_none_or(Value::is_null)
                    || ["with_items", "iterate_cache", "join", "retry"]
                        .iter()
                        .any(|field| target.get(field).is_some_and(|value| !value.is_null()))
                {
                    return Ok(None);
                }
                let Some(wait_for) = target.get("wait_for").and_then(Value::as_object) else {
                    return Ok(None);
                };
                if wait_for.len() != 1
                    || !wait_for
                        .values()
                        .all(|value| value.as_str().is_some_and(|value| value.contains("{{")))
                {
                    return Ok(None);
                }
                authored_targets.insert(name.to_owned());
            }
        }
        if authored_targets.len() != 1 {
            return Ok(None);
        }
        let workflow = WorkflowExecutionRepository::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(changed)?;
        if workflow.completed_tasks.contains(&task.task_name)
            || workflow.failed_tasks.contains(&task.task_name)
        {
            return Ok(None);
        }
        if workflow.current_tasks != vec![task.task_name.clone()]
            || !workflow.failed_tasks.is_empty()
        {
            return Ok(None);
        }
        // Match the scheduler's once-only task semantics before evaluating a
        // Condition. A previously completed target is not a new Wait handoff.
        if workflow
            .completed_tasks
            .iter()
            .any(|name| authored_targets.contains(name))
        {
            return Ok(None);
        }
        let Some(plan) = Self::prepare_completion(conn, completed, key, true).await? else {
            return Ok(None);
        };
        sqlx::query_scalar::<_, Id>("SELECT id FROM execution WHERE id=$1 FOR SHARE")
            .bind(completed.id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(changed)?;
        plan.require_current(&workflow)?;
        let mut selected = BTreeSet::new();
        for (index, transition) in next.iter().enumerate() {
            if plan.fires(index)? {
                for name in transition
                    .get("do")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    selected.insert(name.as_str().ok_or_else(changed)?.to_owned());
                }
            }
        }
        // Once Conditions were evaluated, never fall through and render again.
        if selected.len() != 1 {
            return Err(changed());
        }
        let target = selected.into_iter().next().ok_or_else(changed)?;
        let target_node = tasks
            .iter()
            .find(|node| node.get("name").and_then(Value::as_str) == Some(&target))
            .ok_or_else(changed)?;
        let target_path = format!(
            "/workflow{}",
            super::super::declared_input_materialization::pointer_to_node(graph, target_node)?
        );
        let actual = ExecutionRepository::find_by_id(&mut *conn, completed.id)
            .await?
            .ok_or_else(changed)?;
        if serde_json::to_value(&actual)? != serde_json::to_value(completed)? {
            return Err(changed());
        }
        let mut projected = workflow;
        projected.current_tasks = vec![target.clone()];
        projected.completed_tasks.push(task.task_name.clone());
        projected.updated = sqlx::query_scalar("SELECT transaction_timestamp()")
            .fetch_one(&mut *conn)
            .await?;
        let issued = Arc::new(PreparedNativeWaitPredecessor {
            transitions: Arc::new(plan),
            completed: completed.clone(),
            projected,
            target,
            target_path,
            published: std::sync::OnceLock::new(),
        });
        issued.ensure_fresh_on(conn).await.map_err(|error| {
            source_phase_error(
                error,
                "prepare Wait predecessor held closure",
                &issued.target_path,
            )
        })?;
        Ok(Some(issued))
    }
}

impl PreparedNativeWaitPredecessor {
    pub(in crate::repositories) fn held_deadline(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let sources = &self.transitions.cache_sources;
        // A result view may accompany the real predecessor even when this
        // handoff only selects status. Retain its independently held fence.
        // Groups, root outputs and publication stages remain separate owners.
        if sources.iteration.is_some()
            || sources.empty_iteration.is_some()
            || self.transitions.output.is_some()
            || self
                .transitions
                .transitions
                .iter()
                .any(|transition| !transition.publications.is_empty())
        {
            return Err(changed());
        }
        Ok(sources
            .fence
            .approval_deadline()
            .into_iter()
            .chain(sources.system.held_effect_fence().approval_deadline())
            .chain(
                sources
                    .fields
                    .iter()
                    .filter_map(|field| field.source.as_ref())
                    .filter_map(PreparedInputSource::approval_deadline),
            )
            .chain(
                sources
                    .outcome
                    .iter()
                    .filter_map(PreparedWorkflowOutcome::approval_deadline),
            )
            .chain(
                sources
                    .native_result
                    .iter()
                    .filter_map(PreparedNativeResult::approval_deadline),
            )
            .chain(
                self.transitions
                    .completion_decisions()
                    .into_iter()
                    .filter_map(PreparedInputSource::approval_deadline),
            )
            .min())
    }
    pub(in crate::repositories) fn require_source_workflow(
        &self,
        actual: &WorkflowExecution,
    ) -> Result<()> {
        let expected = if self.published.get().is_some() {
            self.projected()
        } else {
            self.original()
        };
        if serde_json::to_value(actual)? != serde_json::to_value(expected)? {
            return Err(changed());
        }
        Ok(())
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn original(&self) -> &WorkflowExecution {
        self.transitions.workflow()
    }
    pub fn projected(&self) -> &WorkflowExecution {
        &self.projected
    }
    pub fn parent(&self) -> &Execution {
        self.transitions.original_parent()
    }
    pub fn completed(&self) -> &Execution {
        &self.completed
    }
    pub(in crate::repositories) fn attach_rendered_control(
        &self,
        rendered: &mut RenderedReceiptContext,
    ) -> Result<()> {
        self.transitions.attach_frame_literal(rendered)?;
        Inputs::attach_workflow_outcome_control(
            rendered,
            self.transitions
                .cache_sources
                .outcome
                .as_ref()
                .ok_or_else(changed)?,
        )
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.held_deadline()?;
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if transaction != self.transitions.source_transaction() {
            return Err(changed());
        }
        self.transitions.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.held_deadline()?;
        self.transitions.ensure_fresh()
    }
    pub async fn publish(&self, conn: &mut PgConnection, key: &str) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        if self.published.get().is_some() {
            return Err(changed());
        }
        let actual =
            WorkflowExecutionRepository::find_by_id_for_update(&mut *conn, self.original().id)
                .await?
                .ok_or_else(changed)?;
        self.transitions.require_current(&actual)?;
        let child = ExecutionRepository::find_by_id(&mut *conn, self.completed.id)
            .await?
            .ok_or_else(changed)?;
        let parent = ExecutionRepository::find_by_id(&mut *conn, self.parent().id)
            .await?
            .ok_or_else(changed)?;
        if serde_json::to_value(child)? != serde_json::to_value(&self.completed)?
            || serde_json::to_value(parent)? != serde_json::to_value(self.parent())?
        {
            return Err(changed());
        }
        let recorded = self.transitions.record(conn, &actual, key).await?;
        if !recorded.publications.is_empty() || recorded.variables != actual.variables {
            return Err(changed());
        }
        let updated = WorkflowExecutionRepository::update(
            &mut *conn,
            actual.id,
            super::super::workflow::UpdateWorkflowExecutionInput {
                current_tasks: Some(self.projected.current_tasks.clone()),
                completed_tasks: Some(self.projected.completed_tasks.clone()),
                ..Default::default()
            },
        )
        .await?;
        if serde_json::to_value(updated)? != serde_json::to_value(&self.projected)? {
            return Err(changed());
        }
        self.published.set(()).map_err(|_| changed())?;
        self.ensure_fresh_on(conn).await
    }
}
