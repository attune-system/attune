//! Source-prepared predecessor publications followed by actual cache activation.
//! No target-phase renderer or declaration/authority discovery is permitted here.
use super::*;
use crate::repositories::native_transition_materialization::NativeTransitionMaterializationRepository as Transitions;
use crate::secret_values::PreparedSecretValue;

pub enum PreparedNativeCacheWork {
    Checkpoint(Box<PreparedNativeCacheCheckpoint>),
    Activation(Box<PreparedNativeCacheActivation>),
}

pub struct PreparedNativeCacheActivation {
    workflow: WorkflowExecution,
    parent: Execution,
    source: String,
    targets: Vec<String>,
    transitions: Option<Arc<PreparedNativeTransitions>>,
    caches: Vec<PreparedNativeCacheCheckpoint>,
    fields: Vec<crate::secret_values::StoredSecretValue>,
    applied: bool,
}

impl NativeCacheMaterializationRepository {
    pub fn supports_work(parent: &Execution, task: &str, completed: Option<&Execution>) -> bool {
        Self::supports(parent, task)
            || completed.is_some_and(|child| {
                child.status == ExecutionStatus::Completed
                    && child.workflow_task.as_ref().is_some_and(|metadata| {
                        metadata.task_index.is_none() && metadata.task_name == task
                    })
                    && activation_targets(parent, task).is_some()
            })
    }

    /// Keep the owning preparation future out of its callers' inline coroutine
    /// storage. The original body still runs once, in this same transaction.
    pub fn prepare_work<'a>(
        conn: &'a mut PgConnection,
        parent: &'a Execution,
        task: &'a str,
        completed: Option<&'a Execution>,
        key: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PreparedNativeCacheWork>> + Send + 'a>,
    > {
        Box::pin(Self::prepare_work_inner(conn, parent, task, completed, key))
    }

    async fn prepare_work_inner(
        conn: &mut PgConnection,
        parent: &Execution,
        task: &str,
        completed: Option<&Execution>,
        key: &str,
    ) -> Result<PreparedNativeCacheWork> {
        if Self::supports(parent, task) {
            return Ok(PreparedNativeCacheWork::Checkpoint(Box::new(
                Self::prepare(conn, parent, task, key).await?,
            )));
        }
        let completed = completed.ok_or_else(denied)?;
        let targets = activation_targets(parent, task).ok_or_else(denied)?;
        let workflow = Workflows::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(denied)?;
        let actual_child = Executions::find_by_id(&mut *conn, completed.id)
            .await?
            .ok_or_else(denied)?;
        if actual_child.parent != Some(parent.id)
            || actual_child.status != ExecutionStatus::Completed
            || serde_json::to_value(&actual_child)? != serde_json::to_value(completed)?
            || actual_child.workflow_task.as_ref().is_none_or(|metadata| {
                metadata.workflow_execution != workflow.id
                    || metadata.task_name != task
                    || metadata.task_index.is_some()
            })
        {
            return Err(denied());
        }
        if workflow.status != ExecutionStatus::Running || !workflow.failed_tasks.is_empty() {
            return Err(denied());
        }
        let fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?;
        // A duplicate completion never re-evaluates its original conditions or
        // publication Keys. Recover only actual still-live cache streams from
        // their now-committed typed variable inputs and original pinned cursor.
        let transitions = if workflow.completed_tasks.iter().any(|name| name == task) {
            None
        } else {
            Some(Arc::new(
                measure(
                    Op::TransitionPreparation,
                    Transitions::prepare_cache_activation(conn, completed, key),
                )
                .await?
                .ok_or_else(denied)?,
            ))
        };
        if let Some(stage) = &transitions {
            if let Some(saved) = workflow
                .task_graph
                .get("nodes")
                .and_then(|nodes| nodes.get(task))
                .and_then(|node| node.get("transitions"))
            {
                stage.require_transitions(saved)?;
            } else if parent
                .executable_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
                .is_none_or(|graph| graph.definition != workflow.task_graph)
            {
                return Err(denied());
            }
        }
        let fires = transitions
            .as_ref()
            .map(|stage| stage.fires(0))
            .transpose()?
            .unwrap_or(true);
        let mut caches = Vec::new();
        if fires {
            for target in &targets {
                if workflow.completed_tasks.contains(target)
                    || workflow.skipped_tasks.contains(target)
                {
                    continue;
                }
                caches.push(if let Some(stage) = &transitions {
                    let frame: Arc<dyn NativeCacheRenderFrame> = stage.clone();
                    Self::prepare_successor_from_frame(conn, parent, target, frame, key).await?
                } else {
                    Self::prepare_inner(conn, parent, target, Some(completed), None, key).await?
                });
            }
        }
        let prepared = PreparedNativeCacheActivation {
            workflow,
            parent: parent.clone(),
            source: task.into(),
            targets,
            transitions,
            caches,
            fields,
            applied: false,
        };
        prepared.ensure_fresh_on(conn).await?;
        Ok(PreparedNativeCacheWork::Activation(Box::new(prepared)))
    }
}

impl PreparedNativeCacheWork {
    pub fn workflow(&self) -> Id {
        match self {
            Self::Checkpoint(plan) => plan.workflow(),
            Self::Activation(plan) => plan.workflow.id,
        }
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        match self {
            Self::Checkpoint(plan) => plan.ensure_fresh_on(conn).await,
            Self::Activation(plan) => plan.ensure_fresh_on(conn).await,
        }
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        match self {
            Self::Checkpoint(plan) => plan.ensure_fresh(),
            Self::Activation(plan) => plan.ensure_fresh(),
        }
    }
    pub async fn apply(
        &mut self,
        conn: &mut PgConnection,
        key: &str,
    ) -> Result<NativeCacheCheckpointResult> {
        match self {
            Self::Checkpoint(plan) => plan.apply(conn, key).await,
            Self::Activation(plan) => plan.apply(conn, key).await,
        }
    }
}

impl PreparedNativeCacheActivation {
    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(stage) = &self.transitions {
            stage.ensure_fresh_on(conn).await?;
        }
        for cache in &self.caches {
            cache.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    fn ensure_fresh(&self) -> Result<()> {
        if let Some(stage) = &self.transitions {
            stage.ensure_fresh()?;
        }
        for cache in &self.caches {
            cache.ensure_fresh()?;
        }
        Ok(())
    }
    async fn apply(
        &mut self,
        conn: &mut PgConnection,
        key: &str,
    ) -> Result<NativeCacheCheckpointResult> {
        if self.applied {
            return Err(denied());
        }
        let actual = Workflows::find_by_id_for_update(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        if serde_json::to_value(&actual)? != serde_json::to_value(&self.workflow)?
            || Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, self.parent.id)
                .await?
                != self.fields
            || serde_json::to_value(
                Executions::find_by_id(&mut *conn, self.parent.id)
                    .await?
                    .ok_or_else(denied)?,
            )? != serde_json::to_value(&self.parent)?
        {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await?;
        let mut fields = self
            .fields
            .iter()
            .map(|field| {
                Ok(PreparedSecretValue {
                    json_path: field.json_path.clone(),
                    encrypted_value: field.encrypted_value.clone(),
                    encryption_key_hash: field.encryption_key_hash.clone().ok_or_else(denied)?,
                    source_kind: field.source_kind.clone(),
                    source_ref: field.source_ref.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut baseline = actual;
        if let Some(stage) = &self.transitions {
            let recorded = stage.record(conn, &baseline, key).await?;
            fields = recorded.secrets;
            Secrets::replace_entity_with_conn(
                conn,
                ENTITY_WORKFLOW_VARIABLES,
                self.parent.id,
                &fields,
            )
            .await?;
            let mut completed = baseline.completed_tasks.clone();
            if !completed.contains(&self.source) {
                completed.push(self.source.clone());
            }
            let mut current = baseline.current_tasks.clone();
            current.retain(|name| name != &self.source);
            let mut skipped = baseline.skipped_tasks.clone();
            if !stage.fires(0)? {
                for target in &self.targets {
                    if !skipped.contains(target) {
                        skipped.push(target.clone());
                    }
                }
            }
            baseline = Workflows::update(
                &mut *conn,
                baseline.id,
                UpdateWorkflowExecutionInput {
                    variables: Some(recorded.variables),
                    completed_tasks: Some(completed),
                    current_tasks: Some(current),
                    skipped_tasks: Some(skipped),
                    ..Default::default()
                },
            )
            .await?;
        }
        let mut result = NativeCacheCheckpointResult {
            children: vec![],
            completed: None,
        };
        for cache in &mut self.caches {
            // Baseline is the exact owning publication result and previous
            // authorized cache effects. Source variables/ciphertext must match,
            // not a caller-supplied replacement or mutable cache JSON.
            let outcome = if cache.frame.is_some() {
                cache.apply(conn, key).await?
            } else {
                cache
                    .apply_after_publications(conn, key, &baseline, &fields)
                    .await?
            };
            result.children.extend(outcome.children);
            result.completed = outcome.completed.or(result.completed);
            baseline = Workflows::find_by_id(&mut *conn, baseline.id)
                .await?
                .ok_or_else(denied)?;
        }
        let skipped = self
            .transitions
            .as_ref()
            .map(|stage| stage.fires(0))
            .transpose()?
            .is_some_and(|fires| !fires);
        if skipped {
            Workflows::update(
                &mut *conn,
                baseline.id,
                UpdateWorkflowExecutionInput {
                    status: Some(ExecutionStatus::Completed),
                    ..Default::default()
                },
            )
            .await?;
            super::super::runtime_read::RuntimeReadRepository::close_execution(
                conn,
                self.parent.id,
            )
            .await?;
            result.completed = Some(
                Executions::update(
                    &mut *conn,
                    self.parent.id,
                    UpdateExecutionInput {
                        status: Some(ExecutionStatus::Completed),
                        result: Some(json!({"succeeded":true})),
                        ..Default::default()
                    },
                )
                .await?,
            );
        }
        self.ensure_fresh_on(conn).await?;
        self.applied = true;
        Ok(result)
    }
}

// First activation unit: one real predecessor transition can activate several
// terminal cache successors. Other graph shapes retain their owning adapters;
// this predicate never evaluates a selector, condition or publication expression.
fn activation_targets(parent: &Execution, source: &str) -> Option<Vec<String>> {
    let graph = &parent
        .executable_snapshot
        .as_ref()?
        .executable
        .workflow_definition
        .as_ref()?
        .definition;
    let tasks = graph.get("tasks")?.as_array()?;
    if tasks.len() < 2 || tasks.len() > MAX_CHECKPOINT_BATCHES {
        return None;
    }
    let node = tasks
        .iter()
        .find(|task| task.get("name").and_then(Value::as_str) == Some(source))?;
    if [
        "iterate_cache",
        "with_items",
        "wait_for",
        "tasks",
        "publish",
        "on_success",
        "on_failure",
        "on_complete",
        "on_timeout",
        "join",
    ]
    .iter()
    .any(|field| {
        node.get(*field).is_some_and(|value| {
            !value.is_null()
                && !value.as_array().is_some_and(Vec::is_empty)
                && !value.as_object().is_some_and(serde_json::Map::is_empty)
        })
    }) {
        return None;
    }
    let next = node.get("next")?.as_array()?;
    let [transition] = next.as_slice() else {
        return None;
    };
    let targets = transition
        .get("do")?
        .as_array()?
        .iter()
        .map(|target| target.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let unique = targets.iter().collect::<BTreeSet<_>>();
    if targets.is_empty()
        || targets.len() + 1 != tasks.len()
        || unique.len() != targets.len()
        || targets.iter().any(|target| target == source)
    {
        return None;
    }
    for target in &targets {
        if !NativeCacheMaterializationRepository::supports(parent, target) {
            return None;
        }
        let task = tasks
            .iter()
            .find(|task| task.get("name").and_then(Value::as_str) == Some(target))?;
        if task.get("join").is_some_and(|value| !value.is_null())
            || ["action", "permission_set_refs"]
                .iter()
                .any(|field| task.get(*field).is_some_and(contains_template))
        {
            return None;
        }
    }
    Some(targets)
}
