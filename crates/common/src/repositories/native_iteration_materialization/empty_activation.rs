//! Owning empty-source activation. No terminal execution delivery is fabricated.
use super::super::native_transition_materialization::{
    NativeTransitionMaterializationRepository as Transitions, PreparedNativeEmptyTransitions,
};
use super::activation::{committed_successors, prepare_targets, ActivationFrame};
use super::empty_ordinary::{PreparedEmptyOrdinary, RecoveredEmptyOrdinary};
use super::*;

pub struct PreparedNativeEmptyActivation {
    source: Arc<PreparedNativeIteration>,
    frame: Arc<PreparedNativeEmptyTransitions>,
    targets: Vec<PreparedNativeIteration>,
    ordinary: Vec<PreparedEmptyOrdinary>,
    applied: bool,
}
pub struct PreparedNativeEmptyRecovery {
    source: super::super::native_input_snapshot::ReleasedNativeEmptyIteration,
    workflow: WorkflowExecution,
    targets: Vec<PreparedNativeIteration>,
    ordinary: Vec<RecoveredEmptyOrdinary>,
    applied: bool,
}
impl NativeIterationMaterializationRepository {
    /// Before any original list render or transition/Key evaluation. Recovery
    /// uses the recorded empty release and actual attributed dispatches only.
    pub async fn recover_empty_activation(
        conn: &mut PgConnection,
        parent: &Execution,
        task: &str,
        key: &str,
    ) -> Result<Option<PreparedNativeEmptyRecovery>> {
        let Some(source) = super::super::native_input_snapshot::NativeInputSnapshotRepository::reopen_empty_iteration(conn, parent.id, task, key).await? else { return Ok(None); };
        let workflow = Workflows::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(changed)?;
        if workflow.id != source.workflow()
            || !workflow.completed_tasks.iter().any(|name| name == task)
        {
            return Err(changed());
        }
        let names = committed_successors(conn, parent.id, workflow.id, task).await?;
        let targets = prepare_targets(conn, parent, task, names, None, key).await?;
        let ordinary = super::empty_ordinary::recover(conn, parent, workflow.id, task, key).await?;
        let expected: BTreeSet<_> = source
            .selected_targets()
            .iter()
            .filter(|name| !Self::is_ordinary_task(parent, name))
            .cloned()
            .collect();
        let actual: BTreeSet<_> = ordinary
            .iter()
            .map(|target| {
                target
                    .row
                    .workflow_task
                    .as_ref()
                    .map(|metadata| metadata.task_name.clone())
                    .ok_or_else(changed)
            })
            .collect::<Result<_>>()?;
        if expected != actual {
            return Err(changed());
        }
        Ok(Some(PreparedNativeEmptyRecovery {
            source,
            workflow,
            targets,
            ordinary,
            applied: false,
        }))
    }
    pub async fn prepare_empty_activation(
        conn: &mut PgConnection,
        source: PreparedNativeIteration,
        key: &str,
    ) -> Result<PreparedNativeEmptyActivation> {
        source.empty_source()?.require_source_current(conn).await?;
        let source = Arc::new(source);
        let frame =
            Arc::new(Transitions::prepare_empty_successors(conn, source.clone(), key).await?);
        let owned_frame = ActivationFrame::Empty(frame.clone());
        let evidence = source.empty_source()?;
        let mut iteration_names = Vec::new();
        let mut ordinary = Vec::new();
        let mut ordinary_bytes = 0usize;
        let mut source_graph_budget =
            super::super::inherited_input::PreparedSourceGraphBudget::default();
        for name in frame.selected_targets() {
            if Self::is_ordinary_task(evidence.parent(), name) {
                iteration_names.push(name.clone());
            } else {
                let target = PreparedEmptyOrdinary::prepare(
                    conn,
                    evidence.parent(),
                    name,
                    evidence.task_name(),
                    frame.clone(),
                    key,
                )
                .await?;
                ordinary_bytes = ordinary_bytes
                    .checked_add(target.value_bytes()?)
                    .ok_or_else(changed)?;
                source_graph_budget
                    .include(target.source_graph_budget())
                    .map_err(|_| activation_budget())?;
                if ordinary_bytes > MAX_MATERIALIZED_BYTES {
                    return Err(activation_budget());
                }
                ordinary.push(target);
            }
        }
        let targets = prepare_targets(
            conn,
            evidence.parent(),
            evidence.task_name(),
            iteration_names,
            Some(&owned_frame),
            key,
        )
        .await?;
        let mut count = ordinary.len();
        let mut bytes = ordinary_bytes;
        for target in &targets {
            let contexts: Vec<_> = target
                .controls
                .iter()
                .chain(&target.receipts)
                .chain(target.empty_control.iter())
                .chain(
                    target
                        .iteration_control
                        .iter()
                        .map(|control| control.held_receipt()),
                )
                .collect();
            let destinations: Vec<_> = target
                .children
                .iter()
                .flat_map(|child| child.fields.iter().map(|field| &field.graph_budget))
                .collect();
            let target_budget =
                Inputs::prepared_input_graph_budget(conn, &destinations, &contexts, key).await?;
            source_graph_budget
                .include(&target_budget)
                .map_err(|_| activation_budget())?;
            bytes = bytes
                .checked_add(target_budget.bytes())
                .ok_or_else(changed)?;
            count = count.checked_add(target.total()).ok_or_else(changed)?;
            for child in &target.children {
                bytes = bytes
                    .checked_add(super::empty_ordinary::child_materialized_bytes(child)?)
                    .ok_or_else(changed)?;
            }
        }
        if count > MAX_CHILDREN || bytes > MAX_MATERIALIZED_BYTES {
            return Err(activation_budget());
        }
        Ok(PreparedNativeEmptyActivation {
            source,
            frame,
            targets,
            ordinary,
            applied: false,
        })
    }
}

fn activation_budget() -> Error {
    Error::PermissionDenied(
        "Ordinary empty activation materialization/source graph budget exceeded".into(),
    )
}
impl PreparedNativeEmptyRecovery {
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let mut dates = vec![self.source.held_deadline()];
        for target in &self.targets {
            dates.push(super::empty_ordinary::iteration_deadline(target)?);
        }
        dates.extend(
            self.ordinary
                .iter()
                .map(RecoveredEmptyOrdinary::held_deadline),
        );
        Ok(dates.into_iter().flatten().min())
    }
    pub fn workflow(&self) -> Id {
        self.workflow.id
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.ensure_fresh_on(conn).await?;
        for target in &self.targets {
            target.ensure_fresh_on(conn).await?;
        }
        for target in &self.ordinary {
            target.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if !self.applied {
            return Err(changed());
        }
        self.source.ensure_fresh()?;
        for target in &self.targets {
            target.ensure_fresh()?;
        }
        for target in &self.ordinary {
            target.ensure_fresh()?;
        }
        Ok(())
    }
    pub async fn apply(&mut self, conn: &mut PgConnection, key: &str) -> Result<Vec<Execution>> {
        if self.applied {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        self.applied = true;
        let actual = Workflows::find_by_id_for_update(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(changed)?;
        if serde_json::to_value(actual)? != serde_json::to_value(&self.workflow)? {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        let mut requested = Vec::new();
        for target in &mut self.targets {
            requested.extend(target.apply(conn, key).await?.requested);
        }
        for target in &self.ordinary {
            target.ensure_fresh_on(conn).await?;
            if target.row.status == ExecutionStatus::Requested {
                requested.push(target.row.clone());
            }
        }
        self.ensure_fresh_on(conn).await?;
        Ok(requested)
    }
}
impl PreparedNativeEmptyActivation {
    pub fn workflow(&self) -> Id {
        self.frame.workflow().id
    }
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let mut dates = vec![
            self.frame.held_deadline()?,
            super::empty_ordinary::iteration_deadline(&self.source)?,
        ];
        for target in &self.targets {
            dates.push(super::empty_ordinary::iteration_deadline(target)?);
        }
        for target in &self.ordinary {
            dates.push(target.held_deadline()?);
        }
        Ok(dates.into_iter().flatten().min())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        super::empty_ordinary::require_deadline_on(conn, self.held_deadline()?).await?;
        self.frame.ensure_fresh_on(conn).await?;
        for target in &self.targets {
            target.ensure_fresh_on(conn).await?;
        }
        for target in &self.ordinary {
            target.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if !self.applied {
            return Err(changed());
        }
        self.frame.ensure_fresh()?;
        for target in &self.targets {
            target.ensure_fresh()?;
        }
        for target in &self.ordinary {
            target.ensure_fresh()?;
        }
        Ok(())
    }
    pub async fn apply(&mut self, conn: &mut PgConnection, key: &str) -> Result<Vec<Execution>> {
        if self.applied {
            return Err(changed());
        }
        // Reject a foreign issuing transaction before waiting on workflow/FK
        // targets held by the original preparation transaction.
        self.ensure_fresh_on(conn).await?;
        self.applied = true;
        let evidence = self.source.empty_source()?;
        let source_outcome = self.source.empty_outcome().ok_or_else(changed)?;
        let mut baseline = Workflows::find_by_id_for_update(&mut *conn, self.workflow())
            .await?
            .ok_or_else(changed)?;
        evidence.require_source_current(conn).await?;
        source_outcome.guard_original(conn).await?;
        evidence.control().require_original_workflow(&baseline)?;
        for target in &self.targets {
            if let Some(control) = &target.iteration_control {
                control.require_original_workflow(&baseline)?;
            }
            if let Some(empty) = &target.empty_outcome {
                empty.guard_original(conn).await?;
            }
        }
        self.ensure_fresh_on(conn).await?;
        // This Control is rooted in the original empty input, before our own
        // next publications. Release it before recording any dependent receipt.
        evidence.control().finalize(conn).await?;
        source_outcome.release(conn, &evidence).await?;
        source_outcome.require_released(conn).await?;
        let recorded = self.frame.record(conn, &baseline, key).await?;
        Secrets::replace_entity_with_conn(
            conn,
            ENTITY_WORKFLOW_VARIABLES,
            evidence.parent().id,
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
        self.frame.require_published(conn).await?;
        for target in &self.targets {
            if let Some(control) = &target.iteration_control {
                control.finalize(conn).await?;
            }
        }
        let fields = Secrets::find_stored_by_entity(
            &mut *conn,
            ENTITY_WORKFLOW_VARIABLES,
            evidence.parent().id,
        )
        .await?;
        let mut completed = baseline.completed_tasks.clone();
        if !completed.iter().any(|name| name == evidence.task_name()) {
            completed.push(evidence.task_name().into());
        }
        let mut current = baseline.current_tasks.clone();
        current.retain(|name| name != evidence.task_name());
        let mut requested = Vec::new();
        for target in &mut self.targets {
            let result = target
                .apply_inner(conn, key, Some((&baseline, &fields)))
                .await?;
            if result.completed_parent.is_some() {
                return Err(changed());
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
            } else if !current.contains(&target.context.task_name) {
                current.push(target.context.task_name.clone());
            }
            requested.extend(result.requested);
        }
        for target in &mut self.ordinary {
            if !current.iter().any(|name| name == target.name()) {
                current.push(target.name().to_owned());
            }
            requested.push(target.apply(conn, key).await?);
        }
        Workflows::update(
            &mut *conn,
            baseline.id,
            UpdateWorkflowExecutionInput {
                completed_tasks: Some(completed),
                current_tasks: Some(current),
                ..Default::default()
            },
        )
        .await?;
        source_outcome.require_released(conn).await?;
        self.frame.require_members_current(conn).await?;
        self.ensure_fresh_on(conn).await?;
        Ok(requested)
    }
}
