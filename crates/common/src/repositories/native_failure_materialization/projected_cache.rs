//! Finite root projection over B's genuine cache completion capability.
//! This is source state, not an aggregate VALUE, output receipt or write permit.
use super::*;
use crate::repositories::native_cache_materialization::SourceCacheCompletion;

/// Borrows the complete B source, including zero-child EOF and independent
/// controls. A must archive its finite recipe before this borrow is released.
pub struct PreparedProjectedCacheCompletion<'a> {
    source: &'a SourceCacheCompletion,
    projection: ProjectedCacheCompletionState,
    schema_secret_paths: Vec<String>,
}

/// Pure target comparisons retained after the source borrow ends so B can mutate
/// its checkpoint. This state alone supplies no receipt or freshness authority.
pub struct ProjectedCacheCompletionState {
    root: Id,
    workflow: Id,
    definition: Id,
    task: String,
    failed_tasks: Vec<String>,
    completed_tasks: Vec<String>,
    metadata: Value,
}

impl NativeFailureMaterializationRepository {
    /// Before outer targets. IDs must match the owning B capability, never a
    /// caller summary. The first finite scope is one authored cache task with no
    /// recovery/successor, skipped tasks, waits or ordinary iteration siblings.
    pub async fn prepare_projected_cache_completion<'a>(
        conn: &mut PgConnection,
        root: Id,
        workflow: Id,
        source: &'a SourceCacheCompletion,
    ) -> Result<Option<PreparedProjectedCacheCompletion<'a>>> {
        if source.execution().id != root
            || source.workflow().id != workflow
            || source.workflow().execution != root
        {
            return Err(changed());
        }
        source.require_current(conn).await?;
        let parent = source.execution();
        let state = source.workflow();
        if parent.status != ExecutionStatus::Running || state.status != ExecutionStatus::Running {
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
        if definition.id != state.workflow_def {
            return Err(changed());
        }
        if !single_cache_root(&definition.definition, source.task_name())? {
            return Ok(None);
        }
        require_root_inventory(conn, source).await?;
        if !source.eof().exhausted() || source.eof().count() != 0 {
            return Err(changed());
        }
        let contract = original_contract(conn, parent).await?;
        if &contract != source.contract() {
            return Err(changed());
        }
        let Some(projection) = project_task_state(
            state,
            source.task_name(),
            source.has_failed_latest_attempt(),
        )?
        else {
            return Ok(None);
        };
        let schema_secret_paths =
            contract
                .output
                .as_ref()
                .map(|contract| {
                    let compiled = CompiledContract::new(contract).map_err(|_| changed())?;
                    Ok::<_, Error>(compiled.secret_paths(&projection.metadata).unwrap_or_else(
                        |_| {
                            if compiled.declares_secrets() {
                                vec![String::new()]
                            } else {
                                Vec::new()
                            }
                        },
                    ))
                })
                .transpose()?
                .unwrap_or_default();
        let prepared = PreparedProjectedCacheCompletion {
            source,
            projection,
            schema_secret_paths,
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(Some(prepared))
    }
}

impl<'a> PreparedProjectedCacheCompletion<'a> {
    /// Full private B capability, not an arbitrary selected child or aggregate JSON.
    pub fn source(&self) -> &'a SourceCacheCompletion {
        self.source
    }
    pub fn projection(&self) -> &ProjectedCacheCompletionState {
        &self.projection
    }
    pub fn schema_secret_paths(&self) -> &[String] {
        &self.schema_secret_paths
    }
    pub async fn require_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.require_current(conn).await?;
        require_root_inventory(conn, self.source).await
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.source.ensure_fresh()
    }
    /// Call only after A has consumed the genuine borrowed source for its own
    /// recipe. B retains the source and its fences while advancing the checkpoint.
    /// The returned comparison state cannot replace either owner's capability.
    pub fn into_projection(self) -> ProjectedCacheCompletionState {
        self.projection
    }
}

impl ProjectedCacheCompletionState {
    pub fn execution(&self) -> Id {
        self.root
    }
    pub fn workflow(&self) -> Id {
        self.workflow
    }
    pub fn task_name(&self) -> &str {
        &self.task
    }
    pub fn failed_tasks(&self) -> &[String] {
        &self.failed_tasks
    }
    pub fn completed_tasks(&self) -> &[String] {
        &self.completed_tasks
    }
    pub fn succeeded(&self) -> bool {
        self.failed_tasks.is_empty()
    }
    /// Canonical finite status bytes only, NOT VALUE/provenance authority.
    /// Success output composition and validation warnings remain separate recipes.
    pub fn metadata(&self) -> &Value {
        &self.metadata
    }
    pub fn require_metadata(&self, actual: &Value) -> Result<()> {
        if actual != &self.metadata {
            return Err(changed());
        }
        Ok(())
    }
    /// B owns progression. Compare its real resulting task state before A's
    /// recipe records output. Do not accept this state as evidence of frontier
    /// binding: B's actual terminal capability must independently be checked.
    pub fn require_projected_task_state(&self, actual: &WorkflowExecution) -> Result<()> {
        if actual.id != self.workflow
            || actual.execution != self.root
            || actual.workflow_def != self.definition
            || actual.status
                != if self.succeeded() {
                    ExecutionStatus::Completed
                } else {
                    ExecutionStatus::Failed
                }
            || actual.failed_tasks != self.failed_tasks
            || actual.completed_tasks != self.completed_tasks
            || !actual.current_tasks.is_empty()
            || !actual.skipped_tasks.is_empty()
        {
            return Err(changed());
        }
        Ok(())
    }
}

async fn require_root_inventory(
    conn: &mut PgConnection,
    source: &SourceCacheCompletion,
) -> Result<()> {
    let children = ExecutionRepository::find_by_parent(&mut *conn, source.execution().id).await?;
    if children.len() != source.attempts().len() || children.len() >= MAX_CHILDREN {
        return Err(changed());
    }
    for child in children {
        let held = source
            .attempts()
            .iter()
            .find(|attempt| attempt.execution().id == child.id)
            .ok_or_else(changed)?;
        if serde_json::to_value(&child)? != serde_json::to_value(held.execution())? {
            return Err(changed());
        }
    }
    Ok(())
}

fn single_cache_root(graph: &Value, task: &str) -> Result<bool> {
    bounded(graph)?;
    let Some(tasks) = graph.get("tasks").and_then(Value::as_array) else {
        return Err(changed());
    };
    if tasks.len() != 1 {
        return Ok(false);
    }
    let node = &tasks[0];
    if node.get("name").and_then(Value::as_str) != Some(task)
        || node.get("iterate_cache").is_none_or(Value::is_null)
        || ["with_items", "wait_for"]
            .iter()
            .any(|key| node.get(*key).is_some_and(|value| !value.is_null()))
    {
        return Ok(false);
    }
    if let Some(next) = node.get("next").and_then(Value::as_array) {
        if next.iter().any(|edge| {
            edge.get("do").is_some_and(|value| {
                !value.is_null() && value.as_array().is_none_or(|tasks| !tasks.is_empty())
            })
        }) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn project_task_state(
    workflow: &WorkflowExecution,
    task: &str,
    failed: bool,
) -> Result<Option<ProjectedCacheCompletionState>> {
    if !workflow.completed_tasks.is_empty() || !workflow.skipped_tasks.is_empty() {
        return Ok(None);
    }
    if workflow.current_tasks.len() > 1 || workflow.current_tasks.iter().any(|name| name != task) {
        return Err(changed());
    }
    // No caller failed-name array enters the owning factory. An already recorded
    // cache failure must be backed by this exact authenticated aggregate failure.
    if workflow.failed_tasks.len() > 1
        || workflow
            .failed_tasks
            .iter()
            .any(|name| name != task || !failed)
    {
        return Err(changed());
    }
    let mut failed_tasks = workflow.failed_tasks.clone();
    if failed && failed_tasks.is_empty() {
        failed_tasks.push(task.into());
    }
    let completed_tasks = if failed {
        Vec::new()
    } else {
        vec![task.into()]
    };
    let metadata = if failed {
        failure_metadata(&failed_tasks)
    } else {
        serde_json::json!({"succeeded":true})
    };
    bounded(&metadata)?;
    Ok(Some(ProjectedCacheCompletionState {
        root: workflow.execution,
        workflow: workflow.id,
        definition: workflow.workflow_def,
        task: task.into(),
        failed_tasks,
        completed_tasks,
        metadata,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_root_scope_does_not_widen_the_ordinary_factory() {
        let graph = serde_json::json!({"tasks":[{"name":"cache","iterate_cache":{"namespace":"original"}}]});
        assert!(single_cache_root(&graph, "cache").unwrap());
        assert!(!ordinary_terminal_graph(&graph).unwrap());
        assert!(!single_cache_root(&graph, "foreign").unwrap());
        for field in ["with_items", "wait_for"] {
            let mut other = graph.clone();
            other["tasks"][0][field] = serde_json::json!({});
            assert!(!single_cache_root(&other, "cache").unwrap());
        }
        let mut successor = graph.clone();
        successor["tasks"][0]["next"] = serde_json::json!([{"do":["later"]}]);
        assert!(!single_cache_root(&successor, "cache").unwrap());
        let mut sibling = graph.clone();
        sibling["tasks"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"name":"other"}));
        assert!(!single_cache_root(&sibling, "cache").unwrap());
    }
}
