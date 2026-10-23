//! The initial caller prepares before registration's workflow FK, not by
//! inserting a bootstrap wait and coming back in another transaction.
use super::super::native_input_snapshot::RegisteredInitialWait;
use super::*;
use crate::repositories::Update;

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve the inline single-child outcome distinct from recovered child collections"
)]
pub enum InitialWaitApply {
    Registered(Option<Execution>),
    Recovered(Vec<Execution>),
    /// The caller must roll back this transaction's speculative child/snapshot.
    Existing,
}

pub struct PreparedNativeInitialWait {
    context: Context,
    original_secrets: Vec<StoredSecretValue>,
    observation: Option<Observation>,
    bound: std::sync::Arc<OnceLock<RegisteredInitialWait>>,
    admission: ExecutionAdmission,
    child: Option<Child>,
    child_fence: Option<ApprovalEffectFence>,
    selector: Option<ReceiptContext>,
    receipts: Vec<ReceiptContext>,
    duplicate_only: bool,
    recovery: Option<super::recovery::Recovery>,
}

pub struct NativeInitialWaitMaterializationRepository;
impl NativeInitialWaitMaterializationRepository {
    pub async fn prepare(
        conn: &mut PgConnection,
        workflow: Id,
        task_name: &str,
        key: Option<&str>,
        triggered_by: Option<&str>,
    ) -> Result<Option<PreparedNativeInitialWait>> {
        Self::prepare_with_predecessor(conn, workflow, task_name, key, triggered_by, None).await
    }

    pub async fn prepare_successor(
        conn: &mut PgConnection,
        predecessor: std::sync::Arc<
            super::super::native_transition_materialization::PreparedNativeWaitPredecessor,
        >,
        key: &str,
    ) -> Result<PreparedNativeInitialWait> {
        predecessor.ensure_fresh_on(conn).await?;
        let task_name = predecessor.target().to_owned();
        let triggered_by = predecessor
            .completed()
            .workflow_task
            .as_ref()
            .ok_or_else(denied)?
            .task_name
            .clone();
        let workflow = predecessor.original().id;
        Self::prepare_with_predecessor(
            conn,
            workflow,
            &task_name,
            Some(key),
            Some(&triggered_by),
            Some(predecessor),
        )
        .await?
        .ok_or_else(denied)
    }

    async fn prepare_with_predecessor(
        conn: &mut PgConnection,
        workflow: Id,
        task_name: &str,
        key: Option<&str>,
        triggered_by: Option<&str>,
        predecessor: Option<
            std::sync::Arc<
                super::super::native_transition_materialization::PreparedNativeWaitPredecessor,
            >,
        >,
    ) -> Result<Option<PreparedNativeInitialWait>> {
        let workflow = Workflows::find_by_id(&mut *conn, workflow)
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
        if Waits::find_by_workflow_task(&mut *conn, workflow.id, task_name)
            .await?
            .is_some()
        {
            return Err(Error::InvalidState(
                "Initial native wait already registered; use the registered checkpoint".into(),
            ));
        }
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(denied)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        let task = continuation::unique_task(graph, task_name)?;
        if !continuation::ordinary(task) || task.get("wait_for").is_none_or(Value::is_null) {
            return Ok(None);
        }
        let action = task
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        let path =
            Materialize::workflow_task_site_path(conn, parent.id, task_name, &action).await?;
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| denied())?;
        let task = task.clone();
        let key = key.ok_or_else(denied)?;
        let sources = prepare_sources(conn, &parent, &task, &workflow, key)
            .await?
            .ok_or_else(denied)?;
        let declarations = Declarations::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let workflow = if let Some(predecessor) = &predecessor {
            if serde_json::to_value(&workflow)? != serde_json::to_value(predecessor.original())?
                || serde_json::to_value(&parent)? != serde_json::to_value(predecessor.parent())?
                || task_name != predecessor.target()
            {
                return Err(denied());
            }
            predecessor.projected().clone()
        } else {
            workflow
        };
        let context = Context {
            predecessor,
            parent,
            workflow,
            task,
            path,
            revision,
            sources: sources.into(),
            declarations,
        };
        let executable =
            Admissions::resolve_workflow_snapshot(conn, context.parent.id, task_name, &action)
                .await?;
        let refs = continuation::refs(&context.task, &executable)?;
        let admission =
            Admissions::workflow_child(conn, context.parent.id, task_name, &executable, &refs)
                .await?;
        let wait_for: crate::workflow::TaskWaitFor =
            serde_json::from_value(context.task.get("wait_for").cloned().ok_or_else(denied)?)?;
        let (kind, target) = wait_for.target();
        let selector = if contains_template(target) {
            let relative = format!("/wait_for/{kind}");
            let rendered = context
                .render(conn, &relative, InputSitePurpose::Control, None, key)
                .await?;
            let destination = Inputs::prepare_destination(
                conn,
                &rendered,
                &InputSite {
                    execution: context.parent.id,
                    kind: InputEntityKind::WorkflowVariables,
                    purpose: InputSitePurpose::Control,
                    path: format!("{}{relative}", context.path),
                },
                key,
            )
            .await?;
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
        };
        let id = selector
            .as_ref()
            .map(|control| control.accepted_value())
            .unwrap_or(target)
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(denied)?;
        let target = match kind {
            "execution" => crate::models::WorkflowTaskWaitTarget::Execution(id),
            "inquiry" => crate::models::WorkflowTaskWaitTarget::Inquiry(id),
            "work_queue_item" => crate::models::WorkflowTaskWaitTarget::WorkQueueItem(id),
            _ => return Err(denied()),
        };
        let mut registration = Snapshots::prepare_initial_wait(
            conn,
            context.parent.id,
            context.workflow.id,
            &context.path,
            &admission,
            target,
            selector.as_ref(),
            key,
        )
        .await?;
        if let Some(predecessor) = &context.predecessor {
            registration.bind_predecessor_projection(predecessor)?;
        }
        let recover = registration
            .state()
            .is_some_and(|state| state != "released");
        let terminal = registration.state().is_some();
        let observation = Observation::Prospective(registration);
        let duplicate_only =
            Waits::find_by_workflow_task(&mut *conn, context.workflow.id, task_name)
                .await?
                .is_some();
        let bound = std::sync::Arc::new(OnceLock::new());
        let recovery = if recover && !duplicate_only {
            Some(
                super::recovery::Recovery::prepare(
                    conn,
                    &context,
                    &observation,
                    bound.clone(),
                    key,
                )
                .await
                .map_err(|error| match error {
                    Error::PermissionDenied(message) => {
                        Error::PermissionDenied(format!("Initial Wait recovery: {message}"))
                    }
                    other => other,
                })?,
            )
        } else {
            None
        };
        let (child, child_fence) = if terminal && !recover && !duplicate_only {
            let (child, fence) = continuation::prepare_wait_child(
                conn,
                &context,
                task_name,
                key,
                Some(&observation),
                triggered_by,
                true,
                None,
            )
            .await?;
            (Some(child), Some(fence))
        } else {
            (None, None)
        };
        let original_secrets = Secrets::find_stored_by_entity(
            &mut *conn,
            ENTITY_WORKFLOW_VARIABLES,
            context.parent.id,
        )
        .await?;
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
        Ok(Some(PreparedNativeInitialWait {
            context,
            original_secrets,
            observation: Some(observation),
            bound,
            admission,
            child,
            child_fence,
            selector,
            receipts: vec![],
            duplicate_only,
            recovery,
        }))
    }
}

impl PreparedNativeInitialWait {
    pub fn completions(&self) -> &[Execution] {
        self.recovery
            .as_ref()
            .map(|recovery| recovery.completions())
            .unwrap_or(&[])
    }
    pub fn workflow(&self) -> Id {
        self.context.workflow.id
    }
    pub fn terminal_wait(&self) -> Option<Id> {
        self.recovery.as_ref()?;
        Some(self.bound.get()?.wait().id)
    }
    pub async fn apply(&mut self, conn: &mut PgConnection, key: &str) -> Result<InitialWaitApply> {
        if let Some(predecessor) = &self.context.predecessor {
            predecessor.ensure_fresh_on(conn).await?;
        }
        if self.bound.get().is_some() {
            return Err(denied());
        }
        if let Some(recovery) = &self.recovery {
            recovery.guard_original(conn, &self.context).await?;
        }
        let Observation::Prospective(registration) = self.observation.take().ok_or_else(denied)?
        else {
            return Err(denied());
        };
        let bound = registration.bind(conn).await?;
        if !bound.created() {
            self.bound.set(bound).map_err(|_| denied())?;
            return Ok(InitialWaitApply::Existing);
        }
        if self.duplicate_only {
            return Err(denied());
        }
        let workflow = Workflows::find_by_id(&mut *conn, self.context.workflow.id)
            .await?
            .ok_or_else(denied)?;
        if serde_json::to_value(&workflow)? != serde_json::to_value(&self.context.workflow)?
            || Secrets::find_stored_by_entity(
                &mut *conn,
                ENTITY_WORKFLOW_VARIABLES,
                self.context.parent.id,
            )
            .await?
                != self.original_secrets
        {
            return Err(denied());
        }
        self.bound.set(bound).map_err(|_| denied())?;
        if let Some(recovery) = &mut self.recovery {
            let state = self
                .bound
                .get()
                .and_then(|bound| bound.released())
                .ok_or_else(denied)?
                .state();
            if !["failed", "timed_out", "cancelled"].contains(&state) {
                return Err(denied());
            }
            let pending = recovery
                .apply(conn, &self.context, &self.original_secrets, key)
                .await?;
            self.ensure_fresh_on(conn).await?;
            return Ok(InitialWaitApply::Recovered(pending));
        }
        let child = if let Some(child) = &mut self.child {
            if self
                .bound
                .get()
                .and_then(|bound| bound.released())
                .is_none()
            {
                return Err(denied());
            }
            Executions::bind_prepared_workflow_task(conn, &child.execution).await?;
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
            let compiled = crate::value_schema::CompiledContract::new(&child.contract)
                .map_err(|_| denied())?;
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
            self.receipts
                .extend(Materialize::seal_execution_inputs(conn, child.execution.id, key).await?);
            Some(child.execution.clone())
        } else {
            None
        };
        let mut current = workflow.current_tasks;
        let name = self
            .context
            .task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        if !current.iter().any(|task| task == name) {
            current.push(name.into());
        }
        Workflows::update(
            &mut *conn,
            workflow.id,
            super::super::workflow::UpdateWorkflowExecutionInput {
                current_tasks: Some(current),
                ..Default::default()
            },
        )
        .await?;
        self.ensure_fresh_on(conn).await?;
        Ok(InitialWaitApply::Registered(child))
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(predecessor) = &self.context.predecessor {
            predecessor.ensure_fresh_on(conn).await?;
        }
        self.bound
            .get()
            .ok_or_else(denied)?
            .ensure_fresh_on(conn)
            .await?;
        self.admission.ensure_fresh_on(conn).await?;
        if let Some(recovery) = &self.recovery {
            recovery.ensure_fresh_on(conn).await?;
        }
        if let Some(fence) = &self.child_fence {
            fence.ensure_fresh_on(conn).await?;
        }
        for receipt in self.receipts.iter().chain(self.selector.iter()) {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Some(predecessor) = &self.context.predecessor {
            predecessor.ensure_fresh()?;
        }
        self.bound.get().ok_or_else(denied)?.ensure_fresh()?;
        self.admission.effect_fence().ensure_fresh()?;
        if let Some(recovery) = &self.recovery {
            recovery.ensure_fresh()?;
        }
        if let Some(fence) = &self.child_fence {
            fence.ensure_fresh()?;
        }
        for receipt in self.receipts.iter().chain(self.selector.iter()) {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
}
