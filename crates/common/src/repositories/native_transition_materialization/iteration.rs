//! Whole-iteration transition source. D selects/authenticates membership; A owns
//! lifecycle proof binding. B evaluates the retained next program exactly once.
use super::frame::NativeTransitionFrameSeal;
use super::*;
use crate::repositories::{
    native_input_snapshot::PreparedNativeIterationOutcome,
    native_iteration_materialization::PreparedNativeIterationCompletion,
};
use std::sync::Arc;

pub struct PreparedNativeIterationTransitions {
    inner: PreparedNativeTransitions,
    source: Arc<PreparedNativeIterationCompletion>,
    group: Arc<PreparedNativeIterationOutcome>,
    targets: Vec<String>,
    paths: BTreeMap<String, String>,
    bindings: BTreeMap<String, DeclaredInputBinding>,
}

impl NativeTransitionMaterializationRepository {
    /// BEFORE all workflow/list/child targets. Never substitute a delivered item
    /// or parent execution for D's genuine nonempty whole-group source.
    pub async fn prepare_iteration_successors(
        conn: &mut PgConnection,
        source: Arc<PreparedNativeIterationCompletion>,
        key: &str,
    ) -> Result<Option<PreparedNativeIterationTransitions>> {
        source.require_source_current(conn).await?;
        if source.ordinal_count() == 0 {
            return Err(changed());
        }
        let parent = source.original_parent();
        let workflow = source.workflow();
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(changed)?;
        let definition = snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?;
        if source.renderer() != parent.id
            || workflow.execution != parent.id
            || workflow.workflow_def != definition.id
        {
            return Err(changed());
        }
        // Duplicate replay belongs to D's committed successor recovery, with no
        // new group archive, condition evaluation or publication Key read.
        if workflow
            .completed_tasks
            .iter()
            .chain(&workflow.failed_tasks)
            .any(|name| name == source.task_name())
        {
            return Ok(None);
        }
        if !workflow.failed_tasks.is_empty()
            || !group_graph_supported(&definition.definition, source.task_name())?
        {
            return Ok(None);
        }
        let task = definition
            .definition
            .pointer(
                source
                    .task_path()
                    .strip_prefix("/workflow")
                    .ok_or_else(changed)?,
            )
            .ok_or_else(changed)?;
        if task.get("name").and_then(Value::as_str) != Some(source.task_name())
            || task.get("with_items").is_none_or(Value::is_null)
        {
            return Err(changed());
        }
        let Some(next) = task.get("next").and_then(Value::as_array) else {
            return Ok(None);
        };
        if !eligible_for(next, true)? || !group_sites_supported(next)? {
            return Ok(None);
        }
        let bindings: BTreeMap<_, _> = DeclaredInputRepository::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let group = Arc::new(Inputs::prepare_native_iteration_outcome(conn, &source, key).await?);
        group.require_original_rows(parent, workflow)?;
        group.require_source_current(conn).await?;
        let Some(mut sources) = measure(
            Op::RootSources,
            prepare_sources(conn, parent, None, workflow, key),
        )
        .await?
        else {
            return Ok(None);
        };
        sources.iteration = Some(IterationOutcome {
            task_path: source.task_path().to_owned(),
            proof: group.clone(),
        });
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| changed())?;
        let original_secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
            parent.id,
        )
        .await?;
        let mut inner = PreparedNativeTransitions {
            workflow: workflow.clone(),
            task_name: Some(source.task_name().to_owned()),
            authored: next.clone(),
            transitions: Vec::new(),
            original_secrets,
            recorded: std::sync::OnceLock::new(),
            output: None,
            output_receipts: std::sync::OnceLock::new(),
            inherited_controls: inherited_control_sources(&sources),
            cache_sources: Arc::new(sources),
            cache_publication: std::sync::OnceLock::new(),
        };
        if let Some(saved) = workflow
            .task_graph
            .get("nodes")
            .and_then(|nodes| nodes.get(source.task_name()))
            .and_then(|node| node.get("transitions"))
        {
            inner.require_transitions(saved)?;
        } else if workflow.task_graph != definition.definition {
            return Err(changed());
        }
        inner.transitions = Self::prepare_sequence(
            conn,
            parent.id,
            source.task_name(),
            source.task_path(),
            next,
            workflow,
            &bindings,
            &inner.cache_sources,
            &revision,
            true,
            key,
        )
        .await?;
        let tasks = definition
            .definition
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(changed)?;
        let mut targets = Vec::new();
        let mut paths = BTreeMap::new();
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
                    .filter(|node| node.get("name").and_then(Value::as_str) == Some(name));
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
                                &definition.definition,
                                node
                            )?
                        ),
                    );
                }
            }
        }
        let frame = PreparedNativeIterationTransitions {
            inner,
            source,
            group,
            targets,
            paths,
            bindings,
        };
        frame.require_source_current(conn).await?;
        Ok(Some(frame))
    }
}

impl PreparedNativeIterationTransitions {
    pub(in crate::repositories) fn held_deadline(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok([
            self.inner.held_deadline()?,
            self.source.held_deadline()?,
            self.group.held_deadline()?,
        ]
        .into_iter()
        .flatten()
        .min())
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        self.inner.workflow()
    }
    pub fn source(&self) -> &Arc<PreparedNativeIterationCompletion> {
        &self.source
    }
    pub fn group(&self) -> &PreparedNativeIterationOutcome {
        &self.group
    }
    pub fn target_path(&self, target: &str) -> Result<&str> {
        self.paths
            .get(target)
            .map(String::as_str)
            .ok_or_else(changed)
    }
    pub async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.group
            .require_original_rows(self.source.original_parent(), self.workflow())?;
        self.source.require_source_current(conn).await?;
        self.group.require_source_current(conn).await?;
        self.inner.ensure_fresh_on(conn).await
    }
    pub async fn require_members_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.require_members_current(conn).await?;
        self.group.require_members_current(conn).await?;
        self.inner.ensure_fresh_on(conn).await
    }
    pub async fn record(
        &self,
        conn: &mut PgConnection,
        actual: &WorkflowExecution,
        key: &str,
    ) -> Result<RecordedNativeTransitions> {
        let parent = ExecutionRepository::find_by_id(&mut *conn, self.source.renderer())
            .await?
            .ok_or_else(changed)?;
        self.group.require_original_rows(&parent, actual)?;
        self.require_source_current(conn).await?;
        self.inner.record(conn, actual, key).await
    }
}

impl NativeTransitionFrameSeal for PreparedNativeIterationTransitions {}
#[async_trait::async_trait]
impl NativeTransitionRenderFrame for PreparedNativeIterationTransitions {
    fn execution(&self) -> Id {
        self.source.renderer()
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
                                    | "params"
                                    | "input"
                                    | "action"
                                    | "permission_set_refs"
                                    | "timeout"
                                    | "task_timeout"
                                    | "batch_size"
                                    | "concurrency"
                                    | "trace_tag_template"
                                    | "worker_selector"
                                    | "worker_tolerations"
                                    | "worker_affinity"
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
    fn attach_rendered_control(&self, rendered: &mut RenderedReceiptContext) -> Result<()> {
        Inputs::attach_native_iteration_control(rendered, &self.group)
    }
    async fn require_published(&self, conn: &mut PgConnection) -> Result<()> {
        // Do not run D/A's original workflow equality after our own publications.
        self.require_members_current(conn).await?;
        self.inner.require_frame_publication(conn).await?;
        let actual = WorkflowExecutionRepository::find_by_id(&mut *conn, self.workflow().id)
            .await?
            .ok_or_else(changed)?;
        let mut expected = self.workflow().clone();
        expected.variables = actual.variables.clone(); // exact private seal was just checked
        expected.updated = actual.updated;
        if serde_json::to_value(&expected)? != serde_json::to_value(&actual)? {
            return Err(changed());
        }
        self.require_members_current(conn).await
    }
    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.source.ensure_fresh_on(conn).await?;
        self.group.ensure_fresh_on(conn).await?;
        self.inner.ensure_fresh_on(conn).await
    }
    fn ensure_fresh(&self) -> Result<()> {
        self.source.ensure_fresh()?;
        self.group.ensure_fresh()?;
        self.inner.ensure_fresh()
    }
}

fn group_graph_supported(graph: &Value, source: &str) -> Result<bool> {
    check_bounds(graph)?;
    let Some(tasks) = graph.get("tasks").and_then(Value::as_array) else {
        return Ok(false);
    };
    Ok(tasks.iter().all(|task| {
        ["iterate_cache", "wait_for", "tasks"]
            .iter()
            .all(|field| task.get(*field).is_none_or(Value::is_null))
            && (task.get("name").and_then(Value::as_str) == Some(source)
                || task.get("retry").is_none_or(|retry| {
                    retry.is_null() || retry.get("count").and_then(Value::as_i64) == Some(0)
                }))
    }))
}
fn group_sites_supported(next: &[Value]) -> Result<bool> {
    for transition in next {
        let mut leaves = Vec::new();
        collect_leaves(transition, String::new(), &mut leaves, 0)?;
        for (path, value) in leaves {
            let Some(expression) = value.as_str().filter(|value| value.contains("{{")) else {
                continue;
            };
            if expression_names(expression)?.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "result"
                        | "task"
                        | "tasks"
                        | "item"
                        | "index"
                        | "inquiry"
                        | "execution"
                        | "work_queue_item"
                )
            }) {
                return Ok(false);
            }
            if expression_functions(expression)?.iter().any(|name| {
                name == "result"
                    || path != "/when"
                        && matches!(
                            name.as_str(),
                            "succeeded" | "failed" | "timed_out" | "cancelled"
                        )
            }) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
