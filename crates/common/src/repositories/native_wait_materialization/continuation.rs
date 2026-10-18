//! Actual terminal wait recovery, without a synthetic execution. Independent
//! publications and explicit recovery inputs are prepared before outer targets.
use super::*;
use crate::repositories::Update;

fn wait_cache_stage(error: Error, stage: &'static str) -> Error {
    match error {
        Error::PermissionDenied(message) => {
            Error::PermissionDenied(format!("Wait continuation [{stage}] denied: {message}"))
        }
        other => other,
    }
}

struct Decision {
    rendered: RenderedReceiptContext,
    destination: PreparedReceiptDestination,
}
struct Publication {
    name: String,
    value: Value,
    fields: Vec<Field>,
    control_prefix: usize,
}
pub(super) struct Recovery {
    pub(super) name: String,
    pub(super) child: Child,
    pub(super) fence: ApprovalEffectFence,
}

pub struct PreparedNativeWaitContinuation {
    completion: Option<std::sync::Arc<PreparedNativeWaitCompletion>>,
    output: Option<super::super::inherited_input::PreparedNativeWaitCompletionDestination>,
    context: Context,
    original: WorkflowTaskWait,
    original_secrets: Vec<StoredSecretValue>,
    observation: Option<Observation>,
    admission: ExecutionAdmission,
    selector: Option<ReceiptContext>,
    decisions: Vec<Decision>,
    publications: Vec<Publication>,
    children: Vec<Recovery>,
    replay: Vec<Execution>,
    replay_fences: Vec<ApprovalEffectFence>,
    replay_controls: Vec<super::super::inherited_input::PreparedInputSource>,
    replay_caches: Vec<super::cache_replay::Replay>,
    receipts: Vec<ReceiptContext>,
    released: std::sync::Arc<OnceLock<ReleasedWaitInput>>,
    applied: bool,
    staged: Option<std::sync::Arc<super::staged::Program>>,
    cache_frame: Option<std::sync::Arc<super::cache_frame::Frame>>,
    caches: Vec<super::super::native_cache_materialization::PreparedNativeCacheCheckpoint>,
    completions: Vec<Execution>,
}
pub struct NativeWaitContinuationRepository;

impl NativeWaitContinuationRepository {
    pub async fn prepare(
        conn: &mut PgConnection,
        wait: &WorkflowTaskWait,
        key: Option<&str>,
    ) -> Result<Option<PreparedNativeWaitContinuation>> {
        if !matches!(
            wait.state,
            WorkflowTaskWaitState::Waiting
                | WorkflowTaskWaitState::Failed
                | WorkflowTaskWaitState::TimedOut
        ) {
            return Ok(None);
        }
        if wait.state == WorkflowTaskWaitState::Waiting
            && !terminal_source(conn, wait.target()?).await?
        {
            return Ok(None);
        }
        let workflow = Workflows::find_by_id(&mut *conn, wait.workflow_execution)
            .await?
            .ok_or_else(denied)?;
        let parent = Executions::find_by_id(&mut *conn, workflow.execution)
            .await?
            .ok_or_else(denied)?;
        if Admissions::find(conn, RuntimeOwner::Execution(parent.id))
            .await?
            .is_none()
        {
            return Ok(None);
        }
        if parent.status != ExecutionStatus::Running
            || workflow.status != ExecutionStatus::Running
            || workflow.paused
        {
            return Err(denied());
        }
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(denied)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let task = unique_task(graph, &wait.task_name)?;
        if !supported_program(task, graph)? {
            return Ok(None);
        }
        let key = key.ok_or_else(denied)?;
        let action = task
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        let path =
            Materialize::workflow_task_site_path(conn, parent.id, &wait.task_name, &action).await?;
        let task = task.clone();
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| denied())?;
        let declarations = Declarations::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|decl| (decl.source_path.clone(), decl))
            .collect();
        let sources = prepare_sources(conn, &parent, None, &workflow, key)
            .await?
            .ok_or_else(denied)?;
        let context = Context {
            predecessor: None,
            parent,
            workflow,
            task,
            path,
            revision,
            declarations,
            sources: sources.into(),
        };
        let executable = Admissions::resolve_workflow_snapshot(
            conn,
            context.parent.id,
            &wait.task_name,
            &action,
        )
        .await?;
        let refs = refs(&context.task, &executable)?;
        let admission = Admissions::workflow_child(
            conn,
            context.parent.id,
            &wait.task_name,
            &executable,
            &refs,
        )
        .await?;
        let selector = if wait.state == WorkflowTaskWaitState::Waiting {
            if let Some(original) = Snapshots::initial_wait_selector(
                conn,
                context.parent.id,
                &context.path,
                wait.id,
                key,
            )
            .await?
            {
                original
            } else {
                let wait_for: crate::workflow::TaskWaitFor = serde_json::from_value(
                    context.task.get("wait_for").cloned().ok_or_else(denied)?,
                )?;
                let (kind, target) = wait_for.target();
                if contains_template(target) {
                    let relative = format!("/wait_for/{kind}");
                    let rendered = context
                        .render(conn, &relative, InputSitePurpose::Control, None, key)
                        .await?;
                    let site = InputSite {
                        execution: context.parent.id,
                        kind: InputEntityKind::WorkflowVariables,
                        purpose: InputSitePurpose::Control,
                        path: format!("{}{relative}", context.path),
                    };
                    let destination =
                        Inputs::prepare_destination(conn, &rendered, &site, key).await?;
                    Some(
                        Inputs::record_prepared(
                            conn,
                            &rendered,
                            &destination,
                            rendered.value(),
                            rendered.selections(),
                            key,
                        )
                        .await?,
                    )
                } else {
                    None
                }
            }
        } else {
            None
        };
        let observation = if wait.state == WorkflowTaskWaitState::Waiting {
            Observation::Pending(
                Snapshots::prepare_wait(
                    conn,
                    context.parent.id,
                    &context.path,
                    &admission,
                    wait.id,
                    selector.as_ref(),
                    key,
                )
                .await
                .map_err(|error| wait_cache_stage(error, "wait-observation"))?,
            )
        } else {
            Observation::Released(
                Snapshots::prepare_released_wait(
                    conn,
                    context.parent.id,
                    &context.path,
                    wait.id,
                    key,
                )
                .await?,
            )
        };
        let original_secrets = Secrets::find_stored_by_entity(
            &mut *conn,
            ENTITY_WORKFLOW_VARIABLES,
            context.parent.id,
        )
        .await?;
        check_bounds(&context.workflow.variables)?;
        if original_secrets.len() > 256 {
            return Err(denied());
        }
        let mut secret_bytes = 2usize;
        for field in &original_secrets {
            secret_bytes = secret_bytes
                .checked_add(
                    serde_json::to_vec(&serde_json::json!({
                        "json_path": field.json_path,
                        "encrypted_value": field.encrypted_value,
                        "encryption_key_hash": field.encryption_key_hash,
                        "source_kind": field.source_kind,
                        "source_ref": field.source_ref,
                    }))?
                    .len()
                        + 1,
                )
                .ok_or_else(denied)?;
            if secret_bytes > 1024 * 1024 {
                return Err(denied());
            }
        }
        let mut plan = PreparedNativeWaitContinuation {
            completion: None,
            output: None,
            context,
            original: wait.clone(),
            original_secrets,
            observation: Some(observation),
            admission,
            selector,
            decisions: vec![],
            publications: vec![],
            children: vec![],
            replay: vec![],
            replay_fences: vec![],
            replay_controls: vec![],
            replay_caches: vec![],
            receipts: vec![],
            released: std::sync::Arc::new(OnceLock::new()),
            applied: false,
            staged: None,
            cache_frame: None,
            caches: vec![],
            completions: vec![],
        };
        if plan
            .context
            .workflow
            .completed_tasks
            .contains(&wait.task_name)
        {
            plan.prepare_replay(conn, key).await?;
            return Ok(Some(plan));
        }
        if plan.context.workflow.failed_tasks.contains(&wait.task_name) {
            return Err(denied());
        }
        let graph = &plan
            .context
            .parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        if super::program_shape::may_finish_without_recovery(&plan.context.task)?
            || needs_staged(&plan.context.task)?
            || has_cache_successor(&plan.context.task, graph)?
            || recovery_reads_publications(&plan.context.task, graph)?
        {
            let program = std::sync::Arc::new(
                super::staged::Program::prepare(
                    conn,
                    &plan.context,
                    plan.observation.as_ref().ok_or_else(denied)?,
                    key,
                )
                .await
                .map_err(|error| wait_cache_stage(error, "staged-program"))?,
            );
            let frame = std::sync::Arc::new(
                super::cache_frame::Frame::new(
                    conn,
                    &plan.context,
                    &plan.original,
                    plan.observation.as_ref().ok_or_else(denied)?,
                    program.clone(),
                    plan.released.clone(),
                    key,
                )
                .await
                .map_err(|error| wait_cache_stage(error, "cache-frame"))?,
            );
            if program.successors.is_empty() {
                let source = std::sync::Arc::new(
                    PreparedNativeWaitCompletion::prepare(
                        conn,
                        &plan.context,
                        plan.observation.as_ref().ok_or_else(denied)?,
                        program.clone(),
                        frame.clone(),
                        key,
                    )
                    .await?,
                );
                plan.output =
                    Some(Inputs::prepare_native_wait_completion(conn, source.clone(), key).await?);
                plan.completion = Some(source);
            }
            for name in &program.successors {
                let task = unique_task(graph, name)?;
                if task
                    .get("iterate_cache")
                    .is_some_and(|value| !value.is_null())
                {
                    plan.caches.push(super::super::native_cache_materialization::NativeCacheMaterializationRepository::prepare_successor_from_frame(
                        conn,&plan.context.parent,name,frame.clone(),key).await
                        .map_err(|error| wait_cache_stage(error, "cache-successor"))?);
                } else {
                    plan.children.push(
                        prepare_recovery_from_program(conn, &plan.context, name, &program, key)
                            .await?,
                    );
                }
            }
            let bytes = plan
                .children
                .iter()
                .try_fold(program.bytes()?, |bytes, child| {
                    Ok::<_, Error>(
                        bytes
                            .checked_add(serde_json::to_vec(&child.child.value)?.len())
                            .ok_or_else(denied)?,
                    )
                })?;
            if bytes > 1024 * 1024 {
                return Err(denied());
            }
            plan.staged = Some(program);
            if !plan.caches.is_empty() || plan.completion.is_some() {
                plan.cache_frame = Some(frame);
            }
            plan.ensure_prepared_on(conn).await?;
            return Ok(Some(plan));
        }
        let next = plan
            .context
            .task
            .get("next")
            .and_then(Value::as_array)
            .ok_or_else(denied)?;
        let mut recovery_names = Vec::new();
        for (index, transition) in next.iter().enumerate() {
            let relative = format!("/next/{index}/when");
            let rendered = plan
                .context
                .render(
                    conn,
                    &relative,
                    InputSitePurpose::Condition,
                    plan.observation.as_ref(),
                    key,
                )
                .await?;
            let fires = rendered.value().as_bool().ok_or_else(denied)?;
            let site = InputSite {
                execution: plan.context.parent.id,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Condition,
                path: format!("{}{relative}", plan.context.path),
            };
            let destination = Inputs::prepare_destination(conn, &rendered, &site, key).await?;
            plan.decisions.push(Decision {
                rendered,
                destination,
            });
            if !fires {
                continue;
            }
            if let Some(values) = transition.get("publish").and_then(Value::as_object) {
                for (name, authored) in values {
                    let mut value = authored.clone();
                    let mut leaves = Vec::new();
                    collect_leaves(authored, String::new(), &mut leaves, 0)?;
                    let mut fields = Vec::new();
                    for (suffix, _) in leaves {
                        let relative = format!(
                            "/next/{index}/publish/{}{suffix}",
                            super::super::native_workflow_materialization::escape(name)
                        );
                        let rendered = plan
                            .context
                            .render(
                                conn,
                                &relative,
                                InputSitePurpose::Value,
                                plan.observation.as_ref(),
                                key,
                            )
                            .await?;
                        *value.pointer_mut(&suffix).ok_or_else(denied)? = rendered.value().clone();
                        let destination = Inputs::prepare_destination(
                            conn,
                            &rendered,
                            &InputSite {
                                execution: plan.context.parent.id,
                                kind: InputEntityKind::WorkflowVariables,
                                purpose: InputSitePurpose::Value,
                                path: format!(
                                    "/{}{suffix}",
                                    super::super::native_workflow_materialization::escape(name)
                                ),
                            },
                            key,
                        )
                        .await?;
                        fields.push(Field {
                            path: suffix,
                            rendered,
                            destination,
                        });
                    }
                    check_bounds(&value)?;
                    plan.publications.push(Publication {
                        name: name.clone(),
                        value,
                        fields,
                        control_prefix: plan.decisions.len(),
                    });
                }
            }
            for name in transition
                .get("do")
                .and_then(Value::as_array)
                .ok_or_else(denied)?
            {
                let name = name.as_str().ok_or_else(denied)?;
                if !recovery_names.iter().any(|saved| saved == name) {
                    recovery_names.push(name.to_owned());
                }
            }
        }
        // This supported program has an exhaustive real failure/timeout/cancel
        // recovery. No generated terminal failure bytes or synthetic child.
        if recovery_names.is_empty() {
            return Err(denied());
        }
        for name in recovery_names {
            plan.children
                .push(prepare_recovery(conn, &plan.context, &name, key).await?);
            let bytes = plan
                .publications
                .iter()
                .map(|publication| serde_json::to_vec(&publication.value))
                .chain(
                    plan.children
                        .iter()
                        .map(|recovery| serde_json::to_vec(&recovery.child.value)),
                )
                .try_fold(0usize, |total, bytes| {
                    Ok::<_, Error>(total.checked_add(bytes?.len()).ok_or_else(denied)?)
                })?;
            if bytes > 1024 * 1024 {
                return Err(denied());
            }
        }
        plan.ensure_prepared_on(conn).await?;
        Ok(Some(plan))
    }
}

impl PreparedNativeWaitContinuation {
    pub fn workflow(&self) -> Id {
        self.context.workflow.id
    }
    pub fn wait(&self) -> Id {
        self.original.id
    }
    pub fn completions(&self) -> &[Execution] {
        &self.completions
    }
    async fn prepare_replay(&mut self, conn: &mut PgConnection, key: &str) -> Result<()> {
        let Observation::Released(released) = self.observation.as_ref().ok_or_else(denied)? else {
            return Err(denied());
        };
        let snapshot = released.outcome_binding()?.snapshot;
        let next = self
            .context
            .task
            .get("next")
            .and_then(Value::as_array)
            .ok_or_else(denied)?;
        let mut controls = BTreeMap::new();
        let mut seen = std::collections::BTreeSet::new();
        for transition in next {
            for name in transition
                .get("do")
                .and_then(Value::as_array)
                .ok_or_else(denied)?
            {
                let name = name.as_str().ok_or_else(denied)?;
                if !seen.insert(name.to_owned()) {
                    continue;
                }
                let graph = &self
                    .context
                    .parent
                    .executable_snapshot
                    .as_ref()
                    .ok_or_else(denied)?
                    .executable
                    .workflow_definition
                    .as_ref()
                    .ok_or_else(denied)?
                    .definition;
                if unique_task(graph, name)?
                    .get("iterate_cache")
                    .is_some_and(|value| !value.is_null())
                {
                    if let Some(replay) =
                        super::cache_replay::Replay::prepare(conn, &self.context, name, key).await?
                    {
                        collect_controls(
                            conn,
                            &self.context,
                            snapshot,
                            replay.receipts(),
                            &mut controls,
                            key,
                        )
                        .await?;
                        self.replay_caches.push(replay);
                    }
                    continue;
                }
                let Some(child) =
                    Executions::find_by_workflow_task(&mut *conn, self.workflow(), name, None)
                        .await?
                else {
                    continue;
                };
                if child.parent != Some(self.context.parent.id)
                    || child
                        .workflow_task
                        .as_ref()
                        .and_then(|metadata| metadata.triggered_by.as_deref())
                        != Some(self.original.task_name.as_str())
                {
                    return Err(denied());
                }
                let graph = &self
                    .context
                    .parent
                    .executable_snapshot
                    .as_ref()
                    .ok_or_else(denied)?
                    .executable
                    .workflow_definition
                    .as_ref()
                    .ok_or_else(denied)?
                    .definition;
                let recovery = unique_task(graph, name)?;
                if recovery.get("action").and_then(Value::as_str) != Some(child.action_ref.as_str())
                {
                    return Err(denied());
                }
                let executable = Admissions::resolve_workflow_snapshot(
                    conn,
                    self.context.parent.id,
                    name,
                    &child.action_ref,
                )
                .await?;
                if serde_json::to_value(&child.executable_snapshot)?
                    != serde_json::to_value(Some(&executable))?
                    || child.permission_set_refs != refs(recovery, &executable)?
                {
                    return Err(denied());
                }
                let admitted = Authority::execution_admission(conn, child.id)
                    .await?
                    .ok_or_else(denied)?;
                let metadata = child.workflow_task.as_ref().ok_or_else(denied)?;
                if metadata.task_index.is_some()
                    || metadata.task_batch.is_some()
                    || metadata.retry_count != 0
                    || admitted.authority.binding
                        != (crate::runtime_authority::InvocationBinding::WorkflowTask {
                            workflow_ref: self.context.parent.action_ref.clone(),
                            task_path: format!("/tasks/{name}"),
                            action_ref: child.action_ref.clone(),
                        })
                {
                    return Err(denied());
                }
                self.replay_fences.push(
                    super::super::accepted_input::AcceptedInputRepository::revalidate_execution(
                        conn, child.id, key,
                    )
                    .await?,
                );
                let receipts =
                    Materialize::prepare_execution_input_receipts(conn, child.id, key).await?;
                collect_controls(conn, &self.context, snapshot, &receipts, &mut controls, key)
                    .await?;
                self.receipts.extend(receipts);
                self.replay.push(child);
            }
        }
        if controls.len() != next.len() || (self.replay.is_empty() && self.replay_caches.is_empty())
        {
            return Err(denied());
        }
        let mut expected = std::collections::BTreeSet::new();
        for (index, transition) in next.iter().enumerate() {
            let fires = controls[&index].value().as_bool().ok_or_else(denied)?;
            for name in transition
                .get("do")
                .and_then(Value::as_array)
                .ok_or_else(denied)?
            {
                if fires {
                    expected.insert(name.as_str().ok_or_else(denied)?.to_owned());
                }
            }
        }
        let mut actual = self
            .replay
            .iter()
            .map(|child| {
                child
                    .workflow_task
                    .as_ref()
                    .map(|metadata| metadata.task_name.clone())
                    .ok_or_else(denied)
            })
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        actual.extend(
            self.replay_caches
                .iter()
                .map(|cache| cache.task().to_owned()),
        );
        if actual != expected {
            return Err(denied());
        }
        self.replay_controls = controls.into_values().collect();
        Ok(())
    }
    pub async fn apply(&mut self, conn: &mut PgConnection, key: &str) -> Result<Vec<Execution>> {
        if self.applied {
            return Err(denied());
        }
        let actual = Workflows::find_by_id_for_update(&mut *conn, self.workflow())
            .await?
            .ok_or_else(denied)?;
        let parent = Executions::find_by_id(&mut *conn, self.context.parent.id)
            .await?
            .ok_or_else(denied)?;
        let wait = Waits::find_by_workflow_task_for_update(
            conn,
            self.workflow(),
            &self.original.task_name,
        )
        .await?
        .ok_or_else(denied)?;
        if serde_json::to_value(&actual)? != serde_json::to_value(&self.context.workflow)?
            || serde_json::to_value(&parent)? != serde_json::to_value(&self.context.parent)?
            || serde_json::to_value(&wait)? != serde_json::to_value(&self.original)?
            || Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?
                != self.original_secrets
        {
            return Err(denied());
        }
        if let Some(output) = &self.output {
            super::completion_writer::require_original_targets(conn, &self.context, output).await?;
        }
        for cache in &self.replay_caches {
            cache.ensure_fresh_on(conn).await?;
        }
        let released = match self.observation.take().ok_or_else(denied)? {
            Observation::Pending(wait) => Snapshots::release_wait(conn, wait).await?,
            Observation::Released(wait) => wait,
            Observation::Prospective(_) => return Err(denied()),
        };
        if !matches!(released.state(), "failed" | "timed_out") {
            return Err(denied());
        }
        self.released.set(released).map_err(|_| denied())?;
        if let Some(source) = &self.completion {
            let (receipt, completed) = super::completion_writer::finish(
                conn,
                &self.context,
                &self.original_secrets,
                source,
                self.staged.as_ref().ok_or_else(denied)?,
                self.cache_frame.as_ref().ok_or_else(denied)?,
                self.output.as_ref().ok_or_else(denied)?,
                key,
            )
            .await?;
            self.receipts.push(receipt);
            self.completions.push(completed);
            self.applied = true;
            self.ensure_fresh_on(conn).await?;
            return Ok(vec![]);
        }
        if !self.replay.is_empty() || !self.replay_caches.is_empty() {
            let mut pending = Vec::new();
            for child in &self.replay {
                let row = Executions::find_by_id(&mut *conn, child.id)
                    .await?
                    .ok_or_else(denied)?;
                if serde_json::to_value(&row)? != serde_json::to_value(child)? {
                    return Err(denied());
                }
                if row.status == ExecutionStatus::Requested {
                    pending.push(row);
                }
            }
            for cache in &self.replay_caches {
                pending.extend(cache.lock_and_pending(conn).await?);
            }
            self.applied = true;
            self.ensure_fresh_on(conn).await?;
            return Ok(pending);
        }
        let mut recorded_controls = Vec::new();
        for decision in &self.decisions {
            let receipt = Inputs::record_prepared(
                conn,
                &decision.rendered,
                &decision.destination,
                decision.rendered.value(),
                decision.rendered.selections(),
                key,
            )
            .await?;
            recorded_controls
                .push(Inputs::prepare_recorded_source(conn, parent.id, &receipt, key).await?);
            self.receipts.push(receipt);
        }
        let controls = self
            .staged
            .as_ref()
            .map(|program| program.controls())
            .unwrap_or(&recorded_controls);
        let mut variables = actual.variables.clone();
        let mut secrets = self
            .original_secrets
            .iter()
            .map(|field| {
                Ok(crate::secret_values::PreparedSecretValue {
                    json_path: field.json_path.clone(),
                    encrypted_value: field.encrypted_value.clone(),
                    encryption_key_hash: field.encryption_key_hash.clone().ok_or_else(denied)?,
                    source_kind: field.source_kind.clone(),
                    source_ref: field.source_ref.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if let Some(program) = &self.staged {
            let (published, fields) = program
                .publish(
                    conn,
                    actual.id,
                    parent.id,
                    variables,
                    &self.original_secrets,
                    key,
                )
                .await?;
            variables = published;
            secrets = fields;
        }
        for publication in &mut self.publications {
            let mut sources = Vec::new();
            for field in &mut publication.fields {
                for control in &controls[..publication.control_prefix] {
                    Inputs::attach_prepared_destination_control(
                        &mut field.rendered,
                        &mut field.destination,
                        control,
                    )?;
                }
                let receipt = Inputs::record_prepared(
                    conn,
                    &field.rendered,
                    &field.destination,
                    field.rendered.value(),
                    field.rendered.selections(),
                    key,
                )
                .await?;
                sources.push(SecretPathSource {
                    path: field.path.clone(),
                    source: SecretSource::Bound(receipt.provenance()),
                });
                self.receipts.push(receipt);
            }
            let path = format!(
                "/{}",
                super::super::native_workflow_materialization::escape(&publication.name)
            );
            secrets.retain(|field| {
                crate::secret_values::pointer_suffix(&field.json_path, &path).is_none()
            });
            let (value, fields) = crate::secret_values::redact_secret_path_sources(
                publication.value.clone(),
                &sources,
            );
            for mut field in crate::secret_values::prepare_secret_values(fields, key)? {
                field.json_path = crate::secret_values::pointer_join(&path, &field.json_path);
                secrets.push(field);
            }
            variables
                .as_object_mut()
                .ok_or_else(denied)?
                .insert(publication.name.clone(), value);
        }
        check_bounds(&variables)?;
        if secrets.len() > 256 || serde_json::to_vec(&secrets)?.len() > 1024 * 1024 {
            return Err(denied());
        }
        let mut pending = Vec::new();
        let mut current = actual.current_tasks.clone();
        current.retain(|name| name != &self.original.task_name);
        for recovery in &mut self.children {
            pending.push(
                write_recovery_child(conn, &mut recovery.child, controls, &mut self.receipts, key)
                    .await?,
            );
            if !current.contains(&recovery.name) {
                current.push(recovery.name.clone());
            }
        }
        let mut completed = actual.completed_tasks.clone();
        if !completed.contains(&self.original.task_name) {
            completed.push(self.original.task_name.clone());
        }
        let failed = actual
            .failed_tasks
            .iter()
            .filter(|name| *name != &self.original.task_name)
            .cloned()
            .collect();
        Secrets::replace_entity_with_conn(conn, ENTITY_WORKFLOW_VARIABLES, parent.id, &secrets)
            .await?;
        Workflows::update(
            &mut *conn,
            actual.id,
            super::super::workflow::UpdateWorkflowExecutionInput {
                variables: Some(variables.clone()),
                current_tasks: Some(current),
                completed_tasks: Some(completed),
                failed_tasks: Some(failed),
                ..Default::default()
            },
        )
        .await?;
        if let Some(frame) = &self.cache_frame {
            frame.seal(conn, &variables, &secrets).await?;
        }
        for cache in &mut self.caches {
            let result = cache.apply(conn, key).await?;
            pending.extend(result.children);
            if let Some(completed) = result.completed {
                self.completions.push(completed);
            }
        }
        self.applied = true;
        self.ensure_fresh_on(conn).await?;
        Ok(pending)
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(output) = &self.output {
            output.ensure_fresh_on(conn).await?;
        }
        if !self.applied {
            return Err(denied());
        }
        if let Some(frame) = &self.cache_frame {
            super::super::native_cache_materialization::NativeCacheRenderFrame::require_published(
                frame.as_ref(),
                conn,
            )
            .await?;
        }
        self.released
            .get()
            .ok_or_else(denied)?
            .ensure_fresh_on(conn)
            .await?;
        self.admission.ensure_fresh_on(conn).await?;
        if let Some(program) = &self.staged {
            program.ensure_fresh_on(conn).await?;
        }
        for cache in &self.caches {
            cache.ensure_fresh_on(conn).await?;
        }
        for cache in &self.replay_caches {
            cache.ensure_fresh_on(conn).await?;
        }
        for receipt in self.receipts.iter().chain(self.selector.iter()) {
            receipt.ensure_fresh_on(conn).await?;
        }
        for recovery in &self.children {
            recovery.fence.ensure_fresh_on(conn).await?;
        }
        for control in &self.replay_controls {
            control.ensure_fresh_on(conn).await?;
        }
        for fence in &self.replay_fences {
            fence.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Some(output) = &self.output {
            output.ensure_fresh()?;
        }
        if !self.applied {
            return Err(denied());
        }
        self.released.get().ok_or_else(denied)?.ensure_fresh()?;
        self.admission.effect_fence().ensure_fresh()?;
        if let Some(program) = &self.staged {
            program.ensure_fresh()?;
        }
        for cache in &self.caches {
            cache.ensure_fresh()?;
        }
        for cache in &self.replay_caches {
            cache.ensure_fresh()?;
        }
        for receipt in self.receipts.iter().chain(self.selector.iter()) {
            receipt.ensure_fresh()?;
        }
        for recovery in &self.children {
            recovery.fence.ensure_fresh()?;
        }
        for control in &self.replay_controls {
            control.ensure_fresh()?;
        }
        for fence in &self.replay_fences {
            fence.ensure_fresh()?;
        }
        Ok(())
    }
}

impl PreparedNativeWaitContinuation {
    async fn ensure_prepared_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(output) = &self.output {
            output.ensure_fresh_on(conn).await?;
        }
        let binding = match self.observation.as_ref().ok_or_else(denied)? {
            Observation::Pending(wait) => wait.outcome_binding()?,
            Observation::Released(wait) => wait.outcome_binding()?,
            Observation::Prospective(wait) => wait
                .prepared_input()
                .ok_or_else(denied)?
                .outcome_binding()?,
        };
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if transaction != binding.transaction {
            return Err(denied());
        }
        binding.fence.ensure_fresh_on(conn).await?;
        self.admission.ensure_fresh_on(conn).await?;
        if let Some(program) = &self.staged {
            program.ensure_fresh_on(conn).await?;
        }
        for cache in &self.caches {
            cache.ensure_fresh_on(conn).await?;
        }
        for cache in &self.replay_caches {
            cache.ensure_fresh_on(conn).await?;
        }
        for decision in &self.decisions {
            decision.rendered.ensure_fresh_on(conn).await?;
        }
        for publication in &self.publications {
            for field in &publication.fields {
                field.rendered.ensure_fresh_on(conn).await?;
            }
        }
        for recovery in &self.children {
            recovery.fence.ensure_fresh_on(conn).await?;
            for field in &recovery.child.fields {
                field.rendered.ensure_fresh_on(conn).await?;
            }
        }
        if let Some(selector) = &self.selector {
            selector.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
}

async fn collect_controls(
    conn: &mut PgConnection,
    context: &Context,
    snapshot: Id,
    receipts: &[ReceiptContext],
    controls: &mut BTreeMap<usize, super::super::inherited_input::PreparedInputSource>,
    key: &str,
) -> Result<()> {
    let next = super::program_shape::transitions(&context.task)?;
    for receipt in receipts {
        for index in 0..next.len() {
            let path = format!("{}/next/{index}/when", context.path);
            if receipt.site().execution == context.parent.id
                && receipt.site().kind == InputEntityKind::WorkflowVariables
                && receipt.site().purpose == InputSitePurpose::Condition
                && receipt.site().path == path
            {
                retain_control(conn, context, snapshot, index, receipt, controls, key).await?;
            }
            for descriptor in receipt.source_receipts() {
                if descriptor.site().execution == context.parent.id
                    && descriptor.site().kind == InputEntityKind::WorkflowVariables
                    && descriptor.site().purpose == InputSitePurpose::Condition
                    && descriptor.site().path == path
                {
                    let original = Inputs::prepare_receipt_source(conn, descriptor, key).await?;
                    retain_control(conn, context, snapshot, index, &original, controls, key)
                        .await?;
                }
            }
        }
    }
    Ok(())
}

async fn retain_control(
    conn: &mut PgConnection,
    context: &Context,
    snapshot: Id,
    index: usize,
    receipt: &ReceiptContext,
    controls: &mut BTreeMap<usize, super::super::inherited_input::PreparedInputSource>,
    key: &str,
) -> Result<()> {
    if !receipt
        .native_source_metadata()
        .iter()
        .any(|metadata| metadata.snapshot() == snapshot)
    {
        return Err(denied());
    }
    let source = Inputs::prepare_source(
        conn,
        context.parent.id,
        receipt.site(),
        &receipt.origin(),
        key,
    )
    .await?;
    if controls
        .insert(index, source)
        .is_some_and(|previous| previous.origin() != receipt.origin())
    {
        return Err(denied());
    }
    Ok(())
}

async fn prepare_recovery(
    conn: &mut PgConnection,
    context: &Context,
    name: &str,
    key: &str,
) -> Result<Recovery> {
    let trigger = context
        .task
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    let (child, fence) =
        prepare_wait_child(conn, context, name, key, None, Some(trigger), false, None).await?;
    Ok(Recovery {
        name: name.into(),
        child,
        fence,
    })
}

pub(super) async fn write_recovery_child(
    conn: &mut PgConnection,
    child: &mut Child,
    controls: &[super::super::inherited_input::PreparedInputSource],
    receipts: &mut Vec<ReceiptContext>,
    key: &str,
) -> Result<Execution> {
    Executions::bind_prepared_workflow_task(conn, &child.execution).await?;
    let mut sources = child.defaults.clone();
    for field in &mut child.fields {
        for control in controls {
            Inputs::attach_prepared_destination_control(
                &mut field.rendered,
                &mut field.destination,
                control,
            )?;
        }
        let receipt = Inputs::record_prepared(
            conn,
            &field.rendered,
            &field.destination,
            field.rendered.value(),
            field.rendered.selections(),
            key,
        )
        .await?;
        sources.push(SecretPathSource {
            path: field.path.clone(),
            source: SecretSource::Bound(receipt.provenance()),
        });
        receipts.push(receipt);
    }
    let compiled =
        crate::value_schema::CompiledContract::new(&child.contract).map_err(|_| denied())?;
    let (value, fields) = crate::secret_values::merge_contract_secret_redactions(
        child.value.clone(),
        &sources,
        &compiled,
    )?;
    child.execution = Executions::initialize_config_with_conn(
        conn,
        child.execution.id,
        Some(child.admission),
        child
            .execution
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?,
        Some(value),
        &crate::secret_values::prepare_secret_values(fields, key)?,
    )
    .await?;
    receipts.extend(Materialize::seal_execution_inputs(conn, child.execution.id, key).await?);
    Ok(child.execution.clone())
}

pub(super) async fn prepare_recovery_from_program(
    conn: &mut PgConnection,
    context: &Context,
    name: &str,
    program: &super::staged::Program,
    key: &str,
) -> Result<Recovery> {
    let trigger = context
        .task
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    let (child, fence) = prepare_wait_child(
        conn,
        context,
        name,
        key,
        None,
        Some(trigger),
        false,
        Some(program),
    )
    .await?;
    Ok(Recovery {
        name: name.into(),
        child,
        fence,
    })
}

pub(super) async fn prepare_wait_child(
    conn: &mut PgConnection,
    context: &Context,
    name: &str,
    key: &str,
    observation: Option<&Observation>,
    triggered_by: Option<&str>,
    allow_defaults: bool,
    program: Option<&super::staged::Program>,
) -> Result<(Child, ApprovalEffectFence)> {
    let snapshot = context
        .parent
        .executable_snapshot
        .as_ref()
        .ok_or_else(denied)?;
    let graph = &snapshot
        .executable
        .workflow_definition
        .as_ref()
        .ok_or_else(denied)?
        .definition;
    let task = unique_task(graph, name)?;
    if context
        .workflow
        .completed_tasks
        .iter()
        .chain(&context.workflow.failed_tasks)
        .any(|old| old == name)
        || Executions::find_by_workflow_task(&mut *conn, context.workflow.id, name, None)
            .await?
            .is_some()
    {
        return Err(denied());
    }
    let action = task
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    let path = Materialize::workflow_task_site_path(conn, context.parent.id, name, action).await?;
    let executable =
        Admissions::resolve_workflow_snapshot(conn, context.parent.id, name, action).await?;
    let refs = refs(task, &executable)?;
    let admission =
        Admissions::workflow_child(conn, context.parent.id, name, &executable, &refs).await?;
    let schema = Admissions::workflow_schema(conn, context.parent.id, &executable).await?;
    let child = Executions::create_pinned(
        &mut *conn,
        CreateExecutionInput {
            action: Some(executable.executable.action.id),
            action_ref: action.into(),
            parent: Some(context.parent.id),
            executor: context.parent.executor,
            enforcement: context.parent.enforcement,
            env_vars: context.parent.env_vars.clone(),
            status: ExecutionStatus::Requested,
            config: None,
            permission_set_refs: refs,
            trace_tag: context.parent.trace_tag.clone(),
            timeout_seconds: Some(
                executable
                    .executable
                    .action
                    .timeout_seconds
                    .unwrap_or(crate::config::app_default_execution_timeout_seconds() as i32),
            ),
            artifact_retention_policy: context
                .parent
                .artifact_retention_policy
                .or(executable.executable.action.artifact_retention_policy),
            artifact_retention_limit: context
                .parent
                .artifact_retention_limit
                .or(executable.executable.action.artifact_retention_limit),
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: context.workflow.id,
                task_name: name.into(),
                triggered_by: triggered_by.map(str::to_owned),
                task_index: None,
                task_batch: None,
                retry_count: 0,
                max_retries: 0,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            }),
            ..Default::default()
        },
        &executable,
    )
    .await?;
    let attached = admission
        .attach(conn, RuntimeOwner::Execution(child.id))
        .await?;
    super::super::rule_authority::RuleAuthorityRepository::copy_execution(
        conn,
        context.parent.id,
        child.id,
    )
    .await?;
    super::super::recorded_input_authority::RecordedInputAuthorityRepository::copy(
        conn,
        "execution",
        context.parent.id,
        "execution",
        child.id,
    )
    .await?;
    Schemas::attach(conn, BindingOwner::Execution, child.id, &schema).await?;
    let input_field = if task
        .get("input")
        .is_some_and(|value| value.as_object().is_some_and(|v| !v.is_empty()))
    {
        "input"
    } else {
        "params"
    };
    let authored = task.get(input_field).cloned().unwrap_or_else(|| json!({}));
    let mut value = authored.clone();
    let mut leaves = Vec::new();
    collect_leaves(&authored, String::new(), &mut leaves, 0)?;
    let mut fields = Vec::new();
    for (suffix, leaf) in leaves {
        if task.get(input_field).is_none() {
            break;
        }
        let declaration = format!("{path}/{input_field}{suffix}");
        let rendered = if let Some(program) = program {
            // The successor has its own canonical declaration. Cross-task wait
            // values arrive only through issued publications, never its outcome.
            program
                .render_at(
                    conn,
                    context,
                    &declaration,
                    &leaf,
                    InputSitePurpose::Value,
                    None,
                    key,
                )
                .await?
        } else {
            context
                .render_at(
                    conn,
                    &declaration,
                    &leaf,
                    InputSitePurpose::Value,
                    observation,
                    key,
                )
                .await?
        };
        *value.pointer_mut(&suffix).ok_or_else(denied)? = rendered.value().clone();
        let destination = Inputs::prepare_destination(
            conn,
            &rendered,
            &InputSite {
                execution: child.id,
                kind: InputEntityKind::ExecutionConfig,
                purpose: InputSitePurpose::Value,
                path: suffix.clone(),
            },
            key,
        )
        .await?;
        fields.push(Field {
            path: suffix,
            rendered,
            destination,
        });
    }
    let compiled =
        crate::value_schema::CompiledContract::new(&schema.input).map_err(|_| denied())?;
    let defaulted = compiled.apply_defaults(&value).map_err(|_| denied())?;
    let (value, defaults) = if allow_defaults {
        Materialize::action_defaults(conn, child.id, &value, defaulted, Some(key)).await?
    } else {
        if defaulted != value {
            return Err(denied());
        }
        (value, vec![])
    };
    compiled.validate(&value).map_err(|_| denied())?;
    check_bounds(&value)?;
    Ok((
        Child {
            execution: child,
            admission: attached.id,
            contract: schema.input,
            value,
            fields,
            defaults,
        },
        admission.effect_fence().clone(),
    ))
}
pub(super) fn refs(
    task: &Value,
    snapshot: &crate::models::ExecutionExecutableSnapshot,
) -> Result<Vec<String>> {
    match task.get("permission_set_refs") {
        Some(value) if !value.is_null() => Ok(serde_json::from_value(value.clone())?),
        _ => Ok(snapshot
            .executable
            .action
            .default_execution_permission_set_refs
            .clone()),
    }
}
pub(super) fn unique_task<'a>(graph: &'a Value, name: &str) -> Result<&'a Value> {
    let matching: Vec<_> = graph
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(denied)?
        .iter()
        .filter(|task| task.get("name").and_then(Value::as_str) == Some(name))
        .collect();
    let [task] = matching.as_slice() else {
        return Err(denied());
    };
    Ok(task)
}
pub(super) fn ordinary(task: &Value) -> bool {
    task.get("action")
        .and_then(Value::as_str)
        .is_some_and(|action| !action.contains("{{"))
        && !contains_template(task.get("permission_set_refs").unwrap_or(&Value::Null))
        && [
            "iterate_cache",
            "with_items",
            "retry",
            "timeout",
            "worker_selector",
            "worker_tolerations",
            "worker_affinity",
            "trace_tag_template",
            "join",
        ]
        .iter()
        .all(|field| task.get(*field).is_none_or(Value::is_null))
}
pub(super) fn supported_program(task: &Value, graph: &Value) -> Result<bool> {
    if !ordinary(task) {
        return Ok(false);
    }
    let next = super::program_shape::transitions(task)?;
    let mut recovery_tasks = std::collections::BTreeSet::new();
    let mut publication_leaves = 0usize;
    for transition in next {
        let Some(expression) = transition.get("when").and_then(Value::as_str) else {
            return Ok(false);
        };
        let _ = expression;
        let successors = super::program_shape::successors(transition)?;
        if successors.len() > 128 {
            return Ok(false);
        }
        for (_, _, value) in super::staged::publications(transition)? {
            let mut leaves = Vec::new();
            collect_leaves(value, String::new(), &mut leaves, 0)?;
            publication_leaves = publication_leaves
                .checked_add(leaves.len())
                .ok_or_else(denied)?;
            if publication_leaves > 256 {
                return Ok(false);
            }
            for (_, value) in leaves {
                if let Some(value) = value.as_str().filter(|value| value.contains("{{")) {
                    let names = expression_names(value)?;
                    if calls_outcome(value)? {
                        return Ok(false);
                    }
                    if names
                        .iter()
                        .any(|name| ["result", "item", "index"].contains(&name.as_str()))
                    {
                        return Ok(false);
                    }
                }
            }
        }
        for name in successors {
            let Some(name) = name.as_str() else {
                return Ok(false);
            };
            recovery_tasks.insert(name);
            if recovery_tasks.len() > 128 {
                return Ok(false);
            }
            let successor = unique_task(graph, name)?;
            if successor
                .get("iterate_cache")
                .is_some_and(|value| !value.is_null())
            {
                // B authenticates its full retained task and supported cache
                // profile in the real source-phase bridge, before outer locks.
                continue;
            }
            if !ordinary(successor)
                || successor
                    .get("wait_for")
                    .is_some_and(|value| !value.is_null())
            {
                return Ok(false);
            }
            let Some(input) = successor
                .get("input")
                .or_else(|| successor.get("params"))
                .and_then(Value::as_object)
                .filter(|input| !input.is_empty())
            else {
                return Ok(false);
            };
            let mut leaves = Vec::new();
            collect_leaves(&Value::Object(input.clone()), String::new(), &mut leaves, 0)?;
            if leaves.len() > 256 {
                return Ok(false);
            }
            for (_, leaf) in leaves {
                if let Some(value) = leaf.as_str().filter(|value| value.contains("{{")) {
                    let names = expression_names(value)?;
                    if calls_outcome(value)? {
                        return Ok(false);
                    }
                    if names.iter().any(|name| {
                        [
                            "execution",
                            "inquiry",
                            "work_queue_item",
                            "result",
                            "item",
                            "index",
                        ]
                        .contains(&name.as_str())
                    }) {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(true)
}
fn calls_outcome(expression: &str) -> Result<bool> {
    Ok(
        super::super::native_workflow_materialization::expression_functions(expression)?
            .iter()
            .any(|name| {
                ["succeeded", "failed", "timed_out", "cancelled", "result"].contains(&name.as_str())
            }),
    )
}

fn needs_staged(task: &Value) -> Result<bool> {
    let next = super::program_shape::transitions(task)?;
    let mut written = std::collections::BTreeSet::new();
    for transition in next {
        for (_, name, _) in super::staged::publications(transition)? {
            written.insert(name.to_owned());
        }
    }
    for transition in next {
        if transition.get("publish").is_some_and(Value::is_array) {
            return Ok(true);
        }
        let condition = transition
            .get("when")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .trim();
        let condition = condition
            .strip_prefix("{{")
            .and_then(|value| value.strip_suffix("}}"))
            .unwrap_or(condition)
            .trim();
        if !["succeeded()", "failed()", "timed_out()", "cancelled()"].contains(&condition) {
            return Ok(true);
        }
        for (_, _, authored) in super::staged::publications(transition)? {
            let mut leaves = vec![];
            collect_leaves(authored, String::new(), &mut leaves, 0)?;
            for (_, leaf) in leaves {
                if let Some(template) = leaf.as_str().filter(|value| value.contains("{{")) {
                    if expression_names(template)?.iter().any(|name| {
                        written.contains(name)
                            || ["workflow", "vars", "variables"].contains(&name.as_str())
                    }) {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

fn has_cache_successor(task: &Value, graph: &Value) -> Result<bool> {
    for transition in super::program_shape::transitions(task)? {
        for name in super::program_shape::successors(transition)? {
            if unique_task(graph, name.as_str().ok_or_else(denied)?)?
                .get("iterate_cache")
                .is_some_and(|value| !value.is_null())
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn recovery_reads_publications(task: &Value, graph: &Value) -> Result<bool> {
    let next = super::program_shape::transitions(task)?;
    let mut written = std::collections::BTreeSet::new();
    for transition in next {
        for (_, name, _) in super::staged::publications(transition)? {
            written.insert(name.to_owned());
        }
    }
    for transition in next {
        for name in super::program_shape::successors(transition)? {
            let successor = unique_task(graph, name.as_str().ok_or_else(denied)?)?;
            let mut leaves = vec![];
            collect_leaves(
                successor
                    .get("input")
                    .or_else(|| successor.get("params"))
                    .ok_or_else(denied)?,
                String::new(),
                &mut leaves,
                0,
            )?;
            for (_, leaf) in leaves {
                if let Some(template) = leaf.as_str().filter(|value| value.contains("{{")) {
                    if expression_names(template)?.iter().any(|name| {
                        written.contains(name)
                            || ["workflow", "vars", "variables"].contains(&name.as_str())
                    }) {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

async fn terminal_source(conn: &mut PgConnection, target: WorkflowTaskWaitTarget) -> Result<bool> {
    Ok(match target {
        WorkflowTaskWaitTarget::Execution(id) => matches!(
            Executions::find_by_id(&mut *conn, id)
                .await?
                .ok_or_else(denied)?
                .status,
            ExecutionStatus::Failed
                | ExecutionStatus::Timeout
                | ExecutionStatus::Cancelled
                | ExecutionStatus::Abandoned
        ),
        WorkflowTaskWaitTarget::Inquiry(id) => matches!(
            super::super::inquiry::InquiryRepository::find_by_id(&mut *conn, id)
                .await?
                .ok_or_else(denied)?
                .status,
            crate::models::InquiryStatus::Timeout | crate::models::InquiryStatus::Cancelled
        ),
        WorkflowTaskWaitTarget::WorkQueueItem(id) => matches!(
            super::super::work_queue::WorkQueueItemRepository::find_by_id(&mut *conn, id)
                .await?
                .ok_or_else(denied)?
                .status,
            crate::models::WorkQueueItemStatus::Failed
                | crate::models::WorkQueueItemStatus::Cancelled
                | crate::models::WorkQueueItemStatus::Skipped
        ),
    })
}
