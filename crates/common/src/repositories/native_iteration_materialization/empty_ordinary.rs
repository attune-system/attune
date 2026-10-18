//! A real unindexed ordinary task selected by a sealed empty-source program.
//! It has no list Control, item capability or ordinary-member group witness.
use super::super::native_transition_materialization::PreparedNativeEmptyTransitions;
use super::*;

pub(super) struct PreparedEmptyOrdinary {
    name: String,
    child: Child,
    frame: Arc<PreparedNativeEmptyTransitions>,
    receipts: Vec<ReceiptContext>,
    controls: Vec<super::super::inherited_input::PreparedEmptyTaskControl>,
    sources: Sources,
    source_graph_budget: super::super::inherited_input::PreparedSourceGraphBudget,
    applied: bool,
}

impl PreparedEmptyOrdinary {
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    pub(super) fn value_bytes(&self) -> Result<usize> {
        let bytes = child_materialized_bytes(&self.child)?
            .checked_add(self.source_graph_budget.bytes())
            .ok_or_else(changed)?;
        self.controls.iter().try_fold(bytes, |bytes, control| {
            bytes
                .checked_add(serde_json::to_vec(control.receipt().accepted_value())?.len())
                .ok_or_else(changed)
        })
    }
    pub(super) fn source_graph_budget(
        &self,
    ) -> &super::super::inherited_input::PreparedSourceGraphBudget {
        &self.source_graph_budget
    }
    pub(super) async fn prepare(
        conn: &mut PgConnection,
        parent: &Execution,
        name: &str,
        source: &str,
        frame: Arc<PreparedNativeEmptyTransitions>,
        key: &str,
    ) -> Result<Self> {
        frame.require_source_current(conn).await?;
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(changed)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        let path = frame.target_path(name)?.to_owned();
        let task = graph
            .pointer(path.strip_prefix("/workflow").ok_or_else(changed)?)
            .ok_or_else(changed)?
            .clone();
        if [
            "with_items",
            "iterate_cache",
            "wait_for",
            "tasks",
            "join",
            "retry",
            "batch_size",
            "concurrency",
        ]
        .iter()
        .any(|field| task.get(*field).is_some_and(|value| !value.is_null()))
        {
            return Err(changed());
        }
        if Executions::find_by_workflow_task(&mut *conn, frame.workflow().id, name, None)
            .await?
            .is_some()
        {
            return Err(changed());
        }
        let bindings = Declarations::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let context = Context {
            parent: parent.clone(),
            workflow: frame.workflow().clone(),
            task,
            task_name: name.into(),
            path,
            revision: crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
                .map_err(|_| changed())?,
            bindings,
            sources: prepare_sources(conn, parent, None, frame.workflow(), key)
                .await?
                .ok_or_else(changed)?,
            outcome: None,
            frame: Some(frame.clone()),
            frame_deadline: frame.held_deadline()?,
        };
        let mut controls = Vec::new();
        let action = metadata(conn, &context, &frame, "action", key, &mut controls)
            .await?
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(changed)?;
        let snapshot =
            Admissions::resolve_workflow_snapshot(conn, parent.id, name, &action).await?;
        let refs = match metadata(
            conn,
            &context,
            &frame,
            "permission_set_refs",
            key,
            &mut controls,
        )
        .await?
        {
            Some(value) => permission_refs(value)?,
            None => snapshot
                .executable
                .action
                .default_execution_permission_set_refs
                .clone(),
        };
        let closure =
            super::super::native_input_origin::NativeInputOriginRepository::capture_with_key(
                conn,
                parent.id,
                &[],
                Some(key),
            )
            .await?;
        let admission =
            Admissions::workflow_child_prepared(conn, parent.id, name, &snapshot, &refs, &closure)
                .await?;
        let schema = Admissions::workflow_schema(conn, parent.id, &snapshot).await?;
        let timeout = metadata(conn, &context, &frame, "timeout", key, &mut controls)
            .await?
            .as_ref()
            .map(seconds)
            .transpose()?;
        let selector = metadata(
            conn,
            &context,
            &frame,
            "worker_selector",
            key,
            &mut controls,
        )
        .await?;
        let tolerations = metadata(
            conn,
            &context,
            &frame,
            "worker_tolerations",
            key,
            &mut controls,
        )
        .await?;
        let affinity = metadata(
            conn,
            &context,
            &frame,
            "worker_affinity",
            key,
            &mut controls,
        )
        .await?;
        if let Some(value) = &selector {
            crate::scheduling::parse_worker_selector(value)?;
        }
        if let Some(value) = &tolerations {
            crate::scheduling::parse_worker_tolerations(value)?;
        }
        if let Some(value) = &affinity {
            crate::scheduling::parse_worker_affinity(value)?;
        }
        let trace = metadata(
            conn,
            &context,
            &frame,
            "trace_tag_template",
            key,
            &mut controls,
        )
        .await?;
        let trace_tag = match trace {
            None | Some(Value::Null) => parent.trace_tag.clone(),
            Some(value) => {
                let value = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                if value.trim().is_empty() {
                    parent.trace_tag.clone()
                } else {
                    Some(crate::trace_tag::normalize_trace_tag(&value)?)
                }
            }
        };
        let row = Executions::create_pinned(
            &mut *conn,
            CreateExecutionInput {
                action: Some(snapshot.executable.action.id),
                action_ref: action,
                parent: Some(parent.id),
                config: None,
                env_vars: parent.env_vars.clone(),
                enforcement: parent.enforcement,
                executor: parent.executor,
                permission_set_refs: refs,
                status: ExecutionStatus::Requested,
                trace_tag,
                timeout_seconds: Some(
                    timeout
                        .or(snapshot.executable.action.timeout_seconds)
                        .unwrap_or(crate::config::app_default_execution_timeout_seconds() as i32),
                ),
                worker_selector: selector,
                worker_tolerations: tolerations,
                worker_affinity: affinity,
                artifact_retention_policy: parent
                    .artifact_retention_policy
                    .or(snapshot.executable.action.artifact_retention_policy),
                artifact_retention_limit: parent
                    .artifact_retention_limit
                    .or(snapshot.executable.action.artifact_retention_limit),
                workflow_task: Some(WorkflowTaskMetadata {
                    workflow_execution: context.workflow.id,
                    task_name: name.into(),
                    task_index: None,
                    task_batch: None,
                    triggered_by: Some(source.into()),
                    retry_count: 0,
                    max_retries: 0,
                    next_retry_at: None,
                    timeout_seconds: timeout,
                    timed_out: false,
                    duration_ms: None,
                    started_at: None,
                    completed_at: None,
                }),
                ..Default::default()
            },
            &snapshot,
        )
        .await?;
        let attached = admission
            .attach(conn, RuntimeOwner::Execution(row.id))
            .await?;
        super::super::rule_authority::RuleAuthorityRepository::copy_execution(
            conn, parent.id, row.id,
        )
        .await?;
        super::super::recorded_input_authority::RecordedInputAuthorityRepository::copy(
            conn,
            "execution",
            parent.id,
            "execution",
            row.id,
        )
        .await?;
        Schemas::attach(conn, BindingOwner::Execution, row.id, &schema).await?;
        let input = if context
            .task
            .get("input")
            .and_then(Value::as_object)
            .is_some()
        {
            "input"
        } else {
            "params"
        };
        let authored = context
            .task
            .get(input)
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !authored.is_object() {
            return Err(changed());
        }
        let mut value = authored.clone();
        let mut leaves = Vec::new();
        collect_leaves(&authored, String::new(), &mut leaves, 0)?;
        let mut fields = Vec::new();
        for (path, _) in leaves {
            let mut rendered = context
                .render(
                    conn,
                    &format!("/{input}{path}"),
                    InputSitePurpose::Value,
                    None,
                    key,
                )
                .await?;
            *value.pointer_mut(&path).ok_or_else(changed)? = rendered.value().clone();
            let (mut destination, mut graph_budget) =
                Inputs::prepare_destination_with_graph_budget(
                    conn,
                    &rendered,
                    &InputSite {
                        execution: row.id,
                        kind: InputEntityKind::ExecutionConfig,
                        purpose: InputSitePurpose::Value,
                        path: path.clone(),
                    },
                    key,
                )
                .await?;
            for control in &controls {
                let control =
                    Inputs::prepare_recorded_source(conn, parent.id, control.receipt(), key)
                        .await?;
                Inputs::attach_prepared_destination_control(
                    &mut rendered,
                    &mut destination,
                    &control,
                )?;
                Inputs::include_prepared_control_graph_budget(
                    conn,
                    &mut graph_budget,
                    &control,
                    key,
                )
                .await?;
            }
            fields.push(Field {
                graph_budget,
                path,
                rendered,
                destination,
            });
        }
        let contract =
            crate::value_schema::CompiledContract::new(&schema.input).map_err(|_| changed())?;
        let defaults = contract.apply_defaults(&value).map_err(|_| changed())?;
        let (value, defaults) =
            Materialize::action_defaults(conn, row.id, &value, defaults, Some(key)).await?;
        contract.validate(&value).map_err(|_| changed())?;
        check_bounds(&value)?;
        if serde_json::to_vec(&value)?.len() > MAX_MATERIALIZED_BYTES {
            return Err(changed());
        }
        let mut receipts = Vec::new();
        for default in &defaults {
            if let SecretSource::Bound(provenance) = &default.source {
                for origin in &provenance.origins {
                    receipts.push(
                        Inputs::receipt_context(
                            conn,
                            &InputSite {
                                execution: row.id,
                                kind: InputEntityKind::ExecutionConfig,
                                purpose: InputSitePurpose::Value,
                                path: default.path.clone(),
                            },
                            origin,
                            key,
                        )
                        .await?,
                    );
                }
            }
        }
        let contexts: Vec<_> = controls
            .iter()
            .map(|control| control.receipt())
            .chain(receipts.iter())
            .collect();
        let destinations: Vec<_> = fields.iter().map(|field| &field.graph_budget).collect();
        let source_graph_budget =
            Inputs::prepared_input_graph_budget(conn, &destinations, &contexts, key).await?;
        Ok(Self {
            name: name.into(),
            child: Child {
                row,
                admission,
                admission_id: attached.id,
                contract: schema.input,
                value,
                fields,
                defaults,
            },
            frame,
            receipts,
            controls,
            sources: context.sources,
            source_graph_budget,
            applied: false,
        })
    }
    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_deadline_on(conn, self.held_deadline()?).await?;
        self.frame.ensure_fresh_on(conn).await?;
        self.child.admission.ensure_fresh_on(conn).await?;
        self.sources
            .system
            .held_effect_fence()
            .ensure_fresh_on(conn)
            .await?;
        for source in self
            .sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
        {
            source.ensure_fresh_on(conn).await?;
        }
        for control in &self.controls {
            control.ensure_fresh_on(conn).await?;
        }
        for field in &self.child.fields {
            field.rendered.ensure_fresh_on(conn).await?;
            field.destination.ensure_fresh_on(conn).await?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub(super) fn ensure_fresh(&self) -> Result<()> {
        require_deadline(self.held_deadline()?)?;
        self.frame.ensure_fresh()?;
        self.child.admission.effect_fence().ensure_fresh()?;
        self.sources.system.held_effect_fence().ensure_fresh()?;
        for source in self
            .sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
        {
            source.ensure_fresh()?;
        }
        for control in &self.controls {
            control.ensure_fresh()?;
        }
        for field in &self.child.fields {
            field.rendered.ensure_fresh()?;
            field.destination.ensure_fresh()?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
    pub(super) fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let mut dates = vec![
            self.frame.held_deadline()?,
            self.sources.held_deadline()?,
            self.child.admission.effect_fence().approval_deadline(),
        ];
        for field in &self.child.fields {
            dates.push(field.rendered.approval_deadline());
            dates.push(field.destination.held_deadline()?);
        }
        for control in &self.controls {
            dates.push(control.held_deadline()?);
        }
        dates.extend(self.receipts.iter().map(ReceiptContext::approval_deadline));
        Ok(dates.into_iter().flatten().min())
    }
    pub(super) async fn apply(&mut self, conn: &mut PgConnection, key: &str) -> Result<Execution> {
        if self.applied {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        self.frame.require_published(conn).await?;
        self.applied = true;
        let child = &mut self.child;
        Executions::bind_prepared_workflow_task(conn, &child.row).await?;
        for control in &self.controls {
            control.publish(conn, child.row.id).await?;
        }
        let mut sources = child.defaults.clone();
        for field in &child.fields {
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
        let contract =
            crate::value_schema::CompiledContract::new(&child.contract).map_err(|_| changed())?;
        let (value, secrets) = crate::secret_values::merge_contract_secret_redactions(
            child.value.clone(),
            &sources,
            &contract,
        )?;
        let secrets = crate::secret_values::prepare_secret_values(secrets, key)?;
        child.row = Executions::initialize_config_with_conn(
            conn,
            child.row.id,
            Some(child.admission_id),
            child.row.executable_snapshot.as_ref().ok_or_else(changed)?,
            Some(value),
            &secrets,
        )
        .await?;
        self.receipts
            .extend(Materialize::seal_execution_inputs(conn, child.row.id, key).await?);
        self.ensure_fresh_on(conn).await?;
        Ok(self.child.row.clone())
    }
}

pub(super) fn child_materialized_bytes(child: &Child) -> Result<usize> {
    // These are the actual resolved/defaulted row values, including literal
    // placement and trace overrides. No expression or schema is evaluated here.
    let metadata = json!({
        "action":child.row.action_ref,
        "permission_set_refs":child.row.permission_set_refs,
        "timeout":child.row.timeout_seconds,
        "worker_selector":child.row.worker_selector,
        "worker_tolerations":child.row.worker_tolerations,
        "worker_affinity":child.row.worker_affinity,
        "trace_tag":child.row.trace_tag,
        "workflow_task":child.row.workflow_task,
        "env_vars":child.row.env_vars,
        "artifact_retention_policy":child.row.artifact_retention_policy,
        "artifact_retention_limit":child.row.artifact_retention_limit,
    });
    serde_json::to_vec(&child.value)?
        .len()
        .checked_add(serde_json::to_vec(&metadata)?.len())
        .ok_or_else(changed)
}

pub(super) struct RecoveredEmptyOrdinary {
    pub(super) row: Execution,
    input: ObservedInputState,
    fence: ApprovalEffectFence,
    receipts: Vec<ReceiptContext>,
}
impl RecoveredEmptyOrdinary {
    pub(super) fn held_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        std::iter::once(self.fence.approval_deadline())
            .chain(self.receipts.iter().map(ReceiptContext::approval_deadline))
            .flatten()
            .min()
    }
    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_dispatch(conn, &self.row).await?;
        self.input.require_current(conn).await?;
        self.fence.ensure_fresh_on(conn).await?;
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub(super) fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
}

pub(super) async fn recover(
    conn: &mut PgConnection,
    parent: &Execution,
    workflow: Id,
    source: &str,
    key: &str,
) -> Result<Vec<RecoveredEmptyOrdinary>> {
    let invalid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_task_dispatch d LEFT JOIN execution e ON e.id=d.execution_id WHERE d.workflow_execution=$1 AND d.task_index IS NULL AND (e.id IS NULL OR e.parent IS DISTINCT FROM $2 OR e.workflow_task->>'workflow_execution' IS DISTINCT FROM $1::TEXT OR e.workflow_task->>'task_name' IS DISTINCT FROM d.task_name OR e.workflow_task->>'task_index' IS NOT NULL))")
        .bind(workflow).bind(parent.id).fetch_one(&mut *conn).await?;
    if invalid {
        return Err(changed());
    }
    // Start with attributed executions, not an inner join that silently drops a
    // missing or tampered dispatch. Never manufacture a replacement child.
    let rows: Vec<(Id, Option<Id>, Option<String>)> = sqlx::query_as(
        "SELECT e.id,d.execution_id,d.task_name FROM execution e LEFT JOIN workflow_task_dispatch d ON d.workflow_execution=$1 AND d.task_index IS NULL AND d.execution_id=e.id WHERE e.parent=$2 AND e.workflow_task->>'triggered_by'=$3 AND e.workflow_task->>'task_index' IS NULL ORDER BY e.id LIMIT 257",
    ).bind(workflow).bind(parent.id).bind(source).fetch_all(&mut *conn).await?;
    if rows.len() > 256 {
        return Err(changed());
    }
    let mut names = BTreeSet::new();
    let mut recovered = Vec::new();
    for (id, dispatch, task_name) in rows {
        let row = Executions::find_by_id(&mut *conn, id)
            .await?
            .ok_or_else(changed)?;
        let metadata = row.workflow_task.as_ref().ok_or_else(changed)?;
        if row.parent != Some(parent.id)
            || dispatch != Some(id)
            || task_name.as_deref() != Some(metadata.task_name.as_str())
            || metadata.workflow_execution != workflow
            || metadata.triggered_by.as_deref() != Some(source)
            || metadata.task_index.is_some()
            || metadata.task_batch.is_some()
            || metadata.retry_count != 0
            || row.original_execution.is_some()
            || !names.insert(metadata.task_name.clone())
        {
            return Err(changed());
        }
        let (input, fence, receipts) = observe_accepted_inputs(conn, &row, key).await?;
        recovered.push(RecoveredEmptyOrdinary {
            row,
            input,
            fence,
            receipts,
        });
    }
    Ok(recovered)
}

async fn require_dispatch(conn: &mut PgConnection, row: &Execution) -> Result<()> {
    let metadata = row.workflow_task.as_ref().ok_or_else(changed)?;
    let dispatch: Option<Id> = sqlx::query_scalar("SELECT execution_id FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name=$2 AND task_index IS NULL")
        .bind(metadata.workflow_execution).bind(&metadata.task_name).fetch_optional(conn).await?;
    if dispatch != Some(row.id) {
        return Err(changed());
    }
    Ok(())
}

pub(super) fn require_deadline(date: Option<chrono::DateTime<chrono::Utc>>) -> Result<()> {
    if date.is_some_and(|date| date <= chrono::Utc::now()) {
        return Err(changed());
    }
    Ok(())
}
pub(super) async fn require_deadline_on(
    conn: &mut PgConnection,
    date: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<()> {
    let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(conn)
        .await?;
    if date.is_some_and(|date| date <= now) {
        return Err(changed());
    }
    Ok(())
}

pub(super) fn iteration_deadline(
    source: &PreparedNativeIteration,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let mut dates = vec![
        source.parent_fence.approval_deadline(),
        source.context.sources.held_deadline()?,
        source.context.frame_deadline,
    ];
    dates.extend(
        source
            .context
            .outcome
            .iter()
            .map(PreparedWorkflowOutcome::approval_deadline),
    );
    dates.extend(
        source
            .array
            .iter()
            .map(RenderedReceiptContext::approval_deadline),
    );
    dates.extend(
        source
            .iteration_control
            .iter()
            .map(PreparedIterationControl::held_deadline),
    );
    dates.extend(
        source
            .empty_control
            .iter()
            .map(ReceiptContext::approval_deadline),
    );
    if let Some(outcome) = &source.empty_outcome {
        dates.push(outcome.held_deadline()?);
    }
    dates.extend(
        source
            .controls
            .iter()
            .chain(&source.receipts)
            .map(ReceiptContext::approval_deadline),
    );
    dates.extend(
        source
            .recovery_fences
            .iter()
            .map(ApprovalEffectFence::approval_deadline),
    );
    dates.extend(
        source
            .observed_outcomes
            .iter()
            .chain(&source.original_outcomes)
            .map(PreparedWorkflowOutcome::approval_deadline),
    );
    for child in &source.children {
        dates.push(child.admission.effect_fence().approval_deadline());
        for field in &child.fields {
            dates.push(field.rendered.approval_deadline());
            dates.push(field.destination.held_deadline()?);
        }
    }
    Ok(dates.into_iter().flatten().min())
}

async fn metadata(
    conn: &mut PgConnection,
    context: &Context,
    frame: &PreparedNativeEmptyTransitions,
    name: &str,
    key: &str,
    receipts: &mut Vec<super::super::inherited_input::PreparedEmptyTaskControl>,
) -> Result<Option<Value>> {
    let Some(authored) = context.task.get(name).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let mut value = authored.clone();
    let mut leaves = Vec::new();
    collect_leaves(authored, String::new(), &mut leaves, 0)?;
    for (path, leaf) in leaves {
        if !contains_template(&leaf) {
            continue;
        }
        let relative = format!("/{name}{path}");
        let rendered = context
            .render(conn, &relative, InputSitePurpose::Control, None, key)
            .await?;
        *value.pointer_mut(&path).ok_or_else(changed)? = rendered.value().clone();
        receipts.push(
            Inputs::prepare_empty_task_control(
                conn,
                &rendered,
                &InputSite {
                    execution: context.parent.id,
                    kind: InputEntityKind::WorkflowVariables,
                    purpose: InputSitePurpose::Control,
                    path: format!("{}{relative}", context.path),
                },
                frame,
                &context.task_name,
                key,
            )
            .await?,
        );
        let bytes = receipts.iter().try_fold(0usize, |bytes, receipt| {
            bytes
                .checked_add(receipt.materialized_bytes()?)
                .ok_or_else(changed)
        })?;
        if bytes > MAX_MATERIALIZED_BYTES {
            return Err(changed());
        }
    }
    Ok(Some(value))
}
