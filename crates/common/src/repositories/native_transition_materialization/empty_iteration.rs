//! Empty ordinary transitions consume D's genuine original Control and the
//! separate empty archive. They never call the nonempty member-group factory.
use super::frame::NativeTransitionFrameSeal;
use super::*;
use crate::repositories::native_iteration_materialization::PreparedNativeIteration;
use std::sync::Arc;

pub struct PreparedNativeEmptyTransitions {
    inner: PreparedNativeTransitions,
    source: Arc<PreparedNativeIteration>,
    targets: Vec<String>,
    paths: BTreeMap<String, String>,
    bindings: BTreeMap<String, DeclaredInputBinding>,
}

impl NativeTransitionMaterializationRepository {
    pub(crate) async fn prepare_empty_successors(
        conn: &mut PgConnection,
        source: Arc<PreparedNativeIteration>,
        key: &str,
    ) -> Result<PreparedNativeEmptyTransitions> {
        let evidence = source.empty_source()?;
        evidence.require_source_current(conn).await?;
        let parent = evidence.parent();
        let workflow = evidence.workflow();
        if workflow
            .completed_tasks
            .iter()
            .chain(&workflow.failed_tasks)
            .any(|name| name == evidence.task_name())
        {
            return Err(changed());
        }
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(changed)?;
        let definition = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        let task = definition
            .pointer(
                evidence
                    .task_path()
                    .strip_prefix("/workflow")
                    .ok_or_else(changed)?,
            )
            .ok_or_else(changed)?;
        let next = task
            .get("next")
            .and_then(Value::as_array)
            .ok_or_else(changed)?;
        if !eligible_for(next, true)? {
            return Err(changed());
        }
        // No result()/child/item Value capability accompanies an empty outcome.
        for transition in next {
            let mut leaves = Vec::new();
            collect_leaves(transition, String::new(), &mut leaves, 0)?;
            for (path, value) in leaves {
                if let Some(expression) = value.as_str().filter(|value| value.contains("{{")) {
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
                    }) || expression_functions(expression)?.iter().any(|function| {
                        function == "result"
                            || path != "/when"
                                && matches!(
                                    function.as_str(),
                                    "succeeded" | "failed" | "timed_out" | "cancelled"
                                )
                    }) {
                        return Err(changed());
                    }
                }
            }
        }
        let bindings: BTreeMap<_, _> = DeclaredInputRepository::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let mut sources = prepare_sources(conn, parent, None, workflow, key)
            .await?
            .ok_or_else(changed)?;
        sources.empty_iteration = Some(source.clone());
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| changed())?;
        let mut inner = PreparedNativeTransitions {
            empty_completion: None,
            workflow: workflow.clone(),
            task_name: Some(evidence.task_name().into()),
            authored: next.clone(),
            transitions: Vec::new(),
            original_secrets: ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *conn,
                crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
                parent.id,
            )
            .await?,
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
            .and_then(|nodes| nodes.get(evidence.task_name()))
            .and_then(|node| node.get("transitions"))
        {
            inner.require_transitions(saved)?;
        } else if &workflow.task_graph != definition {
            return Err(changed());
        }
        inner.transitions = Self::prepare_sequence(
            conn,
            parent.id,
            evidence.task_name(),
            evidence.task_path(),
            next,
            workflow,
            &bindings,
            &inner.cache_sources,
            &revision,
            true,
            key,
        )
        .await
        .map_err(|error| {
            source_phase_error(
                error,
                "prepare empty transition sequence",
                evidence.task_path(),
            )
        })?;
        let tasks = definition
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(changed)?;
        let mut targets = Vec::new();
        let mut paths = BTreeMap::new();
        for (index, transition) in next.iter().enumerate() {
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
                let mut matches = tasks
                    .iter()
                    .filter(|task| task.get("name").and_then(Value::as_str) == Some(name));
                let task = matches.next().ok_or_else(changed)?;
                if matches.next().is_some()
                    || name == evidence.task_name()
                    || ["iterate_cache", "wait_for", "tasks", "join"]
                        .iter()
                        .any(|field| task.get(*field).is_some_and(|value| !value.is_null()))
                {
                    return Err(changed());
                }
                if !paths.contains_key(name) {
                    targets.push(name.to_owned());
                    paths.insert(
                        name.to_owned(),
                        format!(
                            "/workflow{}",
                            super::super::declared_input_materialization::pointer_to_node(
                                definition, task
                            )?
                        ),
                    );
                }
            }
        }
        Ok(PreparedNativeEmptyTransitions {
            inner,
            source,
            targets,
            paths,
            bindings,
        })
    }
}

impl PreparedNativeEmptyTransitions {
    pub(crate) fn source_outcome(
        &self,
    ) -> Result<&super::super::native_input_snapshot::PreparedNativeEmptyIterationOutcome> {
        self.source.empty_outcome().ok_or_else(changed)
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        self.inner.workflow()
    }
    pub fn target_path(&self, name: &str) -> Result<&str> {
        self.paths.get(name).map(String::as_str).ok_or_else(changed)
    }
    pub fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok([
            self.source.empty_source()?.held_deadline()?,
            self.source
                .empty_outcome()
                .ok_or_else(changed)?
                .held_deadline()?,
            self.inner.held_deadline()?,
        ]
        .into_iter()
        .flatten()
        .min())
    }
    pub async fn require_source_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.source
            .empty_source()?
            .require_source_current(conn)
            .await
    }
    pub async fn require_members_current(&self, conn: &mut PgConnection) -> Result<()> {
        self.source
            .empty_source()?
            .require_zero_dispatches(conn)
            .await?;
        self.source
            .empty_outcome()
            .ok_or_else(changed)?
            .ensure_fresh_on(conn)
            .await?;
        self.inner.ensure_fresh_on(conn).await
    }
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
impl NativeTransitionFrameSeal for PreparedNativeEmptyTransitions {}
#[async_trait::async_trait]
impl NativeTransitionRenderFrame for PreparedNativeEmptyTransitions {
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
        self.inner.attach_frame_literal(rendered)?;
        self.attach_rendered_control(rendered)
    }
    fn attach_rendered_control(&self, rendered: &mut RenderedReceiptContext) -> Result<()> {
        Inputs::attach_empty_iteration_control(
            rendered,
            self.source.empty_outcome().ok_or_else(changed)?,
        )
    }
    async fn require_published(&self, conn: &mut PgConnection) -> Result<()> {
        self.require_members_current(conn).await?;
        self.inner.require_frame_publication(conn).await?;
        let actual = WorkflowExecutionRepository::find_by_id(&mut *conn, self.workflow().id)
            .await?
            .ok_or_else(changed)?;
        let mut expected = self.workflow().clone();
        expected.variables = actual.variables.clone();
        expected.updated = actual.updated;
        if serde_json::to_value(expected)? != serde_json::to_value(actual)? {
            return Err(changed());
        }
        self.source
            .empty_source()?
            .require_zero_dispatches(conn)
            .await
    }
    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        held::require_deadline_on(conn, self.held_deadline()?).await?;
        self.source
            .empty_outcome()
            .ok_or_else(changed)?
            .ensure_fresh_on(conn)
            .await?;
        self.inner.ensure_fresh_on(conn).await
    }
    fn ensure_fresh(&self) -> Result<()> {
        held::require_deadline(self.held_deadline()?, chrono::Utc::now())?;
        self.source
            .empty_outcome()
            .ok_or_else(changed)?
            .ensure_fresh()?;
        self.inner.ensure_fresh()
    }
}
