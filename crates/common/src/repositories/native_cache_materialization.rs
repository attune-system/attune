//! Atomic cache entry/refill and publication-driven activation. Selected reads, pages and child
//! destinations are prepared before workflow/frontier target locks.
use super::{
    declared_input::{DeclaredInputBinding, DeclaredInputRepository as Declarations},
    declared_input_materialization::{
        pointer_to_node, DeclaredInputMaterializationRepository as Materialize,
    },
    execution::{CreateExecutionInput, ExecutionRepository as Executions, UpdateExecutionInput},
    execution_runtime_admission::{ExecutionAdmission, ExecutionAdmissionRepository as Admissions},
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        PreparedInputSource, PreparedReceiptDestination, ReceiptContext, RenderedReceiptContext,
    },
    native_input_snapshot::{
        CacheSelectorControl, NativeInputSnapshotRepository as Snapshots, PreparedCacheInput,
    },
    native_transition_materialization::{prepare_sources, PreparedNativeTransitions, Sources},
    native_workflow_materialization::{
        check_bounds, collect_leaves, contains_template, expression_names,
    },
    runtime_authority::{RuntimeAuthorityRepository as Authority, RuntimeOwner},
    schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
    workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
    workflow_cache_iteration::{
        UpdateWorkflowCacheIterationProgressInput, WorkflowCacheIterationRepository as Iterations,
    },
    workflow_policy::{task_at_authored_path, WorkflowPolicyRepository},
    FindById, Update,
};
use crate::{
    models::{
        Execution, ExecutionStatus, Id, WorkflowCacheIteration, WorkflowCacheIterationState,
        WorkflowExecution, WorkflowTaskMetadata,
    },
    runtime_authority::InvocationBinding,
    secret_values::{SecretPathSource, SecretSource, ENTITY_WORKFLOW_VARIABLES},
    Error, Result,
};
use serde_json::{json, Value};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod activation;
mod completion;
mod frame;
pub(super) mod profile;
pub use activation::PreparedNativeCacheWork;
pub use completion::{NativeCacheCompletionAttempt, SourceCacheCompletion};
pub(super) use frame::NativeCacheRenderFrame;
use profile::{measure, Operation as Op};
pub use profile::{NativeCachePreparationMetric, NativeCachePreparationProfile};

const MAX_PAGE_BYTES: usize = 1024 * 1024;
const MAX_CHECKPOINT_BATCHES: usize = 64;

struct Context {
    parent: Execution,
    workflow: WorkflowExecution,
    task: Value,
    path: String,
    revision: String,
    bindings: BTreeMap<String, DeclaredInputBinding>,
    sources: Arc<Sources>,
}
struct Control {
    site: InputSite,
    rendered: RenderedReceiptContext,
    // Issued selectors use A's prepare_control_source, which prepares this exact
    // destination itself. Other controls must carry their prepared destination.
    destination: Option<PreparedReceiptDestination>,
}
struct Field {
    path: String,
    rendered: RenderedReceiptContext,
    destination: PreparedReceiptDestination,
}
struct Batch {
    page: Arc<PreparedCacheInput>,
    child: Option<Execution>,
    admission: Option<Id>,
    contract: Option<crate::value_schema::Contract>,
    value: Value,
    fields: Vec<Field>,
    controls: Vec<Control>,
    defaults: Vec<SecretPathSource>,
}
pub struct PreparedNativeCacheCheckpoint {
    context: Context,
    admission: ExecutionAdmission,
    controls: Arc<Vec<ReceiptContext>>,
    issued_selectors: Arc<Vec<PreparedInputSource>>,
    default_receipts: Vec<ReceiptContext>,
    batches: Vec<Batch>,
    original: Option<WorkflowCacheIteration>,
    in_flight: i64,
    pending: Vec<Execution>,
    pending_fences: Vec<super::runtime_authority::ApprovalEffectFence>,
    original_secrets: Vec<crate::secret_values::StoredSecretValue>,
    receipts: std::sync::OnceLock<Vec<ReceiptContext>>,
    frame: Option<Arc<dyn NativeCacheRenderFrame>>,
    completion: Option<SourceCacheCompletion>,
}
pub struct NativeCacheCheckpointResult {
    pub children: Vec<Execution>,
    pub completed: Option<Execution>,
}

impl Context {
    async fn render(
        &self,
        conn: &mut PgConnection,
        relative: &str,
        purpose: InputSitePurpose,
        cache: Option<&PreparedCacheInput>,
        frame: Option<&dyn NativeCacheRenderFrame>,
        key: &str,
    ) -> Result<RenderedReceiptContext> {
        let path = format!("{}{relative}", self.path);
        let authored = self.task.pointer(relative).ok_or_else(denied)?;
        let Some(binding) = self.bindings.get(&path) else {
            if purpose != InputSitePurpose::Value || contains_template(authored) {
                return Err(denied());
            }
            let mut literal = measure(
                Op::Literal,
                Inputs::literal(conn, self.parent.id, &self.revision, &path),
            )
            .await?;
            if let Some(frame) = frame {
                frame.attach_literal(&mut literal, &self.sources)?;
            }
            return Ok(literal);
        };
        let mut session = measure(
            Op::SessionBegin,
            Inputs::begin_render(
                conn,
                self.parent.id,
                &binding.declaration_revision,
                &path,
                &binding.expression,
                purpose,
            ),
        )
        .await?;
        if let Some(frame) = frame {
            measure(
                Op::FrameBinding,
                frame.bind(conn, &mut session, binding, &self.sources, key),
            )
            .await
            .map_err(|error| preparation_stage(error, "frame-binding"))?;
            if let Some(cache) = cache {
                Inputs::bind_cache_input(&mut session, cache)
                    .map_err(|error| preparation_stage(error, "cache-input-binding"))?;
            }
            return measure(
                Op::Expression,
                Inputs::evaluate_with_keys(conn, session, key),
            )
            .await
            .map_err(|error| preparation_stage(error, "frame-evaluation"));
        }
        let names = expression_names(&binding.expression)?;
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
                    Inputs::bind_public_field_with_key(conn, &mut session, slot, &field.site, key)
                        .await?;
                }
            }
        }
        if let Some(cache) = cache {
            Inputs::bind_cache_input(&mut session, cache)?;
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
        measure(
            Op::Expression,
            Inputs::evaluate_with_keys(conn, session, key),
        )
        .await
    }
    async fn metadata(
        &self,
        conn: &mut PgConnection,
        field: &str,
        cache: Option<&PreparedCacheInput>,
        frame: Option<&dyn NativeCacheRenderFrame>,
        key: &str,
    ) -> Result<(Option<Value>, Vec<Control>)> {
        let Some(authored) = self.task.pointer(field).filter(|value| !value.is_null()) else {
            return Ok((None, vec![]));
        };
        let mut value = authored.clone();
        let mut leaves = Vec::new();
        collect_leaves(authored, String::new(), &mut leaves, 0)?;
        let mut controls = Vec::new();
        for (suffix, leaf) in leaves {
            if !contains_template(&leaf) {
                continue;
            }
            let path = format!("{field}{suffix}");
            let rendered = self
                .render(conn, &path, InputSitePurpose::Control, cache, frame, key)
                .await?;
            *value.pointer_mut(&suffix).ok_or_else(denied)? = rendered.value().clone();
            let site = InputSite {
                execution: self.parent.id,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Control,
                path: format!("{}{path}", self.path),
            };
            let issued_selector = frame.is_some()
                && matches!(
                    field,
                    "/iterate_cache/namespace"
                        | "/iterate_cache/generation"
                        | "/iterate_cache/owner_type"
                        | "/iterate_cache/owner_ref"
                );
            let destination = if issued_selector {
                None
            } else {
                Some(
                    measure(
                        Op::ControlDestination,
                        Inputs::prepare_destination(conn, &rendered, &site, key),
                    )
                    .await?,
                )
            };
            controls.push(Control {
                site,
                rendered,
                destination,
            });
        }
        Ok((Some(value), controls))
    }
}

pub struct NativeCacheMaterializationRepository;
impl NativeCacheMaterializationRepository {
    /// Terminal cache streams can run beside other tasks. Publication-driven
    /// activation uses the owning prepare_work adapter; this checkpoint handles
    /// later committed-input refill without replacing sibling task progress.
    pub fn supports(parent: &Execution, task: &str) -> bool {
        if parent.executable_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .executable
                .action
                .out_schema
                .as_ref()
                .is_some_and(|schema| {
                    !schema.is_null() && schema.as_object().is_none_or(|fields| !fields.is_empty())
                })
        }) {
            return false;
        }
        let Some(graph) = parent
            .executable_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
        else {
            return false;
        };
        let graph = &graph.definition;
        if ["output", "output_map"].iter().any(|field| {
            graph.get(*field).is_some_and(|value| {
                !value.is_null() && value.as_object().is_none_or(|value| !value.is_empty())
            })
        }) {
            return false;
        }
        let Some(tasks) = graph.get("tasks").and_then(Value::as_array) else {
            return false;
        };
        if tasks.len() > MAX_CHECKPOINT_BATCHES {
            return false;
        }
        let Some(node) = tasks
            .iter()
            .find(|node| node.get("name").and_then(Value::as_str) == Some(task))
        else {
            return false;
        };
        Self::terminal_cache_node(node)
    }

    fn terminal_cache_node(node: &Value) -> bool {
        node.get("name").is_some_and(Value::is_string)
            && node.get("action").is_some_and(Value::is_string)
            && ["input", "params"].iter().any(|field| {
                node.get(*field)
                    .and_then(Value::as_object)
                    .is_some_and(|fields| !fields.is_empty())
            })
            && node
                .get("iterate_cache")
                .is_some_and(|value| !value.is_null())
            && [
                "next",
                "publish",
                "with_items",
                "wait_for",
                "tasks",
                "on_success",
                "on_failure",
                "on_complete",
                "on_timeout",
            ]
            .iter()
            .all(|field| {
                node.get(*field).is_none_or(|value| {
                    value.is_null()
                        || value.as_array().is_some_and(Vec::is_empty)
                        || value.as_object().is_some_and(serde_json::Map::is_empty)
                })
            })
    }

    pub async fn prepare(
        conn: &mut PgConnection,
        parent: &Execution,
        task_name: &str,
        key: &str,
    ) -> Result<PreparedNativeCacheCheckpoint> {
        Self::prepare_inner(conn, parent, task_name, None, None, key).await
    }

    /// Source-only bridge for J's genuine terminal wait stage. There is no
    /// completed/synthetic execution or replacement JSON parameter. The owner
    /// must release its real observation and publish its actual stages before
    /// apply; the retained opaque frame verifies that publication itself.
    pub(super) async fn prepare_successor_from_frame(
        conn: &mut PgConnection,
        parent: &Execution,
        task_name: &str,
        frame: Arc<dyn NativeCacheRenderFrame>,
        key: &str,
    ) -> Result<PreparedNativeCacheCheckpoint> {
        if frame.execution() != parent.id {
            return Err(denied());
        }
        frame.ensure_fresh_on(conn).await?;
        Self::prepare_inner(conn, parent, task_name, None, Some(frame), key).await
    }

    async fn prepare_inner(
        conn: &mut PgConnection,
        parent: &Execution,
        task_name: &str,
        completed: Option<&Execution>,
        frame: Option<Arc<dyn NativeCacheRenderFrame>>,
        key: &str,
    ) -> Result<PreparedNativeCacheCheckpoint> {
        if !Self::supports(parent, task_name) {
            return Err(denied());
        }
        let admitted = Admissions::require(conn, RuntimeOwner::Execution(parent.id)).await?;
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(denied)?;
        let graph = &snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
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
            return Err(denied());
        };
        let task = task_at_authored_path(graph, path).ok_or_else(denied)?;
        let path = format!("/workflow{}", pointer_to_node(graph, task)?);
        let task = task.clone();
        let workflow = Workflows::find_by_execution(&mut *conn, parent.id)
            .await?
            .ok_or_else(denied)?;
        if parent.status != ExecutionStatus::Running || workflow.status != ExecutionStatus::Running
        {
            return Err(denied());
        }
        let sources = if let Some(frame) = &frame {
            measure(
                Op::ReuseSources,
                frame.prepared_sources(conn, parent, &workflow, key),
            )
            .await?
        } else {
            None
        };
        let sources = match sources {
            Some(sources) => sources,
            None => Arc::new(
                measure(
                    Op::RootSources,
                    prepare_sources(conn, parent, completed, &workflow, key),
                )
                .await?
                .ok_or_else(denied)?,
            ),
        };
        let context = Context {
            parent: parent.clone(),
            workflow,
            task,
            path,
            sources,
            revision: crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
                .map_err(|_| denied())?,
            bindings: Declarations::bindings(conn, parent.id)
                .await?
                .into_iter()
                .map(|binding| (binding.source_path.clone(), binding))
                .collect(),
        };
        let mut controls = Vec::new();
        let (action, rendered) = context
            .metadata(conn, "/action", None, frame.as_deref(), key)
            .await?;
        record_controls(conn, rendered, key, &mut controls).await?;
        let action = action
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(denied)?;
        let executable =
            Admissions::resolve_workflow_snapshot(conn, parent.id, task_name, &action).await?;
        let (refs, rendered) = context
            .metadata(conn, "/permission_set_refs", None, frame.as_deref(), key)
            .await?;
        record_controls(conn, rendered, key, &mut controls).await?;
        let refs = match refs {
            Some(value) => permission_refs(value)?,
            None => executable
                .executable
                .action
                .default_execution_permission_set_refs
                .clone(),
        };
        let mut selector_controls = Vec::new();
        let mut issued_selectors = Vec::new();
        for field in ["namespace", "generation", "owner_type", "owner_ref"] {
            let (_, rendered) = context
                .metadata(
                    conn,
                    &format!("/iterate_cache/{field}"),
                    None,
                    frame.as_deref(),
                    key,
                )
                .await?;
            if frame.is_some() {
                for control in rendered {
                    issued_selectors.push(
                        measure(
                            Op::SelectorProof,
                            Inputs::prepare_control_source(
                                conn,
                                &control.rendered,
                                &control.site,
                                key,
                            ),
                        )
                        .await?,
                    );
                }
            } else {
                record_controls(conn, rendered, key, &mut selector_controls).await?;
            }
        }
        let closure = measure(
            Op::NativeClosure,
            super::native_input_origin::NativeInputOriginRepository::capture_with_key(
                conn,
                parent.id,
                &[],
                Some(key),
            ),
        )
        .await?;
        let admission = measure(
            Op::TaskAdmission,
            Admissions::workflow_child_prepared(
                conn,
                parent.id,
                task_name,
                &executable,
                &refs,
                &closure,
            ),
        )
        .await?;
        let original =
            Iterations::find_by_workflow_task(&mut *conn, context.workflow.id, task_name).await?;
        if original
            .as_ref()
            .is_some_and(|row| row.state != WorkflowCacheIterationState::Scanning)
        {
            return Err(denied());
        }
        let summary = Executions::summarize_workflow_task_latest_attempts(
            &mut *conn,
            context.workflow.id,
            task_name,
        )
        .await?;
        if summary.has_failed {
            return Err(denied());
        }
        let concurrency = positive(context.task.get("concurrency"), 1)?;
        let slots = concurrency
            .saturating_sub(usize::try_from(summary.in_flight).map_err(|_| denied())?)
            .min(MAX_CHECKPOINT_BATCHES);
        let mut batches: Vec<Batch> = Vec::new();
        let mut default_receipts = Vec::new();
        let mut pending_fences = Vec::new();
        let pending = Executions::find_requested_workflow_task(
            &mut *conn,
            context.workflow.id,
            task_name,
            MAX_CHECKPOINT_BATCHES as i64,
        )
        .await?;
        for child in &pending {
            if child.parent != Some(parent.id)
                || serde_json::to_value(&child.executable_snapshot)?
                    != serde_json::to_value(Some(&executable))?
            {
                return Err(denied());
            }
            let admitted = Admissions::require(conn, RuntimeOwner::Execution(child.id)).await?;
            pending_fences.push(Authority::retained_admission_fence(conn, admitted.id).await?);
            default_receipts
                .extend(Materialize::prepare_execution_input_receipts(conn, child.id, key).await?);
        }
        for _ in 0..slots {
            let page = if let Some(previous) = batches.last() {
                measure(
                    Op::CachePage,
                    Snapshots::prepare_cache_next(conn, &previous.page, MAX_PAGE_BYTES, key),
                )
                .await?
            } else if let Some(row) = &original {
                measure(
                    Op::CachePage,
                    Snapshots::prepare_cache_with_controls(
                        conn,
                        parent.id,
                        &context.path,
                        &admission,
                        row.id,
                        MAX_PAGE_BYTES,
                        &selector_controls.iter().collect::<Vec<_>>(),
                        key,
                    ),
                )
                .await?
            } else if !issued_selectors.is_empty() {
                let selectors = selector_controls
                    .iter()
                    .map(CacheSelectorControl::recorded)
                    .chain(issued_selectors.iter().map(CacheSelectorControl::prepared))
                    .collect::<Vec<_>>();
                measure(
                    Op::CachePage,
                    Snapshots::prepare_cache_initial_from_controls(
                        conn,
                        parent.id,
                        &context.path,
                        &admission,
                        context.workflow.id,
                        MAX_PAGE_BYTES,
                        &selectors,
                        key,
                    ),
                )
                .await?
            } else {
                measure(
                    Op::CachePage,
                    Snapshots::prepare_cache_initial(
                        conn,
                        parent.id,
                        &context.path,
                        &admission,
                        context.workflow.id,
                        MAX_PAGE_BYTES,
                        &selector_controls.iter().collect::<Vec<_>>(),
                        key,
                    ),
                )
                .await?
            };
            if page.count() == 0 {
                batches.push(Batch {
                    page: Arc::new(page),
                    child: None,
                    admission: None,
                    contract: None,
                    value: json!({}),
                    fields: vec![],
                    controls: vec![],
                    defaults: vec![],
                });
                break;
            }
            let mut batch_controls = Vec::new();
            let mut metadata = BTreeMap::new();
            for field in [
                "timeout",
                "trace_tag_template",
                "worker_selector",
                "worker_tolerations",
                "worker_affinity",
            ] {
                let (value, rendered) = context
                    .metadata(
                        conn,
                        &format!("/{field}"),
                        Some(&page),
                        frame.as_deref(),
                        key,
                    )
                    .await?;
                metadata.insert(field, value);
                batch_controls.extend(rendered);
            }
            let timeout = metadata["timeout"].as_ref().map(seconds).transpose()?;
            let trace_tag = match metadata["trace_tag_template"].as_ref() {
                None | Some(Value::Null) => parent.trace_tag.clone(),
                Some(value) => {
                    let text = value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string());
                    if text.trim().is_empty() {
                        parent.trace_tag.clone()
                    } else {
                        Some(crate::trace_tag::normalize_trace_tag(&text)?)
                    }
                }
            };
            if let Some(value) = metadata["worker_selector"].as_ref() {
                crate::scheduling::parse_worker_selector(value)?;
            }
            if let Some(value) = metadata["worker_tolerations"].as_ref() {
                crate::scheduling::parse_worker_tolerations(value)?;
            }
            if let Some(value) = metadata["worker_affinity"].as_ref() {
                crate::scheduling::parse_worker_affinity(value)?;
            }
            let batch_index = i32::try_from(page.index()).map_err(|_| denied())?;
            let schema = measure(
                Op::SchemaBind,
                Admissions::workflow_schema(conn, parent.id, &executable),
            )
            .await?;
            let child = measure(
                Op::ChildCreate,
                Executions::create_pinned(
                    &mut *conn,
                    CreateExecutionInput {
                        action: Some(executable.executable.action.id),
                        action_ref: action.clone(),
                        parent: Some(parent.id),
                        executor: parent.executor,
                        enforcement: parent.enforcement,
                        env_vars: parent.env_vars.clone(),
                        config: None,
                        permission_set_refs: refs.clone(),
                        status: ExecutionStatus::Requested,
                        trace_tag,
                        timeout_seconds: Some(
                            timeout
                                .map(|value| i32::try_from(value).map_err(|_| denied()))
                                .transpose()?
                                .or(executable.executable.action.timeout_seconds)
                                .unwrap_or(
                                    crate::config::app_default_execution_timeout_seconds() as i32
                                ),
                        ),
                        worker_selector: metadata.remove("worker_selector").flatten(),
                        worker_tolerations: metadata.remove("worker_tolerations").flatten(),
                        worker_affinity: metadata.remove("worker_affinity").flatten(),
                        artifact_retention_policy: parent
                            .artifact_retention_policy
                            .or(executable.executable.action.artifact_retention_policy),
                        artifact_retention_limit: parent
                            .artifact_retention_limit
                            .or(executable.executable.action.artifact_retention_limit),
                        workflow_task: Some(WorkflowTaskMetadata {
                            workflow_execution: context.workflow.id,
                            task_name: task_name.into(),
                            task_index: Some(batch_index),
                            task_batch: Some(i32::try_from(page.count()).map_err(|_| denied())?),
                            triggered_by: None,
                            retry_count: 0,
                            max_retries: context
                                .task
                                .pointer("/retry/count")
                                .and_then(Value::as_i64)
                                .unwrap_or(0)
                                .try_into()
                                .map_err(|_| denied())?,
                            timeout_seconds: timeout
                                .map(|value| i32::try_from(value).map_err(|_| denied()))
                                .transpose()?,
                            next_retry_at: None,
                            timed_out: false,
                            duration_ms: None,
                            started_at: None,
                            completed_at: None,
                        }),
                        ..Default::default()
                    },
                    &executable,
                ),
            )
            .await?;
            let child_admission = measure(
                Op::TaskAdmission,
                admission.attach(conn, RuntimeOwner::Execution(child.id)),
            )
            .await?;
            super::rule_authority::RuleAuthorityRepository::copy_execution(
                conn, parent.id, child.id,
            )
            .await?;
            super::recorded_input_authority::RecordedInputAuthorityRepository::copy(
                conn,
                "execution",
                parent.id,
                "execution",
                child.id,
            )
            .await?;
            measure(
                Op::SchemaBind,
                Schemas::attach(conn, BindingOwner::Execution, child.id, &schema),
            )
            .await?;
            let input_field = if context
                .task
                .get("input")
                .is_some_and(|value| value.as_object().is_some_and(|value| !value.is_empty()))
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
            if authored.as_object().is_none_or(serde_json::Map::is_empty) {
                return Err(Error::validation(
                    "Native cache checkpoint requires explicit task input",
                ));
            }
            let mut value = authored.clone();
            let mut leaves = Vec::new();
            collect_leaves(&authored, String::new(), &mut leaves, 0)?;
            let mut fields = Vec::new();
            for (path, _) in leaves {
                let rendered = context
                    .render(
                        conn,
                        &format!("/{input_field}{path}"),
                        InputSitePurpose::Value,
                        Some(&page),
                        frame.as_deref(),
                        key,
                    )
                    .await
                    .map_err(|error| preparation_stage(error, "child-input-render"))?;
                *value.pointer_mut(&path).ok_or_else(denied)? = rendered.value().clone();
                let destination = measure(
                    Op::ChildDestination,
                    Inputs::prepare_destination(
                        conn,
                        &rendered,
                        &InputSite {
                            execution: child.id,
                            kind: InputEntityKind::ExecutionConfig,
                            purpose: InputSitePurpose::Value,
                            path: path.clone(),
                        },
                        key,
                    ),
                )
                .await
                .map_err(|error| preparation_stage(error, "child-input-destination"))?;
                let mut field = Field {
                    path,
                    rendered,
                    destination,
                };
                for control in &controls {
                    let source = measure(
                        Op::ReceiptReopen,
                        Inputs::prepare_recorded_source(conn, parent.id, control, key),
                    )
                    .await?;
                    Inputs::attach_prepared_destination_control(
                        &mut field.rendered,
                        &mut field.destination,
                        &source,
                    )?;
                }
                fields.push(field);
            }
            let contract =
                crate::value_schema::CompiledContract::new(&schema.input).map_err(|_| denied())?;
            let defaults = contract.apply_defaults(&value).map_err(|_| denied())?;
            let (value, defaults) = measure(
                Op::ChildDefaults,
                Materialize::action_defaults(conn, child.id, &value, defaults, Some(key)),
            )
            .await?;
            contract.validate(&value).map_err(|_| denied())?;
            check_bounds(&value)?;
            for default in &defaults {
                if let SecretSource::Bound(provenance) = &default.source {
                    for origin in &provenance.origins {
                        default_receipts.push(
                            Inputs::receipt_context(
                                conn,
                                &InputSite {
                                    execution: child.id,
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
            batches.push(Batch {
                page: Arc::new(page),
                child: Some(child),
                admission: Some(child_admission.id),
                contract: Some(schema.input),
                value,
                fields,
                controls: batch_controls,
                defaults,
            });
        }
        controls.extend(selector_controls);
        let original_secrets =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?;
        let controls = Arc::new(controls);
        let issued_selectors = Arc::new(issued_selectors);
        let completion =
            if batches.len() == 1 && batches[0].child.is_none() && summary.in_flight == 0 {
                Some(
                    measure(
                        Op::CompletionSource,
                        SourceCacheCompletion::prepare(
                            conn,
                            &context,
                            original.as_ref(),
                            batches[0].page.clone(),
                            controls.clone(),
                            issued_selectors.clone(),
                            frame.clone(),
                            key,
                        ),
                    )
                    .await
                    .map_err(|error| preparation_stage(error, "empty-completion"))?
                    .ok_or_else(denied)?,
                )
            } else {
                None
            };
        Ok(PreparedNativeCacheCheckpoint {
            context,
            admission,
            controls,
            issued_selectors,
            default_receipts,
            batches,
            original,
            in_flight: summary.in_flight,
            pending,
            pending_fences,
            original_secrets,
            receipts: std::sync::OnceLock::new(),
            frame,
            completion,
        })
    }
}

impl PreparedNativeCacheCheckpoint {
    pub fn completion_source(&self) -> Option<&SourceCacheCompletion> {
        self.completion.as_ref()
    }
    pub fn workflow(&self) -> Id {
        self.context.workflow.id
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        if let Some(source) = &self.completion {
            source.ensure_fresh_on(conn).await?;
        }
        if let Some(frame) = &self.frame {
            frame.ensure_fresh_on(conn).await?;
        }
        self.admission.ensure_fresh_on(conn).await?;
        for selector in self.issued_selectors.iter() {
            selector.ensure_fresh_on(conn).await?;
        }
        for fence in &self.pending_fences {
            fence.ensure_fresh_on(conn).await?;
        }
        for receipt in self
            .controls
            .iter()
            .chain(&self.default_receipts)
            .chain(self.receipts.get().into_iter().flatten())
        {
            receipt.ensure_fresh_on(conn).await?;
        }
        for batch in &self.batches {
            batch.page.ensure_fresh_on(conn).await?;
            for field in &batch.fields {
                field.rendered.ensure_fresh_on(conn).await?;
            }
            for control in &batch.controls {
                control.rendered.ensure_fresh_on(conn).await?;
            }
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Some(source) = &self.completion {
            source.ensure_fresh()?;
        }
        if let Some(frame) = &self.frame {
            frame.ensure_fresh()?;
        }
        self.admission.effect_fence().ensure_fresh()?;
        for selector in self.issued_selectors.iter() {
            selector.ensure_fresh()?;
        }
        for fence in &self.pending_fences {
            fence.ensure_fresh()?;
        }
        for receipt in self
            .controls
            .iter()
            .chain(&self.default_receipts)
            .chain(self.receipts.get().into_iter().flatten())
        {
            receipt.ensure_fresh()?;
        }
        for batch in &self.batches {
            batch.page.ensure_fresh()?;
        }
        Ok(())
    }
    pub async fn apply(
        &mut self,
        conn: &mut PgConnection,
        key: &str,
    ) -> Result<NativeCacheCheckpointResult> {
        if let Some(frame) = &self.frame {
            let baseline = Workflows::find_by_id_for_update(&mut *conn, self.workflow())
                .await?
                .ok_or_else(denied)?;
            frame.require_published(conn).await?;
            frame.ensure_fresh_on(conn).await?;
            let fields = Secrets::find_stored_by_entity(
                &mut *conn,
                ENTITY_WORKFLOW_VARIABLES,
                self.context.parent.id,
            )
            .await?
            .into_iter()
            .map(|field| {
                Ok(crate::secret_values::PreparedSecretValue {
                    json_path: field.json_path,
                    encrypted_value: field.encrypted_value,
                    encryption_key_hash: field.encryption_key_hash.ok_or_else(denied)?,
                    source_kind: field.source_kind,
                    source_ref: field.source_ref,
                })
            })
            .collect::<Result<Vec<_>>>()?;
            self.apply_inner(conn, key, Some((&baseline, &fields)))
                .await
        } else {
            self.apply_inner(conn, key, None).await
        }
    }

    // Called only by the owning source-prepared activation after it has checked
    // and finalized the real predecessor publications under the same outer lock.
    async fn apply_after_publications(
        &mut self,
        conn: &mut PgConnection,
        key: &str,
        baseline: &WorkflowExecution,
        fields: &[crate::secret_values::PreparedSecretValue],
    ) -> Result<NativeCacheCheckpointResult> {
        self.apply_inner(conn, key, Some((baseline, fields))).await
    }

    async fn apply_inner(
        &mut self,
        conn: &mut PgConnection,
        key: &str,
        published: Option<(
            &WorkflowExecution,
            &[crate::secret_values::PreparedSecretValue],
        )>,
    ) -> Result<NativeCacheCheckpointResult> {
        if self.receipts.get().is_some() {
            return Err(denied());
        }
        let workflow = Workflows::find_by_id_for_update(&mut *conn, self.context.workflow.id)
            .await?
            .ok_or_else(denied)?;
        let parent = Executions::find_by_id(&mut *conn, self.context.parent.id)
            .await?
            .ok_or_else(denied)?;
        let stored =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await?;
        let buffers_current = if let Some((baseline, fields)) = published {
            if baseline.id != self.context.workflow.id
                || baseline.execution != self.context.workflow.execution
                || baseline.task_graph != self.context.workflow.task_graph
                || baseline.status != self.context.workflow.status
            {
                return Err(denied());
            }
            let mut actual = stored
                .iter()
                .map(|field| {
                    Ok(crate::secret_values::PreparedSecretValue {
                        json_path: field.json_path.clone(),
                        encrypted_value: field.encrypted_value.clone(),
                        encryption_key_hash: field
                            .encryption_key_hash
                            .clone()
                            .ok_or_else(denied)?,
                        source_kind: field.source_kind.clone(),
                        source_ref: field.source_ref.clone(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mut expected = fields.to_vec();
            actual.sort_by(|a, b| a.json_path.cmp(&b.json_path));
            expected.sort_by(|a, b| a.json_path.cmp(&b.json_path));
            serde_json::to_value(&workflow)? == serde_json::to_value(baseline)?
                && actual == expected
        } else {
            serde_json::to_value(&workflow)? == serde_json::to_value(&self.context.workflow)?
                && stored == self.original_secrets
        };
        if !buffers_current
            || serde_json::to_value(&parent)? != serde_json::to_value(&self.context.parent)?
        {
            return Err(denied());
        }
        self.ensure_fresh_on(conn).await?;
        if let Some(source) = &self.completion {
            source.require_frontier_and_attempts(conn).await?;
        }
        let task_name = self
            .context
            .task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        let summary =
            Executions::summarize_workflow_task_latest_attempts(&mut *conn, workflow.id, task_name)
                .await?;
        // The children reserved in the source phase already count as in flight.
        let reserved = i64::try_from(
            self.batches
                .iter()
                .filter(|batch| batch.child.is_some())
                .count(),
        )
        .map_err(|_| denied())?;
        if summary.has_failed || summary.in_flight != self.in_flight + reserved {
            return Err(denied());
        }
        let current = Iterations::find_by_workflow_task(&mut *conn, workflow.id, task_name).await?;
        if serde_json::to_value(&current)? != serde_json::to_value(&self.original)? {
            return Err(denied());
        }
        let mut iteration = match (&current, self.batches.first()) {
            (Some(row), _) => row.clone(),
            (None, Some(first)) => {
                Iterations::create_or_find_for_update(conn, first.page.initial_create_input()?)
                    .await?
            }
            _ => return Err(denied()),
        };
        let mut children = Vec::new();
        let mut receipts = Vec::new();
        let mut exhausted = false;
        for batch in &mut self.batches {
            // For both initial and persisted tokens this consumes the binding
            // exactly once. Never also call require_current on the same token.
            Snapshots::bind_cache_frontier(conn, &batch.page, iteration.id).await?;
            exhausted = batch.page.exhausted();
            let Some(child) = batch.child.as_mut() else {
                break;
            };
            Executions::bind_prepared_workflow_task(conn, child).await?;
            let mut metadata = Vec::new();
            record_controls(
                conn,
                std::mem::take(&mut batch.controls),
                key,
                &mut metadata,
            )
            .await?;
            let mut sources = batch.defaults.clone();
            for field in &mut batch.fields {
                for control in &metadata {
                    // Own same-TX receipt/body verification only, no new source
                    // or graph authorization behind workflow/frontier locks.
                    let source =
                        Inputs::prepare_recorded_source(conn, self.context.parent.id, control, key)
                            .await?;
                    Inputs::attach_prepared_destination_control(
                        &mut field.rendered,
                        &mut field.destination,
                        &source,
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
            receipts.extend(metadata);
            let contract = crate::value_schema::CompiledContract::new(
                batch.contract.as_ref().ok_or_else(denied)?,
            )
            .map_err(|_| denied())?;
            let (value, secrets) = crate::secret_values::merge_contract_secret_redactions(
                batch.value.clone(),
                &sources,
                &contract,
            )?;
            let secrets = crate::secret_values::prepare_secret_values(secrets, key)?;
            *child = Executions::initialize_config_with_conn(
                conn,
                child.id,
                Some(batch.admission.ok_or_else(denied)?),
                child.executable_snapshot.as_ref().ok_or_else(denied)?,
                Some(value),
                &secrets,
            )
            .await?;
            receipts.extend(Materialize::seal_execution_inputs(conn, child.id, key).await?);
            children.push(child.clone());
            iteration = Iterations::update_scan_progress(
                &mut *conn,
                iteration.id,
                UpdateWorkflowCacheIterationProgressInput {
                    last_external_id: batch.page.after_external_id().ok_or_else(denied)?.into(),
                    next_batch_index: batch.page.index().checked_add(1).ok_or_else(denied)?,
                    scanned_count: iteration
                        .scanned_count
                        .checked_add(i64::try_from(batch.page.count()).map_err(|_| denied())?)
                        .ok_or_else(denied)?,
                    dispatched_count: iteration
                        .dispatched_count
                        .checked_add(1)
                        .ok_or_else(denied)?,
                },
            )
            .await?
            .ok_or_else(denied)?;
        }
        let completed = if exhausted && self.in_flight == 0 && children.is_empty() {
            let source = self.completion.as_ref().ok_or_else(denied)?;
            source
                .accept_bound_eof(conn, &iteration, &workflow, &stored)
                .await?;
            let terminal = Iterations::mark_terminal(
                &mut *conn,
                iteration.id,
                WorkflowCacheIterationState::Completed,
                None,
            )
            .await?
            .ok_or_else(denied)?;
            source.record_terminal_frontier(conn, terminal).await?;
            let mut completed_tasks = workflow.completed_tasks.clone();
            if !completed_tasks.iter().any(|name| name == task_name) {
                completed_tasks.push(task_name.into());
            }
            let mut current_tasks = workflow.current_tasks.clone();
            current_tasks.retain(|name| name != task_name);
            let all_done = self
                .context
                .parent
                .executable_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.executable.workflow_definition.as_ref())
                .and_then(|graph| graph.definition.get("tasks"))
                .and_then(Value::as_array)
                .is_some_and(|tasks| {
                    tasks.iter().all(|task| {
                        task.get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|name| {
                                completed_tasks.iter().any(|done| done == name)
                                    || workflow.skipped_tasks.iter().any(|skip| skip == name)
                            })
                    })
                })
                && workflow.failed_tasks.is_empty();
            Workflows::update(
                &mut *conn,
                workflow.id,
                UpdateWorkflowExecutionInput {
                    completed_tasks: Some(completed_tasks),
                    current_tasks: Some(current_tasks),
                    status: all_done.then_some(ExecutionStatus::Completed),
                    ..Default::default()
                },
            )
            .await?;
            if all_done {
                super::runtime_read::RuntimeReadRepository::close_execution(conn, parent.id)
                    .await?;
                Some(
                    Executions::update(
                        &mut *conn,
                        parent.id,
                        UpdateExecutionInput {
                            status: Some(ExecutionStatus::Completed),
                            result: Some(json!({"succeeded":true})),
                            ..Default::default()
                        },
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            if !children.is_empty() && !workflow.current_tasks.iter().any(|name| name == task_name)
            {
                let mut current_tasks = workflow.current_tasks.clone();
                current_tasks.push(task_name.into());
                Workflows::update(
                    &mut *conn,
                    workflow.id,
                    UpdateWorkflowExecutionInput {
                        current_tasks: Some(current_tasks),
                        ..Default::default()
                    },
                )
                .await?;
            }
            None
        };
        self.receipts.set(receipts).map_err(|_| denied())?;
        self.ensure_fresh_on(conn).await?;
        children.extend(self.pending.iter().cloned());
        Ok(NativeCacheCheckpointResult {
            children,
            completed,
        })
    }
}

async fn record_controls(
    conn: &mut PgConnection,
    controls: Vec<Control>,
    key: &str,
    receipts: &mut Vec<ReceiptContext>,
) -> Result<()> {
    for control in controls {
        receipts.push(
            measure(
                Op::ControlRecord,
                Inputs::record_prepared(
                    conn,
                    &control.rendered,
                    control.destination.as_ref().ok_or_else(denied)?,
                    control.rendered.value(),
                    control.rendered.selections(),
                    key,
                ),
            )
            .await?,
        );
    }
    Ok(())
}
fn permission_refs(value: Value) -> Result<Vec<String>> {
    let raw = match value {
        Value::Null => vec![],
        Value::String(value) => vec![value],
        Value::Array(values) => values
            .into_iter()
            .map(|value| value.as_str().map(str::to_owned).ok_or_else(denied))
            .collect::<Result<Vec<_>>>()?,
        _ => return Err(denied()),
    };
    let mut seen = BTreeSet::new();
    Ok(raw
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect())
}
fn positive(value: Option<&Value>, default: usize) -> Result<usize> {
    value
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(denied)
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}
fn seconds(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        .filter(|value| *value > 0)
        .ok_or_else(denied)
}
fn preparation_stage(error: Error, stage: &'static str) -> Error {
    match error {
        Error::PermissionDenied(message) => Error::PermissionDenied(format!(
            "Native cache preparation [{stage}] denied: {message}"
        )),
        other => other,
    }
}

fn denied() -> Error {
    Error::PermissionDenied(
        "Native cache checkpoint source or frontier changed or is unavailable".into(),
    )
}
