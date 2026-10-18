//! Source-first delivery of already registered native waits.
//! Source observation, task admission and input preparation precede outer locks.
use super::{
    declared_input::{DeclaredInputBinding, DeclaredInputRepository as Declarations},
    declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
    execution::{CreateExecutionInput, ExecutionRepository as Executions},
    execution_runtime_admission::{ExecutionAdmission, ExecutionAdmissionRepository as Admissions},
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        PreparedReceiptDestination, ReceiptContext, RenderedReceiptContext,
    },
    native_input_snapshot::{
        NativeInputSnapshotRepository as Snapshots, PreparedWaitInput, ReleasedWaitInput,
    },
    native_transition_materialization::{prepare_sources, Sources},
    native_workflow_materialization::{
        check_bounds, collect_leaves, contains_template, expression_names,
    },
    runtime_authority::{
        ApprovalEffectFence, RuntimeAuthorityRepository as Authority, RuntimeOwner,
    },
    schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
    workflow::WorkflowExecutionRepository as Workflows,
    workflow_task_wait::WorkflowTaskWaitRepository as Waits,
    FindById,
};
use crate::{
    models::{
        Execution, ExecutionStatus, Id, WorkflowExecution, WorkflowTaskMetadata, WorkflowTaskWait,
        WorkflowTaskWaitState, WorkflowTaskWaitTarget,
    },
    secret_values::{SecretPathSource, SecretSource, StoredSecretValue, ENTITY_WORKFLOW_VARIABLES},
    Error, Result,
};
use serde_json::{json, Value};
use sqlx::PgConnection;
use std::{collections::BTreeMap, sync::OnceLock};

mod cache_frame;
mod cache_replay;
mod completion;
mod continuation;
mod initial;
mod program_shape;
mod recovery;
pub use completion::{PreparedNativeWaitCompletion, WaitCompletionOutcome};
mod completion_replay;
mod completion_writer;
pub use completion_replay::{
    NativeWaitCompletionReplayRepository, PreparedNativeWaitCompletionReplay,
};
mod staged;
pub use continuation::{NativeWaitContinuationRepository, PreparedNativeWaitContinuation};
pub use initial::{
    InitialWaitApply, NativeInitialWaitMaterializationRepository, PreparedNativeInitialWait,
};

#[expect(
    clippy::large_enum_variant,
    reason = "Retain distinct owned pending, released, and prospective wait witnesses inline"
)]
enum Observation {
    Pending(PreparedWaitInput),
    Released(ReleasedWaitInput),
    Prospective(super::native_input_snapshot::PreparedWaitRegistration),
}
impl Observation {
    fn bind(&self, session: &mut super::inherited_input::RenderSession) -> Result<()> {
        match self {
            Self::Pending(wait) => Inputs::bind_prepared_wait_input(session, wait),
            Self::Released(wait) => Inputs::bind_wait_input(session, wait),
            Self::Prospective(wait) => wait.bind_input(session),
        }
    }
    fn bind_outcome(&self, session: &mut super::inherited_input::RenderSession) -> Result<()> {
        match self {
            Self::Pending(wait) => Inputs::bind_prepared_wait_outcome(session, wait),
            Self::Released(wait) => Inputs::bind_wait_outcome(session, wait),
            Self::Prospective(wait) => wait.bind_outcome(session),
        }
    }
}
struct Context {
    predecessor: Option<
        std::sync::Arc<super::native_transition_materialization::PreparedNativeWaitPredecessor>,
    >,
    parent: Execution,
    workflow: WorkflowExecution,
    task: Value,
    path: String,
    revision: String,
    declarations: BTreeMap<String, DeclaredInputBinding>,
    sources: std::sync::Arc<Sources>,
}
struct Field {
    path: String,
    rendered: RenderedReceiptContext,
    destination: PreparedReceiptDestination,
}
struct Child {
    execution: Execution,
    admission: Id,
    contract: crate::value_schema::Contract,
    value: Value,
    fields: Vec<Field>,
    defaults: Vec<SecretPathSource>,
}
pub struct PreparedNativeWait {
    context: Context,
    original: WorkflowTaskWait,
    original_secrets: Vec<StoredSecretValue>,
    observation: Option<Observation>,
    admission: ExecutionAdmission,
    selector: Option<ReceiptContext>,
    child: Option<Child>,
    existing: Option<Execution>,
    recovery_fence: Option<ApprovalEffectFence>,
    receipts: Vec<ReceiptContext>,
    released: OnceLock<ReleasedWaitInput>,
}
impl Context {
    async fn render(
        &self,
        conn: &mut PgConnection,
        relative: &str,
        purpose: InputSitePurpose,
        observation: Option<&Observation>,
        key: &str,
    ) -> Result<RenderedReceiptContext> {
        let path = format!("{}{relative}", self.path);
        let authored = self.task.pointer(relative).ok_or_else(denied)?;
        self.render_at(conn, &path, authored, purpose, observation, key)
            .await
    }

    async fn render_at(
        &self,
        conn: &mut PgConnection,
        path: &str,
        authored: &Value,
        purpose: InputSitePurpose,
        observation: Option<&Observation>,
        key: &str,
    ) -> Result<RenderedReceiptContext> {
        let Some(binding) = self.declarations.get(path) else {
            if purpose != InputSitePurpose::Value || contains_template(authored) {
                return Err(denied());
            }
            let mut rendered = Inputs::literal(conn, self.parent.id, &self.revision, path).await?;
            if let Some(predecessor) = &self.predecessor {
                predecessor.attach_rendered_control(&mut rendered)?;
            }
            return Ok(rendered);
        };
        let mut session = Inputs::begin_render(
            conn,
            self.parent.id,
            &binding.declaration_revision,
            path,
            &binding.expression,
            purpose,
        )
        .await?;
        let names = expression_names(&binding.expression)?;
        // The predecessor seals the handoff, not this task's expression view.
        // In particular its outcome must never replace the Wait observation.
        for field in &self.sources.fields {
            for slot in &field.paths {
                let namespace = slot
                    .split('/')
                    .nth(1)
                    .unwrap_or_default()
                    .replace("~1", "/")
                    .replace("~0", "~");
                if !names.contains(&namespace) {
                    continue;
                }
                if let Some(source) = &field.source {
                    Inputs::bind_prepared(&mut session, slot, source, source.value())?;
                } else {
                    Inputs::bind_public_field(conn, &mut session, slot, &field.site).await?;
                }
            }
        }
        if let Some(observation) = observation {
            observation.bind(&mut session)?;
            if purpose == InputSitePurpose::Condition {
                observation.bind_outcome(&mut session)?;
            }
        }
        if names.contains("system") {
            Inputs::bind_prepared_system(&mut session, &self.sources.system)?;
        }
        if binding.uses_pack_configuration() {
            Inputs::bind_configuration(conn, &mut session, key).await?;
        }
        for source in self
            .sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
        {
            Inputs::attach_prepared_control(&mut session, source)?;
        }
        let mut rendered = if purpose == InputSitePurpose::Condition {
            Inputs::evaluate_condition_with_keys(conn, session, key).await?
        } else {
            Inputs::evaluate_with_keys(conn, session, key).await?
        };
        if let Some(predecessor) = &self.predecessor {
            predecessor.attach_rendered_control(&mut rendered)?;
        }
        Ok(rendered)
    }
}

pub struct NativeWaitMaterializationRepository;
impl NativeWaitMaterializationRepository {
    /// This delivery slice handles successful execution/inquiry/queue waits.
    /// Terminal wait outcomes, dynamic metadata and initial wait creation remain
    /// with their caller owners; selected proof failures never become fallbacks.
    pub async fn prepare(
        conn: &mut PgConnection,
        wait: &WorkflowTaskWait,
        key: Option<&str>,
    ) -> Result<Option<PreparedNativeWait>> {
        let target = wait.target()?;
        if !matches!(
            wait.state,
            WorkflowTaskWaitState::Waiting | WorkflowTaskWaitState::Released
        ) {
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
        if parent.status != ExecutionStatus::Running || workflow.status != ExecutionStatus::Running
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
        // Resolve only a literal action here. A dynamic action needs its own
        // source-prepared metadata path, never a post-lock template evaluation.
        let tasks = graph
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(denied)?;
        let matching: Vec<_> = tasks
            .iter()
            .filter(|task| {
                task.get("name").and_then(Value::as_str) == Some(wait.task_name.as_str())
            })
            .collect();
        let [task] = matching.as_slice() else {
            return Err(denied());
        };
        if [
            "iterate_cache",
            "with_items",
            "retry",
            "timeout",
            "worker_selector",
            "worker_tolerations",
            "worker_affinity",
            "trace_tag_template",
        ]
        .iter()
        .any(|name| task.get(*name).is_some_and(|v| !v.is_null()))
        {
            return Ok(None);
        }
        let action = task
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        if action.contains("{{")
            || contains_template(task.get("permission_set_refs").unwrap_or(&Value::Null))
        {
            return Ok(None);
        }
        if wait.state == WorkflowTaskWaitState::Waiting {
            // Readiness routes the caller only. The snapshot constructor below
            // authorizes and locks the actual source, schema and ciphertext;
            // release compares its opaque sealed state before any delivery.
            let ready = match target {
                WorkflowTaskWaitTarget::Execution(id) => {
                    Executions::find_by_id(&mut *conn, id)
                        .await?
                        .ok_or_else(denied)?
                        .status
                        == ExecutionStatus::Completed
                }
                WorkflowTaskWaitTarget::Inquiry(id) => {
                    super::inquiry::InquiryRepository::find_by_id(&mut *conn, id)
                        .await?
                        .ok_or_else(denied)?
                        .status
                        == crate::models::InquiryStatus::Responded
                }
                WorkflowTaskWaitTarget::WorkQueueItem(id) => {
                    super::work_queue::WorkQueueItemRepository::find_by_id(&mut *conn, id)
                        .await?
                        .ok_or_else(denied)?
                        .status
                        == crate::models::WorkQueueItemStatus::Completed
                }
            };
            if !ready {
                return Ok(None);
            }
        }
        let key = key.ok_or_else(denied)?;
        super::accepted_input::AcceptedInputRepository::revalidate_execution(conn, parent.id, key)
            .await?;
        let path =
            Materialize::workflow_task_site_path(conn, parent.id, &wait.task_name, &action).await?;
        let task = graph
            .pointer(path.strip_prefix("/workflow").ok_or_else(denied)?)
            .cloned()
            .ok_or_else(denied)?;
        let revision = crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| denied())?;
        let sources = prepare_sources(conn, &parent, None, &workflow, key)
            .await?
            .ok_or_else(denied)?;
        let declarations = Declarations::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|decl| (decl.source_path.clone(), decl))
            .collect();
        let context = Context {
            predecessor: None,
            parent,
            workflow,
            task,
            path,
            revision,
            sources: sources.into(),
            declarations,
        };
        Self::prepare_context(conn, context, wait, key)
            .await
            .map(Some)
    }

    async fn prepare_context(
        conn: &mut PgConnection,
        context: Context,
        wait: &WorkflowTaskWait,
        key: &str,
    ) -> Result<PreparedNativeWait> {
        let action = context
            .task
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        let executable =
            Admissions::resolve_workflow_snapshot(conn, context.parent.id, &wait.task_name, action)
                .await?;
        let refs = match context.task.get("permission_set_refs") {
            Some(value) if !value.is_null() => {
                serde_json::from_value::<Vec<String>>(value.clone())?
            }
            _ => executable
                .executable
                .action
                .default_execution_permission_set_refs
                .clone(),
        };
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
                    let path = format!("/wait_for/{kind}");
                    let rendered = context
                        .render(conn, &path, InputSitePurpose::Control, None, key)
                        .await?;
                    let site = InputSite {
                        execution: context.parent.id,
                        kind: InputEntityKind::WorkflowVariables,
                        purpose: InputSitePurpose::Control,
                        path: format!("{}{path}", context.path),
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
                .await?,
            )
        } else {
            let released = Snapshots::prepare_released_wait(
                conn,
                context.parent.id,
                &context.path,
                wait.id,
                key,
            )
            .await?;
            if released.state() != "released" {
                return Err(denied());
            }
            Observation::Released(released)
        };
        let existing = Executions::find_by_workflow_task(
            &mut *conn,
            context.workflow.id,
            &wait.task_name,
            None,
        )
        .await?;
        let mut receipts = Vec::new();
        let mut recovery_fence = None;
        let child = if let Some(existing) = &existing {
            if existing.parent != Some(context.parent.id)
                || serde_json::to_value(&existing.executable_snapshot)?
                    != serde_json::to_value(Some(&executable))?
            {
                return Err(denied());
            }
            let retained = Admissions::require(conn, RuntimeOwner::Execution(existing.id)).await?;
            if retained.authority.binding != admission.authority().binding
                || retained.authority.attribution != admission.authority().attribution
            {
                return Err(denied());
            }
            recovery_fence = Some(Authority::retained_admission_fence(conn, retained.id).await?);
            if existing.status == ExecutionStatus::Requested {
                receipts.extend(
                    Materialize::prepare_execution_input_receipts(conn, existing.id, key).await?,
                );
            }
            None
        } else {
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
                        executable.executable.action.timeout_seconds.unwrap_or(
                            crate::config::app_default_execution_timeout_seconds() as i32,
                        ),
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
                        task_name: wait.task_name.clone(),
                        task_index: None,
                        task_batch: None,
                        triggered_by: None,
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
            super::rule_authority::RuleAuthorityRepository::copy_execution(
                conn,
                context.parent.id,
                child.id,
            )
            .await?;
            super::recorded_input_authority::RecordedInputAuthorityRepository::copy(
                conn,
                "execution",
                context.parent.id,
                "execution",
                child.id,
            )
            .await?;
            Schemas::attach(conn, BindingOwner::Execution, child.id, &schema).await?;
            let input_field = if context
                .task
                .get("input")
                .is_some_and(|v| v.as_object().is_some_and(|v| !v.is_empty()))
            {
                "input"
            } else {
                "params"
            };
            let authored = context
                .task
                .get(input_field)
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut value = authored.clone();
            let mut leaves = Vec::new();
            collect_leaves(&authored, String::new(), &mut leaves, 0)?;
            let mut fields = Vec::new();
            for (path, _) in leaves {
                // Empty absent input is literal absence, not an invented authored field.
                if context.task.get(input_field).is_none() {
                    break;
                }
                let rendered = context
                    .render(
                        conn,
                        &format!("/{input_field}{path}"),
                        InputSitePurpose::Value,
                        Some(&observation),
                        key,
                    )
                    .await?;
                *value.pointer_mut(&path).ok_or_else(denied)? = rendered.value().clone();
                let destination = Inputs::prepare_destination(
                    conn,
                    &rendered,
                    &InputSite {
                        execution: child.id,
                        kind: InputEntityKind::ExecutionConfig,
                        purpose: InputSitePurpose::Value,
                        path: path.clone(),
                    },
                    key,
                )
                .await?;
                fields.push(Field {
                    path,
                    rendered,
                    destination,
                });
            }
            let compiled =
                crate::value_schema::CompiledContract::new(&schema.input).map_err(|_| denied())?;
            let defaults = compiled.apply_defaults(&value).map_err(|_| denied())?;
            let (value, defaults) =
                Materialize::action_defaults(conn, child.id, &value, defaults, Some(key)).await?;
            compiled.validate(&value).map_err(|_| denied())?;
            check_bounds(&value)?;
            Some(Child {
                execution: child,
                admission: attached.id,
                contract: schema.input,
                value,
                fields,
                defaults,
            })
        };
        let original_secrets = Secrets::find_stored_by_entity(
            &mut *conn,
            ENTITY_WORKFLOW_VARIABLES,
            context.parent.id,
        )
        .await?;
        Ok(PreparedNativeWait {
            context,
            original: wait.clone(),
            original_secrets,
            observation: Some(observation),
            admission,
            selector,
            child,
            existing,
            recovery_fence,
            receipts,
            released: OnceLock::new(),
        })
    }
}

impl PreparedNativeWait {
    pub fn workflow(&self) -> Id {
        self.context.workflow.id
    }
    /// Only target buffer comparisons, release association, prepared child binding,
    /// receipt insertion and one-shot config/seal publication occur here.
    pub async fn apply(&mut self, conn: &mut PgConnection, key: &str) -> Result<Option<Execution>> {
        if self.released.get().is_some() {
            return Err(denied());
        }
        let workflow = Workflows::find_by_id_for_update(&mut *conn, self.workflow())
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
        if serde_json::to_value(&workflow)? != serde_json::to_value(&self.context.workflow)?
            || serde_json::to_value(&parent)? != serde_json::to_value(&self.context.parent)?
            || serde_json::to_value(&wait)? != serde_json::to_value(&self.original)?
            || Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?
                != self.original_secrets
        {
            return Err(denied());
        }
        let released = match self.observation.take().ok_or_else(denied)? {
            Observation::Pending(wait) => Snapshots::release_wait(conn, wait).await?,
            Observation::Released(wait) => wait,
            Observation::Prospective(_) => return Err(denied()),
        };
        if released.state() != "released" {
            return Err(denied());
        }
        self.released.set(released).map_err(|_| denied())?;
        let result = if let Some(child) = &mut self.child {
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
            let (value, secrets) = crate::secret_values::merge_contract_secret_redactions(
                child.value.clone(),
                &sources,
                &compiled,
            )?;
            let secrets = crate::secret_values::prepare_secret_values(secrets, key)?;
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
                &secrets,
            )
            .await?;
            self.receipts
                .extend(Materialize::seal_execution_inputs(conn, child.execution.id, key).await?);
            Some(child.execution.clone())
        } else if let Some(existing) = &self.existing {
            let actual = Executions::find_by_id(&mut *conn, existing.id)
                .await?
                .ok_or_else(denied)?;
            if serde_json::to_value(&actual)? != serde_json::to_value(existing)? {
                return Err(denied());
            }
            (actual.status == ExecutionStatus::Requested).then_some(actual)
        } else {
            return Err(denied());
        };
        self.ensure_fresh_on(conn).await?;
        Ok(result)
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.released
            .get()
            .ok_or_else(denied)?
            .ensure_fresh_on(conn)
            .await?;
        self.admission.ensure_fresh_on(conn).await?;
        if let Some(fence) = &self.recovery_fence {
            fence.ensure_fresh_on(conn).await?;
        }
        for receipt in self.receipts.iter().chain(self.selector.iter()) {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.released.get().ok_or_else(denied)?.ensure_fresh()?;
        self.admission.effect_fence().ensure_fresh()?;
        if let Some(fence) = &self.recovery_fence {
            fence.ensure_fresh()?;
        }
        for receipt in self.receipts.iter().chain(self.selector.iter()) {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
}
fn denied() -> Error {
    Error::PermissionDenied("Native wait delivery is unavailable or changed".into())
}
