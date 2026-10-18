//! Ordinary with_items preparation. Item/index are selected exclusively from
//! the retained Control render, never supplied by a scheduler context or cache.
use super::{
    accepted_execution_source::prepare_observation,
    accepted_input::AcceptedInputRepository,
    declared_input::{DeclaredInputBinding, DeclaredInputRepository as Declarations},
    declared_input_materialization::{
        pointer_to_node, DeclaredInputMaterializationRepository as Materialize,
    },
    execution::{
        CreateExecutionInput, ExecutionRepository as Executions, UpdateExecutionInput,
        SELECT_COLUMNS,
    },
    execution_runtime_admission::{ExecutionAdmission, ExecutionAdmissionRepository as Admissions},
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        PreparedIterationControl, PreparedIterationItem, PreparedReceiptDestination,
        PreparedWorkflowOutcome, ReceiptContext, RenderedReceiptContext,
    },
    native_transition_materialization::{
        append_fields, prepare_sources, NativeTransitionRenderFrame, Sources,
    },
    native_workflow_materialization::{
        check_bounds, collect_leaves, contains_template, escape, expression_functions,
        expression_names,
    },
    runtime_authority::{
        ApprovalEffectFence, RuntimeAuthorityRepository as Authority, RuntimeOwner,
    },
    schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository as Schemas},
    workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
    workflow_policy::{task_at_authored_path, WorkflowPolicyRepository},
    FindById, Update,
};
use crate::{
    models::{Execution, ExecutionStatus, Id, WorkflowExecution, WorkflowTaskMetadata},
    runtime_authority::InvocationBinding,
    secret_values::{SecretPathSource, SecretSource, StoredSecretValue, ENTITY_WORKFLOW_VARIABLES},
    Error, Result,
};
use serde_json::{json, Value};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod activation;
pub use activation::PreparedNativeIterationActivation;
mod completion;
mod empty;
mod empty_activation;
mod empty_ordinary;
mod empty_root;
pub use completion::{NativeIterationCompletionMember, PreparedNativeIterationCompletion};
pub use empty::NativeEmptyIterationSource;
pub use empty_activation::{PreparedNativeEmptyActivation, PreparedNativeEmptyRecovery};
pub use empty_root::PreparedEmptyRootReplay;

// Ordinary iteration already creates its complete sibling set at activation.
// Bound that set and its proof buffers instead of loading retry/result history.
const MAX_CHILDREN: usize = 16_384;
const MAX_MATERIALIZED_BYTES: usize = 1024 * 1024;

struct Context {
    parent: Execution,
    workflow: WorkflowExecution,
    task: Value,
    task_name: String,
    path: String,
    revision: String,
    bindings: BTreeMap<String, DeclaredInputBinding>,
    sources: Sources,
    outcome: Option<PreparedWorkflowOutcome>,
    frame: Option<Arc<dyn NativeTransitionRenderFrame>>,
    frame_deadline: Option<chrono::DateTime<chrono::Utc>>,
}
struct Field {
    graph_budget: super::inherited_input::PreparedSourceGraphBudget,
    path: String,
    rendered: RenderedReceiptContext,
    destination: PreparedReceiptDestination,
}
struct Child {
    row: Execution,
    admission: ExecutionAdmission,
    admission_id: Id,
    contract: crate::value_schema::Contract,
    value: Value,
    fields: Vec<Field>,
    defaults: Vec<SecretPathSource>,
}

/// Status facts only. Do not load or synthesize result/data/whole-result Values
/// while counting the authoritative latest attempt for each actual ordinal.
#[derive(Clone, Debug, PartialEq, serde::Serialize, sqlx::FromRow)]
struct IterationObservation {
    execution: Id,
    created: chrono::DateTime<chrono::Utc>,
    original: Id,
    ordinal: i32,
    batch: Option<i32>,
    attempt: i32,
    metadata_attempt: i32,
    status: ExecutionStatus,
}

struct ObservedInputState {
    source: Execution,
    fields: Vec<StoredSecretValue>,
    schema: ActionSchemaBinding,
}
impl ObservedInputState {
    async fn require_current(&self, conn: &mut PgConnection) -> Result<()> {
        let current = Executions::find_by_id(&mut *conn, self.source.id)
            .await?
            .ok_or_else(changed)?;
        super::accepted_execution_source::same_execution(&self.source, &current)?;
        if Secrets::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_EXECUTION_CONFIG,
            self.source.id,
        )
        .await?
            != self.fields
            || Schemas::find(conn, BindingOwner::Execution, self.source.id)
                .await?
                .as_ref()
                != Some(&self.schema)
        {
            return Err(changed());
        }
        Ok(())
    }
}

pub struct PreparedNativeIteration {
    empty_root: Option<super::inherited_input::PreparedEmptyRoot>,
    empty_control: Option<ReceiptContext>,
    empty_outcome: Option<super::native_input_snapshot::PreparedNativeEmptyIterationOutcome>,
    context: Context,
    array: Option<RenderedReceiptContext>,
    iteration_control: Option<PreparedIterationControl>,
    parent_fence: ApprovalEffectFence,
    original_children: Vec<Execution>,
    original_secrets: Vec<StoredSecretValue>,
    children: Vec<Child>,
    controls: Vec<ReceiptContext>,
    recovered: Vec<Execution>,
    recovery_fences: Vec<ApprovalEffectFence>,
    receipts: Vec<ReceiptContext>,
    applied: bool,
    finished: bool,
    total: usize,
    concurrency: usize,
    active_attempts: i64,
    incomplete_attempts: i64,
    observations: Vec<IterationObservation>,
    observed_outcomes: Vec<PreparedWorkflowOutcome>,
    original_outcomes: Vec<PreparedWorkflowOutcome>,
    observed_inputs: Vec<ObservedInputState>,
}

pub struct NativeIterationResult {
    /// Only the initial concurrency window. Every sibling is already sealed in
    /// this creation transaction, including the ones not yet sent to the broker.
    pub requested: Vec<Execution>,
    pub total: usize,
    pub remaining: i64,
    /// A real terminal parent, never a synthetic child/outcome. Empty roots use
    /// their finite output recipe; nonempty aggregation remains status-only.
    pub completed_parent: Option<Execution>,
}

pub struct PreparedIterationControlDelivery {
    empty: Option<super::native_input_snapshot::ReleasedNativeEmptyIteration>,
    receipts: Vec<ReceiptContext>,
    outcomes: Vec<PreparedWorkflowOutcome>,
    fence: ApprovalEffectFence,
    source_fences: Vec<ApprovalEffectFence>,
    observed_inputs: Vec<ObservedInputState>,
}
impl PreparedIterationControlDelivery {
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(empty) = &self.empty {
            empty.ensure_fresh_on(conn).await?;
        }
        self.fence.ensure_fresh_on(conn).await?;
        for input in &self.observed_inputs {
            input.require_current(conn).await?;
        }
        for fence in &self.source_fences {
            fence.ensure_fresh_on(conn).await?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        for outcome in &self.outcomes {
            outcome.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Some(empty) = &self.empty {
            empty.ensure_fresh()?;
        }
        self.fence.ensure_fresh()?;
        for fence in &self.source_fences {
            fence.ensure_fresh()?;
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        for outcome in &self.outcomes {
            outcome.ensure_fresh()?;
        }
        Ok(())
    }
}

impl Context {
    async fn render(
        &self,
        conn: &mut PgConnection,
        relative: &str,
        purpose: InputSitePurpose,
        item: Option<&PreparedIterationItem>,
        key: &str,
    ) -> Result<RenderedReceiptContext> {
        let path = format!("{}{relative}", self.path);
        let authored = self.task.pointer(relative).ok_or_else(changed)?;
        let Some(binding) = self.bindings.get(&path) else {
            if contains_template(authored) {
                return Err(changed());
            }
            let mut literal = Inputs::literal(conn, self.parent.id, &self.revision, &path)
                .await
                .map_err(|error| preparation_error(error, "literal-site"))?;
            if let Some(frame) = &self.frame {
                frame.attach_literal(&mut literal)?;
            } else if relative == "/with_items" && authored.as_array().is_some_and(Vec::is_empty) {
                // A literal empty list reads no item, but its outcome must keep
                // the complete independently held accepted source closure.
                for source in self
                    .sources
                    .fields
                    .iter()
                    .filter_map(|field| field.source.as_ref())
                {
                    Inputs::attach_rendered_control(&mut literal, source)?;
                }
            }
            return Ok(literal);
        };
        let mut session = Inputs::begin_render(
            conn,
            self.parent.id,
            &binding.declaration_revision,
            &path,
            &binding.expression,
            purpose,
        )
        .await
        .map_err(|error| preparation_error(error, "begin-render"))?;
        if let Some(frame) = &self.frame {
            frame
                .bind(conn, &mut session, binding, key)
                .await
                .map_err(|error| preparation_error(error, "frame-bind"))?;
            if let Some(item) = item {
                Inputs::bind_iteration_item(&mut session, item)
                    .map_err(|error| preparation_error(error, "frame-item-bind"))?;
            }
            let mut rendered = Inputs::evaluate_with_keys(conn, session, key)
                .await
                .map_err(|error| preparation_error(error, "frame-evaluate"))?;
            frame
                .attach_rendered_control(&mut rendered)
                .map_err(|error| preparation_error(error, "frame-rendered-control"))?;
            return Ok(rendered);
        }
        let names = expression_names(&binding.expression)?;
        for field in &self.sources.fields {
            for path in &field.paths {
                let namespace = path
                    .split('/')
                    .nth(1)
                    .unwrap_or_default()
                    .replace("~1", "/")
                    .replace("~0", "~");
                if !names.contains(&namespace) {
                    continue;
                }
                if let Some(source) = &field.source {
                    Inputs::bind_prepared(&mut session, path, source, source.value())?;
                } else {
                    Inputs::bind_public_field(conn, &mut session, path, &field.site).await?;
                }
            }
        }
        if let Some(item) = item {
            Inputs::bind_iteration_item(&mut session, item)?;
        }
        if let Some(outcome) = &self.outcome {
            Inputs::bind_outcome(&mut session, outcome)?;
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
        Inputs::evaluate_with_keys(conn, session, key).await
    }

    async fn metadata(
        &self,
        conn: &mut PgConnection,
        field: &str,
        item: &PreparedIterationItem,
        key: &str,
        controls: &mut Vec<ReceiptContext>,
    ) -> Result<Option<Value>> {
        let Some(authored) = self.task.get(field).filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let mut value = authored.clone();
        let mut leaves = Vec::new();
        collect_leaves(authored, String::new(), &mut leaves, 0)?;
        for (suffix, leaf) in leaves {
            if !contains_template(&leaf) {
                continue;
            }
            let relative = format!("/{field}{suffix}");
            let rendered = self
                .render(conn, &relative, InputSitePurpose::Control, Some(item), key)
                .await?;
            *value.pointer_mut(&suffix).ok_or_else(changed)? = rendered.value().clone();
            let site = InputSite {
                execution: self.parent.id,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Control,
                path: format!("{}{relative}", self.path),
            };
            controls.push(
                Inputs::record_render(
                    conn,
                    &rendered,
                    &site,
                    rendered.value(),
                    rendered.selections(),
                    key,
                )
                .await?,
            );
        }
        Ok(Some(value))
    }
}

pub struct NativeIterationMaterializationRepository;
impl NativeIterationMaterializationRepository {
    /// Syntax-only routing. This never evaluates a selector or supplies proof.
    pub fn is_ordinary_task(parent: &Execution, name: &str) -> bool {
        let Some(graph) = parent
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
        else {
            return false;
        };
        fn visit(tasks: &Value, name: &str, depth: usize) -> bool {
            if depth > 32 {
                return false;
            }
            match tasks {
                Value::Array(tasks) => tasks.iter().any(|task| {
                    task.get("name").and_then(Value::as_str) == Some(name)
                        && task.get("with_items").is_some_and(|v| !v.is_null())
                        && task.get("iterate_cache").is_none_or(Value::is_null)
                        || task
                            .get("tasks")
                            .is_some_and(|nested| visit(nested, name, depth + 1))
                }),
                Value::Object(tasks) => tasks.iter().any(|(key, task)| {
                    key == name
                        && task.get("with_items").is_some_and(|v| !v.is_null())
                        && task.get("iterate_cache").is_none_or(Value::is_null)
                        || task
                            .get("tasks")
                            .is_some_and(|nested| visit(nested, name, depth + 1))
                }),
                _ => false,
            }
        }
        graph
            .definition
            .get("tasks")
            .is_some_and(|tasks| visit(tasks, name, 0))
    }
    /// Source phase only. The caller owns one creation transaction and must call
    /// apply after acquiring the workflow lock, then fence commit and broker I/O.
    pub async fn prepare(
        conn: &mut PgConnection,
        parent: &Execution,
        task_name: &str,
        triggered_by: Option<&str>,
        key: &str,
    ) -> Result<PreparedNativeIteration> {
        Self::prepare_inner(conn, parent, task_name, triggered_by, key, None, None).await
    }
    async fn prepare_inner(
        conn: &mut PgConnection,
        parent: &Execution,
        task_name: &str,
        triggered_by: Option<&str>,
        key: &str,
        frame: Option<Arc<dyn NativeTransitionRenderFrame>>,
        frame_deadline: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<PreparedNativeIteration> {
        let admitted = Admissions::require(conn, RuntimeOwner::Execution(parent.id)).await?;
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(changed)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(changed)?
            .definition;
        let policy = Authority::require_policy(conn, admitted.policy_revision).await?;
        let policy = WorkflowPolicyRepository::for_invocation(
            conn,
            &policy,
            &admitted.authority.binding,
            snapshot,
        )
        .await?;
        let paths: BTreeSet<_> = policy
            .policy
            .invocations
            .iter()
            .filter_map(|invocation| {
                let InvocationBinding::WorkflowTask {
                    workflow_ref,
                    task_path,
                    ..
                } = &invocation.binding
                else {
                    return None;
                };
                (workflow_ref == &parent.action_ref
                    && Admissions::workflow_task_matches(graph, task_path, task_name))
                .then_some(task_path)
            })
            .collect();
        let paths = paths.into_iter().collect::<Vec<_>>();
        let [path] = paths.as_slice() else {
            return Err(changed());
        };
        let task = task_at_authored_path(graph, path).ok_or_else(changed)?;
        if task.get("with_items").is_none_or(Value::is_null)
            || task.get("iterate_cache").is_some_and(|v| !v.is_null())
        {
            return Err(changed());
        }
        let workflow = Workflows::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(changed)?;
        if parent.status != ExecutionStatus::Running || workflow.status != ExecutionStatus::Running
        {
            return Err(changed());
        }
        let path = format!("/workflow{}", pointer_to_node(graph, task)?);
        if let Some(frame) = &frame {
            if frame.execution() != parent.id
                || !frame
                    .selected_targets()
                    .iter()
                    .any(|name| name == task_name)
            {
                return Err(changed());
            }
        }
        let bindings: BTreeMap<_, _> = Declarations::bindings(conn, parent.id)
            .await?
            .into_iter()
            .map(|binding| (binding.source_path.clone(), binding))
            .collect();
        let mut named_results = false;
        for binding in bindings
            .values()
            .filter(|binding| binding.source_path.starts_with(&format!("{path}/")))
        {
            let names = expression_names(&binding.expression)?;
            named_results |= names
                .iter()
                .any(|name| matches!(name.as_str(), "task" | "tasks" | "result"))
                || expression_functions(&binding.expression)?.contains("result");
        }
        let mut sources = prepare_sources(conn, parent, None, &workflow, key)
            .await?
            .ok_or_else(changed)?;
        let mut outcome = None;
        let named_sources = if named_results {
            latest_named_sources(conn, parent.id, workflow.id).await?
        } else {
            Vec::new()
        };
        for child in named_sources {
            if !matches!(
                child.status,
                ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Timeout
            ) {
                continue;
            }
            let metadata = child.workflow_task.as_ref().ok_or_else(changed)?;
            let held = Inputs::prepare_outcome(conn, parent.id, child.id).await?;
            let mut prefixes = vec![
                format!("/task/{}", escape(&metadata.task_name)),
                format!("/tasks/{}", escape(&metadata.task_name)),
            ];
            if triggered_by == Some(metadata.task_name.as_str()) {
                prefixes.push("/result".into());
                outcome = Some(held);
            }
            if !append_fields(
                conn,
                parent.id,
                InputEntityKind::ExecutionResult,
                child.id,
                child.result.as_ref().unwrap_or(&Value::Null),
                "",
                &prefixes,
                false,
                key,
                &mut sources.fields,
            )
            .await?
            {
                return Err(changed());
            }
            if sources.fields.len() > 256 {
                return Err(changed());
            }
        }
        let context = Context {
            parent: parent.clone(),
            workflow,
            task: task.clone(),
            task_name: task_name.into(),
            path,
            revision: crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
                .map_err(|_| changed())?,
            bindings,
            sources,
            outcome,
            frame,
            frame_deadline,
        };
        let original_secrets =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?;
        let original_children =
            initial_children(conn, parent.id, context.workflow.id, task_name).await?;
        let dispatches = original_dispatch_count(conn, context.workflow.id, task_name).await?;
        if dispatches != original_children.len() {
            // Retention/missing rows must not look like an empty/new expansion.
            // Never evaluate a replacement list or create new siblings here.
            return Err(changed());
        }
        let observations =
            latest_observations(conn, parent.id, context.workflow.id, task_name).await?;
        if observations.len() != dispatches
            || observations
                .iter()
                .enumerate()
                .any(|(index, observation)| i32::try_from(index).ok() != Some(observation.ordinal))
        {
            return Err(changed());
        }
        let mut observed_outcomes = Vec::new();
        for observation in &observations {
            if matches!(
                observation.status,
                ExecutionStatus::Completed
                    | ExecutionStatus::Failed
                    | ExecutionStatus::Timeout
                    | ExecutionStatus::Cancelled
                    | ExecutionStatus::Abandoned
            ) {
                let outcome =
                    Inputs::prepare_outcome(conn, parent.id, observation.execution).await?;
                if outcome.execution() != observation.execution || outcome.task_name() != task_name
                {
                    return Err(changed());
                }
                observed_outcomes.push(outcome);
            }
        }
        let parent_fence = Authority::retained_admission_fence(conn, admitted.id).await?;
        let concurrency = positive_optional(context.task.get("concurrency"))?.unwrap_or(1);
        let active_attempts = active_attempts(conn, context.workflow.id, task_name).await?;
        let incomplete_attempts = Executions::summarize_workflow_task_latest_attempts(
            &mut *conn,
            context.workflow.id,
            task_name,
        )
        .await?
        .in_flight;
        let mut plan = PreparedNativeIteration {
            empty_root: None,
            empty_control: None,
            empty_outcome: None,
            context,
            array: None,
            iteration_control: None,
            parent_fence,
            original_children,
            original_secrets,
            children: vec![],
            controls: vec![],
            recovered: vec![],
            recovery_fences: vec![],
            receipts: vec![],
            applied: false,
            finished: false,
            total: 0,
            concurrency,
            active_attempts,
            incomplete_attempts,
            observations,
            observed_outcomes,
            original_outcomes: vec![],
            observed_inputs: vec![],
        };
        if !plan.original_children.is_empty() {
            plan.total = plan.original_children.len();
            let grouped = positive_optional(plan.context.task.get("batch_size"))?
                .is_some_and(|size| size > 1);
            for (index, child) in plan.original_children.iter().enumerate() {
                let metadata = child.workflow_task.as_ref().ok_or_else(changed)?;
                let ordinal = i32::try_from(index).map_err(|_| changed())?;
                if metadata.task_index != Some(ordinal)
                    || metadata.task_batch != grouped.then_some(ordinal)
                {
                    return Err(changed());
                }
                let observed = plan.observations.get(index).ok_or_else(changed)?;
                if observed.original != child.id
                    || observed.batch != metadata.task_batch
                    || observed.attempt != observed.metadata_attempt
                    || observed.attempt < 0
                    || (observed.execution == child.id) != (observed.attempt == 0)
                {
                    return Err(changed());
                }
                let (input, fence, receipts) = observe_accepted_inputs(conn, child, key).await?;
                plan.observed_inputs.push(input);
                plan.recovery_fences.push(fence);
                if child.status != ExecutionStatus::Requested {
                    plan.receipts.extend(receipts);
                    if observed.execution != child.id
                        && matches!(
                            child.status,
                            ExecutionStatus::Completed
                                | ExecutionStatus::Failed
                                | ExecutionStatus::Timeout
                                | ExecutionStatus::Cancelled
                                | ExecutionStatus::Abandoned
                        )
                    {
                        let original = Inputs::prepare_outcome(conn, parent.id, child.id).await?;
                        if original.execution() != child.id || original.task_name() != task_name {
                            return Err(changed());
                        }
                        plan.original_outcomes.push(original);
                    }
                    continue;
                }
                // The Requested branch below retains the delivery views of the
                // same accepted receipt graph, without duplicating its roots.
                let admission =
                    Admissions::require(conn, RuntimeOwner::Execution(child.id)).await?;
                plan.recovery_fences
                    .push(Authority::retained_admission_fence(conn, admission.id).await?);
                plan.receipts.extend(
                    Materialize::prepare_execution_input_receipts(conn, child.id, key).await?,
                );
                plan.recovered.push(child.clone());
            }
            // A latest retry has its own accepted input/schema/admission scope.
            // Original input equality does not make that independent scope or
            // its deadline disappear. Hold both without reevaluating any data.
            for observation in &plan.observations {
                if observation.execution == observation.original {
                    continue;
                }
                let latest = Executions::find_by_id(&mut *conn, observation.execution)
                    .await?
                    .ok_or_else(changed)?;
                if latest.parent != Some(parent.id)
                    || latest.original_execution != Some(observation.original)
                    || latest.created != observation.created
                    || latest.status != observation.status
                    || latest.retry_count != observation.attempt
                    || latest.workflow_task.as_ref().is_none_or(|metadata| {
                        metadata.workflow_execution != plan.context.workflow.id
                            || metadata.task_name != task_name
                            || metadata.task_index != Some(observation.ordinal)
                            || metadata.task_batch != observation.batch
                            || metadata.retry_count != observation.metadata_attempt
                    })
                {
                    return Err(changed());
                }
                let (input, fence, receipts) = observe_accepted_inputs(conn, &latest, key).await?;
                plan.observed_inputs.push(input);
                plan.recovery_fences.push(fence);
                plan.receipts.extend(receipts);
            }
            plan.ensure_fresh_on(conn).await?;
            return Ok(plan);
        }
        // Single canonical evaluation. Never pass the returned strings back to a
        // template renderer, including strings nested under an item object/list.
        let array = plan
            .context
            .render(conn, "/with_items", InputSitePurpose::Control, None, key)
            .await?;
        check_bounds(array.value())?;
        // This computes only the sibling count. Item shape remains exclusively
        // A's authenticated constructor's responsibility, including scalars.
        let value_count = array.value().as_array().map_or(1, Vec::len);
        let batch = positive_optional(plan.context.task.get("batch_size"))?;
        let total = value_count.div_ceil(batch.unwrap_or(1));
        if total > MAX_CHILDREN {
            return Err(Error::validation(
                "Ordinary iteration exceeds its bounded sibling set",
            ));
        }
        if total
            .checked_mul(serde_json::to_vec(array.value())?.len())
            .is_none_or(|bytes| bytes > MAX_MATERIALIZED_BYTES)
        {
            return Err(Error::validation(
                "Ordinary iteration retained controls exceed their byte bound",
            ));
        }
        let array_path = format!("{}/with_items", plan.context.path);
        if plan.context.frame.is_some() || value_count == 0 {
            // A selected condition can itself still be pending, even when the
            // list reads only committed parameters. The official constructor
            // retains those parents; public record/replay must stay closed.
            let control = Inputs::prepare_iteration_control(conn, &array, key)
                .await
                .map_err(|error| preparation_error(error, "prepare-list-control"))?;
            if control.batch_size() != batch || control.item_count() != value_count {
                return Err(changed());
            }
            if value_count == 0 {
                plan.empty_control =
                    Some(Inputs::empty_iteration_control(conn, &control, key).await?);
            }
            plan.iteration_control = Some(control);
        } else if plan.context.bindings.contains_key(&array_path) {
            let site = InputSite {
                execution: parent.id,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Control,
                path: array_path,
            };
            plan.controls.push(
                Inputs::record_render(conn, &array, &site, array.value(), array.selections(), key)
                    .await?,
            );
        }
        plan.array = Some(array);
        plan.total = total;
        let array_control_count = plan.controls.len();
        let closure = super::native_input_origin::NativeInputOriginRepository::capture_with_key(
            conn,
            parent.id,
            &[],
            Some(key),
        )
        .await?;
        let mut bytes = 0usize;
        for ordinal in 0..total {
            let item = if let Some(control) = &plan.iteration_control {
                Inputs::prepare_iteration_item_from_control(control, ordinal)
                    .map_err(|error| preparation_error(error, "select-prepared-item"))?
            } else {
                Inputs::prepare_iteration_item(
                    conn,
                    plan.array.as_ref().ok_or_else(changed)?,
                    ordinal,
                    batch,
                    key,
                )
                .await
                .map_err(|error| preparation_error(error, "select-committed-item"))?
            };
            let mut controls = Vec::new();
            let action = plan
                .context
                .metadata(conn, "action", &item, key, &mut controls)
                .await?
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(changed)?;
            let snapshot =
                Admissions::resolve_workflow_snapshot(conn, parent.id, task_name, &action).await?;
            let selected = plan
                .context
                .metadata(conn, "permission_set_refs", &item, key, &mut controls)
                .await?;
            let refs = match selected {
                Some(v) => permission_refs(v)?,
                None => snapshot
                    .executable
                    .action
                    .default_execution_permission_set_refs
                    .clone(),
            };
            let admission = Admissions::workflow_child_prepared(
                conn, parent.id, task_name, &snapshot, &refs, &closure,
            )
            .await?;
            let schema = Admissions::workflow_schema(conn, parent.id, &snapshot).await?;
            let mut metadata = BTreeMap::new();
            for name in [
                "timeout",
                "trace_tag_template",
                "worker_selector",
                "worker_tolerations",
                "worker_affinity",
            ] {
                metadata.insert(
                    name,
                    plan.context
                        .metadata(conn, name, &item, key, &mut controls)
                        .await?,
                );
            }
            let timeout = metadata["timeout"].as_ref().map(seconds).transpose()?;
            if let Some(value) = metadata["worker_selector"].as_ref() {
                crate::scheduling::parse_worker_selector(value)?;
            }
            if let Some(value) = metadata["worker_tolerations"].as_ref() {
                crate::scheduling::parse_worker_tolerations(value)?;
            }
            if let Some(value) = metadata["worker_affinity"].as_ref() {
                crate::scheduling::parse_worker_affinity(value)?;
            }
            let trace_tag = match metadata["trace_tag_template"].as_ref() {
                None | Some(Value::Null) => parent.trace_tag.clone(),
                Some(v) => {
                    let text = v
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string());
                    if text.trim().is_empty() {
                        parent.trace_tag.clone()
                    } else {
                        Some(crate::trace_tag::normalize_trace_tag(&text)?)
                    }
                }
            };
            let row = Executions::create_pinned(
                &mut *conn,
                CreateExecutionInput {
                    action: Some(snapshot.executable.action.id),
                    action_ref: action,
                    config: None,
                    env_vars: parent.env_vars.clone(),
                    parent: Some(parent.id),
                    enforcement: parent.enforcement,
                    executor: parent.executor,
                    permission_set_refs: refs,
                    status: ExecutionStatus::Requested,
                    trace_tag,
                    timeout_seconds: Some(
                        timeout
                            .or(snapshot.executable.action.timeout_seconds)
                            .unwrap_or(
                                crate::config::app_default_execution_timeout_seconds() as i32
                            ),
                    ),
                    worker_selector: metadata.remove("worker_selector").flatten(),
                    worker_tolerations: metadata.remove("worker_tolerations").flatten(),
                    worker_affinity: metadata.remove("worker_affinity").flatten(),
                    artifact_retention_policy: parent
                        .artifact_retention_policy
                        .or(snapshot.executable.action.artifact_retention_policy),
                    artifact_retention_limit: parent
                        .artifact_retention_limit
                        .or(snapshot.executable.action.artifact_retention_limit),
                    workflow_task: Some(WorkflowTaskMetadata {
                        workflow_execution: plan.context.workflow.id,
                        task_name: task_name.into(),
                        task_index: Some(i32::try_from(item.index()).map_err(|_| changed())?),
                        // Ordinary task_batch is its batch ordinal, not cache
                        // entry count. The signed Control pins the actual size.
                        task_batch: batch
                            .filter(|size| *size > 1)
                            .map(|_| i32::try_from(item.index()).map_err(|_| changed()))
                            .transpose()?,
                        triggered_by: triggered_by.map(str::to_owned),
                        retry_count: 0,
                        max_retries: plan
                            .context
                            .task
                            .pointer("/retry/count")
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                            .try_into()
                            .map_err(|_| changed())?,
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
            super::rule_authority::RuleAuthorityRepository::copy_execution(conn, parent.id, row.id)
                .await?;
            super::recorded_input_authority::RecordedInputAuthorityRepository::copy(
                conn,
                "execution",
                parent.id,
                "execution",
                row.id,
            )
            .await?;
            Schemas::attach(conn, BindingOwner::Execution, row.id, &schema).await?;
            let input_name = if plan
                .context
                .task
                .get("input")
                .and_then(Value::as_object)
                .is_some()
            {
                "input"
            } else {
                "params"
            };
            let authored = plan
                .context
                .task
                .get(input_name)
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
                let rendered = plan
                    .context
                    .render(
                        conn,
                        &format!("/{input_name}{path}"),
                        InputSitePurpose::Value,
                        Some(&item),
                        key,
                    )
                    .await?;
                *value.pointer_mut(&path).ok_or_else(changed)? = rendered.value().clone();
                let (destination, graph_budget) = Inputs::prepare_destination_with_graph_budget(
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
                .await
                .map_err(|error| preparation_error(error, "prepare-input-destination"))?;
                let mut field = Field {
                    graph_budget,
                    path,
                    rendered,
                    destination,
                };
                for control in plan
                    .controls
                    .iter()
                    .take(array_control_count)
                    .chain(&controls)
                {
                    let source =
                        Inputs::prepare_recorded_source(conn, parent.id, control, key).await?;
                    Inputs::attach_prepared_destination_control(
                        &mut field.rendered,
                        &mut field.destination,
                        &source,
                    )?;
                    Inputs::include_prepared_control_graph_budget(
                        conn,
                        &mut field.graph_budget,
                        &source,
                        key,
                    )
                    .await?;
                }
                fields.push(field);
            }
            let contract =
                crate::value_schema::CompiledContract::new(&schema.input).map_err(|_| changed())?;
            let defaults = contract.apply_defaults(&value).map_err(|_| changed())?;
            let (value, defaults) =
                Materialize::action_defaults(conn, row.id, &value, defaults, Some(key)).await?;
            for default in &defaults {
                if let SecretSource::Bound(provenance) = &default.source {
                    for origin in &provenance.origins {
                        plan.receipts.push(
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
            contract.validate(&value).map_err(|_| changed())?;
            check_bounds(&value)?;
            bytes = bytes
                .checked_add(serde_json::to_vec(&value)?.len())
                .ok_or_else(changed)?;
            if bytes > MAX_MATERIALIZED_BYTES {
                return Err(Error::validation(
                    "Ordinary iteration materialization exceeds its byte bound",
                ));
            }
            plan.controls.extend(controls);
            plan.children.push(Child {
                row,
                admission,
                admission_id: attached.id,
                contract: schema.input,
                value,
                fields,
                defaults,
            });
        }
        if plan.total == 0 {
            let outcome = super::native_input_snapshot::NativeInputSnapshotRepository::prepare_empty_iteration(
                conn, &plan.empty_source()?, key,
            ).await?;
            plan.empty_outcome = Some(outcome);
            if plan.is_terminal_empty_root() && plan.context.frame.is_none() {
                plan.empty_root = Some(plan.prepare_empty_root(conn, key).await?);
            }
        }
        plan.ensure_fresh_on(conn).await?;
        Ok(plan)
    }
}

impl PreparedNativeIteration {
    pub fn workflow(&self) -> Id {
        self.context.workflow.id
    }
    pub fn total(&self) -> usize {
        self.total
    }
    pub fn can_complete_empty(&self) -> bool {
        self.empty_root.is_some()
    }
    /// Successful or failed item aggregation may change real lifecycle state,
    /// but must not manufacture a result view while A/H own that Value seam.
    fn can_finalize_status_only(&self) -> bool {
        let Some(snapshot) = &self.context.parent.executable_snapshot else {
            return false;
        };
        let Some(workflow) = &snapshot.executable.workflow_definition else {
            return false;
        };
        let graph = &workflow.definition;
        let one_task = match graph.get("tasks") {
            Some(Value::Array(tasks)) => tasks.len() == 1,
            Some(Value::Object(tasks)) => tasks.len() == 1,
            _ => false,
        };
        let empty = |value: Option<&Value>| {
            value.is_none_or(|value| {
                value.is_null()
                    || value.as_object().is_some_and(serde_json::Map::is_empty)
                    || value.as_array().is_some_and(Vec::is_empty)
            })
        };
        one_task
            && self.context.parent.result.is_none()
            && self
                .context
                .workflow
                .failed_tasks
                .iter()
                .all(|name| name == &self.context.task_name)
            && empty(graph.get("output_map"))
            && empty(graph.get("output"))
            && empty(snapshot.executable.action.out_schema.as_ref())
            && [
                "next",
                "publish",
                "tasks",
                "wait_for",
                "on_success",
                "on_failure",
                "on_complete",
                "on_timeout",
            ]
            .iter()
            .all(|field| empty(self.context.task.get(*field)))
    }
    /// Reopen exact registry Controls in a later broker transaction. This is an
    /// observation of the original proof, not a new render/Condition/Value.
    pub async fn prepare_control_delivery(
        &self,
        conn: &mut PgConnection,
        key: &str,
    ) -> Result<PreparedIterationControlDelivery> {
        if !self.finished {
            return Err(changed());
        }
        let mut receipts = Vec::new();
        let mut source_fences = Vec::new();
        let mut observed_inputs = Vec::new();
        let mut outcomes = Vec::new();
        for input in &self.observed_inputs {
            input.require_current(conn).await?;
        }
        for control in &self.controls {
            receipts
                .push(Inputs::receipt_context(conn, control.site(), &control.origin(), key).await?);
        }
        // Real terminal observations do not replace the item/list Controls.
        for child in &self.original_children {
            let (input, fence, observed) = observe_accepted_inputs(conn, child, key).await?;
            observed_inputs.push(input);
            source_fences.push(fence);
            receipts.extend(observed);
            if self
                .observations
                .iter()
                .any(|latest| latest.original == child.id && latest.execution != child.id)
                && matches!(
                    child.status,
                    ExecutionStatus::Completed
                        | ExecutionStatus::Failed
                        | ExecutionStatus::Timeout
                        | ExecutionStatus::Cancelled
                        | ExecutionStatus::Abandoned
                )
            {
                outcomes
                    .push(Inputs::prepare_outcome(conn, self.context.parent.id, child.id).await?);
            }
        }
        let actual = latest_observations(
            conn,
            self.context.parent.id,
            self.workflow(),
            &self.context.task_name,
        )
        .await?;
        if actual != self.observations {
            return Err(changed());
        }
        for observation in &self.observations {
            if observation.execution == observation.original {
                continue;
            }
            let latest = Executions::find_by_id(&mut *conn, observation.execution)
                .await?
                .ok_or_else(changed)?;
            let (input, fence, observed) = observe_accepted_inputs(conn, &latest, key).await?;
            observed_inputs.push(input);
            source_fences.push(fence);
            receipts.extend(observed);
        }
        for observation in &self.observations {
            let outcome =
                Inputs::prepare_outcome(conn, self.context.parent.id, observation.execution)
                    .await?;
            outcome.ensure_fresh_on(conn).await?;
            outcomes.push(outcome);
        }
        let admission = Authority::execution_admission(conn, self.context.parent.id)
            .await?
            .ok_or_else(changed)?;
        let fence = Authority::retained_admission_fence(conn, admission.id).await?;
        Ok(PreparedIterationControlDelivery {
            empty: if self.empty_outcome.is_some() {
                Some(super::native_input_snapshot::NativeInputSnapshotRepository::reopen_empty_iteration(
                    conn, self.context.parent.id, &self.context.task_name, key).await?.ok_or_else(changed)?)
            } else {
                None
            },
            receipts,
            outcomes,
            fence,
            source_fences,
            observed_inputs,
        })
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(root) = &self.empty_root {
            root.ensure_fresh_on(conn).await?;
        }
        if let Some(frame) = &self.context.frame {
            frame.ensure_fresh_on(conn).await?;
        }
        if let Some(control) = &self.empty_control {
            control.ensure_fresh_on(conn).await?;
        }
        for input in &self.observed_inputs {
            input.require_current(conn).await?;
        }
        self.parent_fence.ensure_fresh_on(conn).await?;
        if let Some(empty) = &self.empty_outcome {
            empty.ensure_fresh_on(conn).await?;
        }
        if let Some(array) = &self.array {
            array.ensure_fresh_on(conn).await?;
        }
        if let Some(control) = &self.iteration_control {
            control.ensure_fresh_on(conn).await?;
        }
        for child in &self.children {
            child.admission.ensure_fresh_on(conn).await?;
            for field in &child.fields {
                field.rendered.ensure_fresh_on(conn).await?;
            }
        }
        for receipt in self.controls.iter().chain(&self.receipts) {
            receipt.ensure_fresh_on(conn).await?;
        }
        for fence in &self.recovery_fences {
            fence.ensure_fresh_on(conn).await?;
        }
        for outcome in self.observed_outcomes.iter().chain(&self.original_outcomes) {
            outcome.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if !self.finished {
            return Err(changed());
        }
        self.ensure_source_fresh()
    }
    fn ensure_source_fresh(&self) -> Result<()> {
        if let Some(root) = &self.empty_root {
            root.ensure_fresh()?;
        }
        if let Some(control) = &self.empty_control {
            control.ensure_fresh()?;
        }
        if let Some(empty) = &self.empty_outcome {
            empty.ensure_fresh()?;
        }
        if let Some(frame) = &self.context.frame {
            frame.ensure_fresh()?;
        }
        self.parent_fence.ensure_fresh()?;
        if let Some(control) = &self.iteration_control {
            control.ensure_fresh()?;
        }
        for child in &self.children {
            child.admission.effect_fence().ensure_fresh()?;
        }
        for receipt in self.controls.iter().chain(&self.receipts) {
            receipt.ensure_fresh()?;
        }
        for fence in &self.recovery_fences {
            fence.ensure_fresh()?;
        }
        for outcome in self.observed_outcomes.iter().chain(&self.original_outcomes) {
            outcome.ensure_fresh()?;
        }
        Ok(())
    }
    /// No source discovery or expression evaluation is allowed after this lock.
    pub async fn apply(
        &mut self,
        conn: &mut PgConnection,
        key: &str,
    ) -> Result<NativeIterationResult> {
        // A frame consumer must publish once at the owning fan-out boundary,
        // never apply an individual target before its original VALUE stages.
        if self.context.frame.is_some() {
            return Err(changed());
        }
        self.apply_inner(conn, key, None).await
    }
    async fn apply_inner(
        &mut self,
        conn: &mut PgConnection,
        key: &str,
        published: Option<(&WorkflowExecution, &[StoredSecretValue])>,
    ) -> Result<NativeIterationResult> {
        // Reject foreign/expired source tokens before taking target locks or
        // consuming the genuine issuing plan.
        self.ensure_fresh_on(conn).await?;
        if self.applied {
            return Err(changed());
        }
        self.applied = true;
        if published.is_none() {
            if let Some(empty) = &self.empty_outcome {
                empty.guard_original(conn).await?;
            }
        }
        let current = Workflows::find_by_id_for_update(&mut *conn, self.context.workflow.id)
            .await?
            .ok_or_else(changed)?;
        let parent = Executions::find_by_id(&mut *conn, self.context.parent.id)
            .await?
            .ok_or_else(changed)?;
        let (expected_workflow, expected_secrets) =
            published.unwrap_or((&self.context.workflow, &self.original_secrets));
        if serde_json::to_value(&current)? != serde_json::to_value(expected_workflow)?
            || serde_json::to_value(parent)? != serde_json::to_value(&self.context.parent)?
            || Secrets::find_stored_by_entity(
                &mut *conn,
                ENTITY_WORKFLOW_VARIABLES,
                self.context.parent.id,
            )
            .await?
                != expected_secrets
        {
            return Err(changed());
        }
        let actual = initial_children(
            conn,
            self.context.parent.id,
            self.workflow(),
            &self.context.task_name,
        )
        .await?;
        if serde_json::to_value(actual)? != serde_json::to_value(&self.original_children)? {
            return Err(changed());
        }
        if original_dispatch_count(conn, self.workflow(), &self.context.task_name).await?
            != self.original_children.len()
            || latest_observations(
                conn,
                self.context.parent.id,
                self.workflow(),
                &self.context.task_name,
            )
            .await?
                != self.observations
        {
            return Err(changed());
        }
        if active_attempts(conn, self.workflow(), &self.context.task_name).await?
            != self.active_attempts
        {
            return Err(changed());
        }
        let remaining = Executions::summarize_workflow_task_latest_attempts(
            &mut *conn,
            self.workflow(),
            &self.context.task_name,
        )
        .await?
        .in_flight;
        if remaining
            != self
                .incomplete_attempts
                .checked_add(i64::try_from(self.children.len()).map_err(|_| changed())?)
                .ok_or_else(changed)?
        {
            return Err(changed());
        }
        self.ensure_fresh_on(conn).await?;
        if let Some(empty) = &self.empty_outcome {
            let source = self.empty_source()?;
            if published.is_none() {
                source.control().require_original_workflow(&current)?;
                source.control().finalize(conn).await?;
            }
            empty.release(conn, &source).await?;
            empty.require_released(conn).await?;
        }
        for child in &mut self.children {
            Executions::bind_prepared_workflow_task(conn, &child.row).await?;
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
            let contract = crate::value_schema::CompiledContract::new(&child.contract)
                .map_err(|_| changed())?;
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
        }
        let settled = (self.empty_root.is_some()
            || (self.total != 0 && self.can_finalize_status_only()))
            && remaining == 0
            && (self.total == 0 || self.observations.len() == self.total);
        let succeeded = self
            .observations
            .iter()
            .all(|outcome| outcome.status == ExecutionStatus::Completed);
        let completed_parent = if settled {
            let root_output = if let Some(root) = &self.empty_root {
                let receipt = Inputs::record_empty_root(
                    conn,
                    root,
                    self.empty_outcome.as_ref().ok_or_else(changed)?,
                    key,
                )
                .await?;
                let sources = vec![SecretPathSource {
                    path: String::new(),
                    source: SecretSource::Bound(receipt.provenance()),
                }];
                let (value, secrets) = crate::secret_values::redact_secret_path_sources(
                    root.value().clone(),
                    &sources,
                );
                let secrets = crate::secret_values::prepare_secret_values(secrets, key)?;
                Secrets::replace_entity_with_conn(
                    &mut *conn,
                    "execution_result",
                    self.context.parent.id,
                    &secrets,
                )
                .await?;
                self.receipts.push(receipt);
                Some(value)
            } else {
                None
            };
            let mut completed_tasks = self.context.workflow.completed_tasks.clone();
            let mut failed_tasks = self.context.workflow.failed_tasks.clone();
            if succeeded && !completed_tasks.contains(&self.context.task_name) {
                completed_tasks.push(self.context.task_name.clone());
            }
            if !succeeded && !failed_tasks.contains(&self.context.task_name) {
                failed_tasks.push(self.context.task_name.clone());
            }
            if succeeded {
                failed_tasks.retain(|name| name != &self.context.task_name);
            }
            let status = if succeeded {
                ExecutionStatus::Completed
            } else {
                ExecutionStatus::Failed
            };
            let current_tasks = self
                .context
                .workflow
                .current_tasks
                .iter()
                .filter(|name| name.as_str() != self.context.task_name.as_str())
                .cloned()
                .collect();
            Workflows::update(
                &mut *conn,
                self.workflow(),
                UpdateWorkflowExecutionInput {
                    status: Some(status),
                    completed_tasks: Some(completed_tasks),
                    failed_tasks: Some(failed_tasks),
                    current_tasks: Some(current_tasks),
                    ..Default::default()
                },
            )
            .await?;
            // Nonempty aggregation keeps its status-only contract. Empty output
            // above comes from the finite recipe, never an invented child VALUE.
            super::runtime_read::RuntimeReadRepository::close_execution(
                conn,
                self.context.parent.id,
            )
            .await?;
            Some(
                Executions::update(
                    &mut *conn,
                    self.context.parent.id,
                    UpdateExecutionInput {
                        status: Some(status),
                        result: root_output,
                        ..Default::default()
                    },
                )
                .await?,
            )
        } else {
            None
        };
        self.finished = true;
        self.ensure_fresh_on(conn).await?;
        let rows = self
            .children
            .iter()
            .map(|child| &child.row)
            .chain(self.recovered.iter())
            .collect::<Vec<_>>();
        let in_flight = usize::try_from(self.active_attempts).map_err(|_| changed())?;
        let requested = rows
            .into_iter()
            .filter(|child| child.status == ExecutionStatus::Requested)
            .take(self.concurrency.saturating_sub(in_flight))
            .cloned()
            .collect();
        Ok(NativeIterationResult {
            requested,
            total: self.total,
            remaining,
            completed_parent,
        })
    }
}

async fn original_dispatch_count(
    conn: &mut PgConnection,
    workflow: Id,
    task: &str,
) -> Result<usize> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM workflow_task_dispatch WHERE workflow_execution=$1 AND task_name=$2 AND task_index IS NOT NULL")
        .bind(workflow).bind(task).fetch_one(conn).await?;
    let count = usize::try_from(count).map_err(|_| changed())?;
    if count > MAX_CHILDREN {
        return Err(changed());
    }
    Ok(count)
}

async fn latest_observations(
    conn: &mut PgConnection,
    parent: Id,
    workflow: Id,
    task: &str,
) -> Result<Vec<IterationObservation>> {
    let rows: Vec<IterationObservation> = sqlx::query_as("SELECT execution,created,original,ordinal,batch,attempt,metadata_attempt,status FROM (SELECT id AS execution,created,COALESCE(original_execution,id) AS original,(workflow_task->>'task_index')::INT AS ordinal,(workflow_task->>'task_batch')::INT AS batch,retry_count AS attempt,(workflow_task->>'retry_count')::INT AS metadata_attempt,status,ROW_NUMBER() OVER(PARTITION BY (workflow_task->>'task_index')::INT ORDER BY retry_count DESC,created DESC,id DESC) AS rank FROM execution e WHERE parent=$1 AND workflow_task->>'workflow_execution'=$2::TEXT AND workflow_task->>'task_name'=$3 AND workflow_task->>'task_index' IS NOT NULL AND EXISTS(SELECT 1 FROM workflow_task_dispatch d WHERE d.workflow_execution=$4 AND d.task_name=$3 AND d.task_index=(e.workflow_task->>'task_index')::INT)) observed WHERE rank=1 ORDER BY ordinal LIMIT $5")
        .bind(parent).bind(workflow.to_string()).bind(task).bind(workflow).bind((MAX_CHILDREN+1) as i64).fetch_all(conn).await?;
    if rows.len() > MAX_CHILDREN {
        return Err(changed());
    }
    Ok(rows)
}

async fn latest_named_sources(
    conn: &mut PgConnection,
    parent: Id,
    workflow: Id,
) -> Result<Vec<Execution>> {
    // One actual latest attempt per non-iteration task. No iteration history is
    // fetched merely to bind a finite task/result namespace.
    let query = format!("SELECT {SELECT_COLUMNS} FROM execution WHERE id IN (SELECT DISTINCT ON(workflow_task->>'task_name') id FROM execution WHERE parent=$1 AND workflow_task->>'workflow_execution'=$2::TEXT AND workflow_task->>'task_index' IS NULL ORDER BY workflow_task->>'task_name',retry_count DESC,created DESC,id DESC) ORDER BY id LIMIT 257");
    let rows: Vec<Execution> = sqlx::query_as(&query)
        .bind(parent)
        .bind(workflow.to_string())
        .fetch_all(conn)
        .await?;
    if rows.len() > 256 {
        return Err(changed());
    }
    Ok(rows)
}

async fn active_attempts(conn: &mut PgConnection, workflow: Id, task: &str) -> Result<i64> {
    // Aggregate current retry attempts without materializing their history. A
    // retry remains I's creation/publication responsibility, but occupies a slot.
    Ok(sqlx::query_scalar("WITH latest AS (SELECT status,ROW_NUMBER() OVER(PARTITION BY (workflow_task->>'task_index')::INT ORDER BY retry_count DESC,created DESC,id DESC) AS rank FROM execution WHERE workflow_task->>'workflow_execution'=$1::TEXT AND workflow_task->>'task_name'=$2 AND workflow_task->>'task_index' IS NOT NULL) SELECT COUNT(*)::BIGINT FROM latest WHERE rank=1 AND status NOT IN ('requested','completed','failed','timeout','cancelled','abandoned')")
        .bind(workflow.to_string()).bind(task).fetch_one(conn).await?)
}

async fn initial_children(
    conn: &mut PgConnection,
    parent: Id,
    workflow: Id,
    task: &str,
) -> Result<Vec<Execution>> {
    // Reserved rows have metadata, but no dispatch identity until apply binds it.
    let query = format!("SELECT {SELECT_COLUMNS} FROM execution WHERE parent=$1 AND workflow_task->>'workflow_execution'=$2::TEXT AND workflow_task->>'task_name'=$3 AND workflow_task->>'task_index' IS NOT NULL AND original_execution IS NULL AND EXISTS(SELECT 1 FROM workflow_task_dispatch d WHERE d.execution_id=execution.id AND d.workflow_execution=(execution.workflow_task->>'workflow_execution')::BIGINT AND d.task_name=$3 AND d.task_index=(execution.workflow_task->>'task_index')::INT) ORDER BY (workflow_task->>'task_index')::INT,id LIMIT $4");
    let rows: Vec<Execution> = sqlx::query_as(&query)
        .bind(parent)
        .bind(workflow.to_string())
        .bind(task)
        .bind((MAX_CHILDREN + 1) as i64)
        .fetch_all(conn)
        .await?;
    if rows.len() > MAX_CHILDREN {
        return Err(changed());
    }
    Ok(rows)
}
// Observation of an EXISTING first seal, not input delivery to a terminal
// process. Canonical verifiers own acceptance/archive/contract/ceiling parsing.
async fn observe_accepted_inputs(
    conn: &mut PgConnection,
    source: &Execution,
    key: &str,
) -> Result<(ObservedInputState, ApprovalEffectFence, Vec<ReceiptContext>)> {
    let schema = Schemas::find(conn, BindingOwner::Execution, source.id)
        .await?
        .ok_or_else(changed)?;
    schema.validate()?;
    let fence = AcceptedInputRepository::revalidate_execution(conn, source.id, key).await?;
    let (fields, receipts) = prepare_observation(conn, source, &schema, key).await?;
    fence.ensure_fresh_on(conn).await?;
    Ok((
        ObservedInputState {
            source: source.clone(),
            fields,
            schema,
        },
        fence,
        receipts,
    ))
}

fn positive_optional(value: Option<&Value>) -> Result<Option<usize>> {
    value
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v > 0)
                .ok_or_else(changed)
        })
        .transpose()
}
fn seconds(value: &Value) -> Result<i32> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .and_then(|v| i32::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or_else(changed)
}
fn permission_refs(value: Value) -> Result<Vec<String>> {
    let values = match value {
        Value::Null => vec![],
        Value::String(s) => vec![Value::String(s)],
        Value::Array(v) => v,
        _ => return Err(changed()),
    };
    let mut seen = BTreeSet::new();
    let mut refs = Vec::new();
    for value in values {
        let value = value.as_str().ok_or_else(changed)?.trim();
        if !value.is_empty() && seen.insert(value.to_owned()) {
            refs.push(value.to_owned());
        }
    }
    Ok(refs)
}
fn changed() -> Error {
    Error::PermissionDenied(
        "Ordinary iteration source, ordinal, variables or child identity changed".into(),
    )
}

fn preparation_error(error: Error, stage: &'static str) -> Error {
    match error {
        Error::PermissionDenied(message)
            if !message.starts_with("Ordinary iteration preparation [")
                && !message.starts_with("Iteration control ") =>
        {
            Error::PermissionDenied(format!("Ordinary iteration preparation [{stage}] denied"))
        }
        other => other,
    }
}
