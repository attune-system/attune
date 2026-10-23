//! Shared stage binding, not a list/cache evaluator or a caller JSON proof.
use super::*;

// Public in this private module only. Owning factories issue genuine frames;
// API callers cannot implement this sealed interface to manufacture one.
pub trait NativeTransitionFrameSeal {}

#[async_trait::async_trait]
pub trait NativeTransitionRenderFrame: NativeTransitionFrameSeal + Send + Sync {
    fn execution(&self) -> Id;
    fn selected_targets(&self) -> &[String];
    async fn bind(
        &self,
        conn: &mut PgConnection,
        session: &mut RenderSession,
        actual: &DeclaredInputBinding,
        key: &str,
    ) -> Result<()>;
    fn attach_literal(&self, rendered: &mut RenderedReceiptContext) -> Result<()>;
    /// Invoke on the actual expression result too, before list/destination proof
    /// construction. Existing child frames already bind their controls in-session.
    fn attach_rendered_control(&self, _rendered: &mut RenderedReceiptContext) -> Result<()> {
        Ok(())
    }
    async fn require_published(&self, conn: &mut PgConnection) -> Result<()>;
    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()>;
    fn ensure_fresh(&self) -> Result<()>;
}

/// A genuine non-indexed predecessor's selected ordinary iteration successors.
/// Private constructor. D owns the downstream list evaluation and child writer.
pub struct PreparedNativeOrdinaryTransitions {
    inner: PreparedNativeTransitions,
    parent: Execution,
    completed: Execution,
    targets: Vec<String>,
    paths: BTreeMap<String, String>,
    bindings: BTreeMap<String, DeclaredInputBinding>,
}

impl NativeTransitionMaterializationRepository {
    /// Source phase only. A genuine whole-iteration D outcome will get its own
    /// typed factory branch; no indexed child is ever promoted to that outcome.
    pub async fn prepare_ordinary_successors(
        conn: &mut PgConnection,
        completed_id: Id,
        key: &str,
    ) -> Result<Option<PreparedNativeOrdinaryTransitions>> {
        let completed = ExecutionRepository::find_by_id(&mut *conn, completed_id)
            .await?
            .ok_or_else(changed)?;
        let Some(metadata) = &completed.workflow_task else {
            return Ok(None);
        };
        if metadata.task_index.is_some() || completed.status != ExecutionStatus::Completed {
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
        if !ordinary_successor_graph(graph)? {
            return Ok(None);
        }
        let workflow =
            WorkflowExecutionRepository::find_by_id(&mut *conn, metadata.workflow_execution)
                .await?
                .ok_or_else(changed)?;
        if workflow.execution != parent.id {
            return Err(changed());
        }
        // Duplicate delivery belongs to D's committed-list recovery path. Never
        // rerender the predecessor's original publications or Keys on a replay.
        if workflow.completed_tasks.contains(&metadata.task_name) {
            return Ok(None);
        }
        let Some(inner) = Self::prepare_completion(conn, &completed, key, true).await? else {
            return Ok(None);
        };
        if inner.workflow.execution != parent.id
            || inner.workflow.workflow_def
                != parent
                    .executable_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
                    .ok_or_else(changed)?
                    .id
        {
            return Err(changed());
        }
        if let Some(saved) = inner
            .workflow
            .task_graph
            .get("nodes")
            .and_then(|nodes| nodes.get(&metadata.task_name))
            .and_then(|node| node.get("transitions"))
        {
            inner.require_transitions(saved)?;
        } else if &inner.workflow.task_graph != graph {
            return Err(changed());
        }
        let mut targets = Vec::new();
        let mut paths = BTreeMap::new();
        let tasks = graph
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(changed)?;
        for (index, transition) in inner.authored.iter().enumerate() {
            if !inner.fires(index)? {
                continue;
            }
            for target in transition
                .get("do")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = target.as_str().ok_or_else(changed)?;
                let mut matching = tasks
                    .iter()
                    .filter(|task| task.get("name").and_then(Value::as_str) == Some(name));
                let node = matching.next().ok_or_else(changed)?;
                if matching.next().is_some() || node.get("with_items").is_none_or(Value::is_null) {
                    return Ok(None);
                }
                if !paths.contains_key(name) {
                    targets.push(name.to_owned());
                    paths.insert(
                        name.to_owned(),
                        format!(
                            "/workflow{}",
                            super::super::declared_input_materialization::pointer_to_node(
                                graph, node
                            )?
                        ),
                    );
                }
            }
        }
        if targets.is_empty() {
            return Ok(None);
        }
        let bindings = DeclaredInputRepository::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let plan = PreparedNativeOrdinaryTransitions {
            inner,
            parent,
            completed,
            targets,
            paths,
            bindings,
        };
        plan.require_source_current(conn).await?;
        Ok(Some(plan))
    }
}

impl PreparedNativeOrdinaryTransitions {
    pub(in crate::repositories) fn held_deadline(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        self.inner.held_deadline()
    }
    pub fn completed_execution(&self) -> &Execution {
        &self.completed
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        self.inner.workflow()
    }
    pub fn target_path(&self, name: &str) -> Result<&str> {
        self.paths.get(name).map(String::as_str).ok_or_else(changed)
    }
    pub async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.inner.ensure_fresh_on(conn).await?;
        let parent = ExecutionRepository::find_by_id(&mut *conn, self.parent.id)
            .await?
            .ok_or_else(changed)?;
        let original = &self.completed;
        let child = ExecutionRepository::find_by_id(&mut *conn, original.id)
            .await?
            .ok_or_else(changed)?;
        if serde_json::to_value(&parent)? != serde_json::to_value(&self.parent)?
            || serde_json::to_value(&child)? != serde_json::to_value(original)?
        {
            return Err(changed());
        }
        Ok(())
    }
    /// D calls this only after its original workflow comparison and before any
    /// child/list target effects, then persists these exact returned buffers.
    /// The sealed expectation belongs to the stage, never a caller setter.
    pub async fn record(
        &self,
        conn: &mut PgConnection,
        actual: &WorkflowExecution,
        key: &str,
    ) -> Result<RecordedNativeTransitions> {
        self.require_source_current(conn).await?;
        self.inner.record(conn, actual, key).await
    }
}

#[async_trait::async_trait]
impl NativeTransitionRenderFrame for PreparedNativeOrdinaryTransitions {
    fn execution(&self) -> Id {
        self.inner.execution()
    }
    fn selected_targets(&self) -> &[String] {
        &self.targets
    }
    async fn bind(
        &self,
        conn: &mut PgConnection,
        session: &mut RenderSession,
        actual: &DeclaredInputBinding,
        key: &str,
    ) -> Result<()> {
        self.require_source_current(conn).await?;
        // D opens the actual declaration/session. Verify the complete original
        // binding and selected ordinary target kind before binding sources.
        if self.bindings.get(&actual.source_path) != Some(actual)
            || !self.paths.values().any(|path| {
                actual
                    .source_path
                    .strip_prefix(path)
                    .and_then(|suffix| suffix.strip_prefix('/'))
                    .is_some_and(|suffix| {
                        matches!(
                            suffix.split('/').next(),
                            Some(
                                "with_items"
                                    | "input"
                                    | "params"
                                    | "action"
                                    | "permission_set_refs"
                                    | "task_timeout"
                                    | "timeout"
                                    | "batch_size"
                                    | "concurrency"
                                    | "worker_selector"
                                    | "worker_tolerations"
                                    | "worker_affinity"
                                    | "trace_tag"
                            )
                        )
                    })
            })
        {
            return Err(changed());
        }
        self.inner
            .bind_prepared_frame(conn, session, actual, key)
            .await
    }
    fn attach_literal(&self, rendered: &mut RenderedReceiptContext) -> Result<()> {
        self.inner.attach_frame_literal(rendered)
    }
    async fn require_published(&self, conn: &mut PgConnection) -> Result<()> {
        self.require_source_current(conn).await?;
        self.inner.require_frame_publication(conn).await?;
        let actual = WorkflowExecutionRepository::find_by_id(&mut *conn, self.inner.workflow.id)
            .await?
            .ok_or_else(changed)?;
        let mut expected = self.inner.workflow.clone();
        // Variables are already compared to the private publication seal above.
        // The database alone supplies its update timestamp. All other workflow
        // fields must still be the original state before D's first target effect.
        expected.variables = actual.variables.clone();
        expected.updated = actual.updated;
        if serde_json::to_value(&actual)? != serde_json::to_value(&expected)? {
            return Err(changed());
        }
        Ok(())
    }
    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.inner.ensure_fresh_on(conn).await
    }
    fn ensure_fresh(&self) -> Result<()> {
        self.inner.ensure_fresh()
    }
}

impl NativeTransitionFrameSeal for PreparedNativeOrdinaryTransitions {}

fn ordinary_successor_graph(graph: &Value) -> Result<bool> {
    check_bounds(graph)?;
    let Some(tasks) = graph.get("tasks").and_then(Value::as_array) else {
        return Ok(false);
    };
    Ok(tasks.iter().all(|task| {
        ["iterate_cache", "wait_for", "tasks"]
            .iter()
            .all(|field| task.get(*field).is_none_or(Value::is_null))
            && task.get("retry").is_none_or(|retry| {
                retry.is_null() || retry.get("count").and_then(Value::as_i64) == Some(0)
            })
    }))
}
