//! Authenticated ordinary success state. A owns composition, generated status,
//! validation diagnostics and the finite receipt; this module issues none of them.
use super::*;
use crate::{
    repositories::{
        inherited_input::PreparedInputSource,
        native_transition_materialization::PreparedNativeTransitions,
    },
    secret_values::{ENTITY_EXECUTION_CONFIG, ENTITY_WORKFLOW_VARIABLES},
};

/// Issued only from persisted IDs plus B's genuine source plan. No Clone,
/// Deserialize, supplied payload, warning list, success flag or public setter.
pub struct PreparedNativeSuccess<'a> {
    transaction: String,
    completed: Id,
    parent: Execution,
    workflow: WorkflowExecution,
    children: Vec<OriginalChild>,
    contract: ActionSchemaBinding,
    config_fields: Vec<StoredSecretValue>,
    result_fields: Vec<StoredSecretValue>,
    variable_fields: Vec<StoredSecretValue>,
    projected_completed: Vec<String>,
    plan: &'a PreparedNativeTransitions,
    fences: Vec<ApprovalEffectFence>,
}

impl NativeFailureMaterializationRepository {
    /// Source phase, before any target effects. A receives this token and B's
    /// opaque leaves/decisions, never a caller's representation of those facts.
    pub async fn prepare_native_success<'a>(
        conn: &mut PgConnection,
        root: Id,
        workflow_id: Id,
        completed: Id,
        plan: &'a PreparedNativeTransitions,
    ) -> Result<Option<PreparedNativeSuccess<'a>>> {
        plan.require_source_phase()?;
        let parent = ExecutionRepository::find_by_id(&mut *conn, root)
            .await?
            .ok_or_else(changed)?;
        let workflow = Workflows::find_by_id(&mut *conn, workflow_id)
            .await?
            .ok_or_else(changed)?;
        let current = ExecutionRepository::find_by_id(&mut *conn, completed)
            .await?
            .ok_or_else(changed)?;
        if workflow.execution != root
            || plan.execution() != root
            || serde_json::to_value(plan.original_parent())? != serde_json::to_value(&parent)?
            || serde_json::to_value(plan.workflow())? != serde_json::to_value(&workflow)?
            || plan
                .completion_outcome()
                .is_none_or(|outcome| outcome.execution() != completed)
        {
            return Err(changed());
        }
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if transaction != plan.source_transaction() {
            return Err(changed());
        }
        if parent.status != ExecutionStatus::Running
            || workflow.status != ExecutionStatus::Running
            || current.status != ExecutionStatus::Completed
        {
            return Ok(None);
        }
        let definition = parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(changed)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?;
        if definition.id != workflow.workflow_def {
            return Err(changed());
        }
        let Some(names) = success_task_names(&definition.definition)? else {
            return Ok(None);
        };
        let Some(task) = current.workflow_task.as_ref() else {
            return Err(changed());
        };
        if current.parent != Some(root)
            || task.workflow_execution != workflow_id
            || plan.completion_outcome().is_none_or(|outcome| {
                outcome.task_name() != task.task_name
                    || outcome.value("succeeded") != Some(Value::Bool(true))
            })
        {
            return Err(changed());
        }
        let contract = original_contract(conn, &parent).await?;
        require_output_plan(&definition.definition, plan, &contract)?;
        let config_fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_CONFIG, root).await?;
        let result_fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, root).await?;
        let variable_fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, root).await?;
        if variable_fields.as_slice() != plan.original_variable_fields() {
            return Err(changed());
        }
        let admission = Authority::execution_admission(conn, root)
            .await?
            .ok_or_else(changed)?;
        let mut fences = vec![Authority::retained_admission_fence(conn, admission.id).await?];
        let rows = ExecutionRepository::find_by_parent(&mut *conn, root).await?;
        if rows.len() >= MAX_CHILDREN {
            return Err(changed());
        }
        plan.require_original_child_rows(&rows)?;
        let mut observed = BTreeSet::new();
        let mut children = Vec::new();
        let mut bytes = 0;
        add_held_bytes(&mut bytes, &parent)?;
        add_held_bytes(&mut bytes, &workflow)?;
        add_held_bytes(&mut bytes, &contract)?;
        for fields in [&config_fields, &result_fields, &variable_fields] {
            account_fields(&mut bytes, fields)?;
        }
        if let Some(output) = plan.output() {
            add_held_bytes(&mut bytes, output)?;
        }
        for row in rows {
            let metadata = row.workflow_task.as_ref().ok_or_else(changed)?;
            if row.parent != Some(root)
                || metadata.workflow_execution != workflow_id
                || !names.contains(&metadata.task_name)
            {
                return Err(changed());
            }
            if row.status != ExecutionStatus::Completed
                || metadata.task_index.is_some()
                || metadata.task_batch.is_some()
                || metadata.retry_count != 0
                || row.retry_count != 0
                || metadata.timed_out
                || !observed.insert(metadata.task_name.clone())
            {
                return Ok(None);
            }
            let outcome = Inputs::prepare_outcome(conn, root, row.id).await?;
            if outcome.execution() != row.id
                || outcome.task_name() != metadata.task_name
                || outcome.value("succeeded") != Some(Value::Bool(true))
            {
                return Err(changed());
            }
            let authored_path = super::super::declared_input_materialization::DeclaredInputMaterializationRepository::workflow_task_site_path(
                conn, root, &metadata.task_name, &row.action_ref,
            ).await?;
            let source = Authority::execution_admission(conn, row.id)
                .await?
                .ok_or_else(changed)?;
            fences.push(Authority::retained_admission_fence(conn, source.id).await?);
            let child_contract = original_contract(conn, &row).await?;
            let fields =
                Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, row.id).await?;
            let after = ExecutionRepository::find_by_id(&mut *conn, row.id)
                .await?
                .ok_or_else(changed)?;
            if serde_json::to_value(&after)? != serde_json::to_value(&row)? {
                return Err(changed());
            }
            add_held_bytes(&mut bytes, &row)?;
            add_held_bytes(&mut bytes, &child_contract)?;
            account_fields(&mut bytes, &fields)?;
            children.push(OriginalChild {
                execution: row,
                authored_path,
                contract: child_contract,
                result_fields: fields,
                outcome,
            });
        }
        if observed != names {
            return Ok(None);
        }
        let Some(projected_completed) = project_success_tasks(&workflow, &names, &task.task_name)?
        else {
            return Ok(None);
        };
        let prepared = PreparedNativeSuccess {
            transaction,
            completed,
            parent,
            workflow,
            children,
            contract,
            config_fields,
            result_fields,
            variable_fields,
            projected_completed,
            plan,
            fences,
        };
        prepared.require_current_on(conn).await?;
        Ok(Some(prepared))
    }
}

impl<'a> PreparedNativeSuccess<'a> {
    pub fn execution(&self) -> Id {
        self.parent.id
    }
    pub fn completed_execution(&self) -> Id {
        self.completed
    }
    pub fn parent(&self) -> &Execution {
        &self.parent
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        &self.workflow
    }
    pub fn output_contract(&self) -> &ActionSchemaBinding {
        &self.contract
    }
    pub fn output_plan(&self) -> &'a PreparedNativeTransitions {
        self.plan
    }
    pub fn decisions(&self) -> Vec<&PreparedInputSource> {
        self.plan.completion_decisions()
    }
    pub fn outcomes(&self) -> impl Iterator<Item = &PreparedWorkflowOutcome> {
        self.children.iter().map(|child| &child.outcome)
    }
    pub fn source_executions(&self) -> impl Iterator<Item = &Execution> {
        self.children.iter().map(|child| &child.execution)
    }
    pub fn root_config_fields(&self) -> &[StoredSecretValue] {
        &self.config_fields
    }
    pub fn root_result_fields(&self) -> &[StoredSecretValue] {
        &self.result_fields
    }
    pub fn variable_fields(&self) -> &[StoredSecretValue] {
        &self.variable_fields
    }
    pub fn original_contract(&self, child: Id) -> Result<&ActionSchemaBinding> {
        Ok(&self.child(child)?.contract)
    }
    pub fn original_result_fields(&self, child: Id) -> Result<&[StoredSecretValue]> {
        Ok(&self.child(child)?.result_fields)
    }
    pub fn original_task_path(&self, child: Id) -> Result<&str> {
        Ok(&self.child(child)?.authored_path)
    }
    pub fn projected_status(&self) -> ExecutionStatus {
        ExecutionStatus::Completed
    }
    pub fn projected_completed_tasks(&self) -> &[String] {
        &self.projected_completed
    }
    pub fn projected_failed_tasks(&self) -> &[String] {
        &[]
    }
    pub fn projected_current_tasks(&self) -> &[String] {
        &[]
    }
    pub fn projected_skipped_tasks(&self) -> &[String] {
        &[]
    }
    fn child(&self, id: Id) -> Result<&OriginalChild> {
        self.children
            .iter()
            .find(|child| child.execution.id == id)
            .ok_or_else(changed)
    }
    pub fn require_original_rows(
        &self,
        parent: &Execution,
        workflow: &WorkflowExecution,
    ) -> Result<()> {
        if serde_json::to_value(parent)? != serde_json::to_value(&self.parent)?
            || serde_json::to_value(workflow)? != serde_json::to_value(&self.workflow)?
        {
            return Err(changed());
        }
        Ok(())
    }
    /// Equality only. The actual all_done writer has not changed root status yet.
    pub fn require_projected_workflow(&self, actual: &WorkflowExecution) -> Result<()> {
        if actual.id != self.workflow.id
            || actual.execution != self.parent.id
            || actual.workflow_def != self.workflow.workflow_def
            || actual.task_graph != self.workflow.task_graph
            || !matches!(
                actual.status,
                ExecutionStatus::Running | ExecutionStatus::Completed
            )
            || actual.completed_tasks != self.projected_completed
            || !actual.failed_tasks.is_empty()
            || !actual.current_tasks.is_empty()
            || !actual.skipped_tasks.is_empty()
        {
            return Err(changed());
        }
        Ok(())
    }
    pub fn require_original_child(
        &self,
        actual: &Execution,
        contract: &ActionSchemaBinding,
        fields: &[StoredSecretValue],
    ) -> Result<()> {
        let child = self.child(actual.id)?;
        if serde_json::to_value(actual)? != serde_json::to_value(&child.execution)?
            || contract != &child.contract
            || fields != child.result_fields.as_slice()
        {
            return Err(changed());
        }
        Ok(())
    }
    /// Initial source-buffer comparison, before targets. This grants no write.
    pub async fn require_current_on(&self, conn: &mut PgConnection) -> Result<()> {
        let parent = ExecutionRepository::find_by_id(&mut *conn, self.parent.id)
            .await?
            .ok_or_else(changed)?;
        let workflow = Workflows::find_by_id(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(changed)?;
        self.require_original_rows(&parent, &workflow)?;
        for (entity, expected) in [
            (ENTITY_EXECUTION_CONFIG, &self.config_fields),
            (ENTITY_EXECUTION_RESULT, &self.result_fields),
            (ENTITY_WORKFLOW_VARIABLES, &self.variable_fields),
        ] {
            if Secrets::find_stored_by_entity(&mut *conn, entity, self.parent.id).await?
                != *expected
            {
                return Err(changed());
            }
        }
        self.require_original_sources_on(conn).await
    }
    /// Source/contract comparisons only; safe after B advances workflow buffers.
    /// A must separately compare its actual output target and projected state.
    pub async fn require_original_sources_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        self.plan.require_original_result_view_on(conn).await?;
        let rows = ExecutionRepository::find_by_parent(&mut *conn, self.parent.id).await?;
        self.plan.require_original_child_rows(&rows)?;
        if rows.len() != self.children.len()
            || original_contract(conn, &self.parent).await? != self.contract
        {
            return Err(changed());
        }
        for row in rows {
            let contract = original_contract(conn, &row).await?;
            let fields =
                Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, row.id).await?;
            self.require_original_child(&row, &contract, &fields)?;
        }
        Ok(())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        let current: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if current != self.transaction {
            return Err(changed());
        }
        self.plan.ensure_fresh_on(conn).await?;
        for fence in &self.fences {
            fence.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.plan.ensure_fresh()?;
        for fence in &self.fences {
            fence.ensure_fresh()?;
        }
        Ok(())
    }
}

fn success_task_names(graph: &Value) -> Result<Option<BTreeSet<String>>> {
    bounded(graph)?;
    if !ordinary_terminal_graph(graph)? {
        return Ok(None);
    }
    let tasks = graph
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(changed)?;
    if tasks.is_empty() || tasks.len() >= MAX_CHILDREN {
        return Ok(None);
    }
    let mut names = BTreeSet::new();
    for task in tasks {
        let name = task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(changed)?;
        if name.is_empty() || !names.insert(name.to_owned()) {
            return Err(changed());
        }
        if CanonicalCompletionTask::prepare(task)?.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(names))
}
fn project_success_tasks(
    workflow: &WorkflowExecution,
    names: &BTreeSet<String>,
    current: &str,
) -> Result<Option<Vec<String>>> {
    if !workflow.failed_tasks.is_empty() || !workflow.skipped_tasks.is_empty() {
        return Ok(None);
    }
    let mut complete = BTreeSet::new();
    for name in &workflow.completed_tasks {
        if !names.contains(name) || !complete.insert(name.clone()) {
            return Err(changed());
        }
    }
    let mut active = BTreeSet::new();
    for name in &workflow.current_tasks {
        if !names.contains(name) || !active.insert(name.clone()) {
            return Err(changed());
        }
    }
    if !names.contains(current) {
        return Err(changed());
    }
    if complete.contains(current)
        || active.iter().any(|name| name != current)
        || names
            .iter()
            .any(|name| name != current && !complete.contains(name))
    {
        return Ok(None);
    }
    let mut projected = workflow.completed_tasks.clone();
    projected.push(current.to_owned());
    Ok(Some(projected))
}
fn require_output_plan(
    graph: &Value,
    plan: &PreparedNativeTransitions,
    contract: &ActionSchemaBinding,
) -> Result<()> {
    let authored = graph.get("output_map").filter(|value| !value.is_null());
    let authored = authored
        .map(|value| value.as_object().ok_or_else(changed))
        .transpose()?;
    let mut expected = Vec::new();
    if let Some(authored) = authored {
        for (name, value) in authored {
            super::super::native_workflow_materialization::collect_leaves(
                value,
                format!(
                    "/{}",
                    super::super::native_workflow_materialization::escape(name)
                ),
                &mut expected,
                0,
            )?;
        }
    }
    let actual = plan.prepared_output_leaves().collect::<Vec<_>>();
    if expected.is_empty() {
        if plan.output().is_some() || !actual.is_empty() {
            return Err(changed());
        }
        return Ok(());
    }
    let output = plan.output().ok_or_else(changed)?;
    if plan.prepared_output_contract() != contract.output.as_ref() || expected.len() != actual.len()
    {
        return Err(changed());
    }
    let paths = expected
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<BTreeSet<_>>();
    let actual_paths = actual
        .iter()
        .map(|(path, _)| *path)
        .collect::<BTreeSet<_>>();
    if paths != actual_paths || actual_paths.len() != actual.len() {
        return Err(changed());
    }
    for (path, rendered) in actual {
        if output.pointer(path) != Some(rendered.value()) {
            return Err(changed());
        }
    }
    Ok(())
}
fn account_fields(bytes: &mut usize, fields: &[StoredSecretValue]) -> Result<()> {
    for field in fields {
        add_held_bytes(
            bytes,
            &(
                &field.json_path,
                &field.encrypted_value,
                &field.encryption_key_hash,
                &field.source_kind,
                &field.source_ref,
            ),
        )?;
    }
    Ok(())
}
