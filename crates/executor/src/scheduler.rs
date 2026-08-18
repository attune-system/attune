//! Execution Scheduler - Routes executions to available workers
//!
//! This module is responsible for:
//! - Listening for ExecutionRequested messages
//! - Selecting appropriate workers for executions
//! - Queuing executions to worker-specific queues
//! - Updating execution status to Scheduled
//! - Handling worker unavailability and retries
//! - Detecting workflow actions and orchestrating them via child task executions
//! - Resolving `{{ }}` template expressions in workflow task inputs
//! - Processing `publish` directives from transitions
//! - Expanding `with_items` into parallel child executions

use anyhow::Result;
use attune_common::{
    metadata_cache::MetadataCache,
    models::{
        enums::{ExecutionStatus, InquiryStatus, WorkflowCacheIterationState},
        execution::WorkflowTaskMetadata,
        workflow::WorkflowDefinition as WorkflowDefinitionModel,
        Action, CacheEntry, CacheGenerationState, Execution, OwnerType, Runtime,
        WorkflowCacheIteration,
    },
    mq::{
        Consumer, ExecutionCompletedPayload, ExecutionRequestedPayload, MessageEnvelope,
        MessageType, MqError, Publisher,
    },
    rbac::{Action as RbacAction, AuthorizationContext, Grant, Resource},
    repositories::{
        action::ActionRepository,
        cache::{
            CacheEntryRepository, CacheGenerationRepository, CacheNamespaceRepository,
            CacheOwnerScope, MAX_SCAN_MATERIALIZATION_BYTES,
        },
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{IdentityRepository, PermissionSetRepository},
        inquiry::{CreateInquiryInput, InquiryRepository},
        pack::PackRepository,
        runtime::{RuntimeRepository, WorkerRepository},
        trigger::SensorRepository,
        workflow::{
            CreateWorkflowExecutionInput, WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        workflow_cache_iteration::{
            CreateWorkflowCacheIterationInput, UpdateWorkflowCacheIterationProgressInput,
            WorkflowCacheIterationRepository,
        },
        Create, FindById, FindByRef, Update,
    },
    runtime_detection::{normalize_runtime_name, runtime_aliases_contain},
    scheduling::{
        parse_worker_affinity, parse_worker_selector, parse_worker_tolerations,
        preferred_affinity_score, worker_labels_from_capabilities, worker_matches_placement,
        worker_taints_from_capabilities, WorkerAffinity, WorkerToleration,
    },
    secret_values::{
        merge_schema_secret_redactions, prepare_secret_values, redacted_paths,
        restore_secret_values, validate_secret_destination_paths, JsonPointer, RenderedJson,
        SecretPathSource, SecretSource, SecretValueInput, ENTITY_EXECUTION_CONFIG,
        ENTITY_EXECUTION_RESULT,
    },
    trace_tag::normalize_trace_tag,
    version_matching::matches_constraint,
    workflow::{IterateCacheConfig, WorkflowDefinition},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::{Executor, PgConnection, PgPool, Postgres};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::policy_enforcer::{PolicyEnforcer, SchedulingPolicyOutcome};
use crate::workflow::context::{TaskOutcome, WorkflowContext};
use crate::workflow::graph::{BackoffStrategy, TaskGraph};
use crate::workflow::log::WorkflowLogger;

#[derive(Debug, Clone)]
struct EffectiveWorkerPlacement {
    selector: BTreeMap<String, String>,
    tolerations: Vec<WorkerToleration>,
    affinity: WorkerAffinity,
}

struct SchedulingRequestContext<'a> {
    round_robin_counter: &'a AtomicUsize,
    artifacts_dir: &'a str,
    encryption_key: Option<&'a str>,
    envelope: &'a MessageEnvelope<ExecutionRequestedPayload>,
}

/// Extract workflow parameters from an execution's `config` field.
///
/// All executions store config in flat format: `{"n": 5, ...}`.
/// The config object itself IS the parameters map.
fn extract_workflow_params(config: &Option<JsonValue>) -> JsonValue {
    match config {
        Some(c) if c.is_object() => c.clone(),
        _ => serde_json::json!({}),
    }
}

fn iteration_items(items: Vec<JsonValue>, batch_size: Option<usize>) -> Vec<JsonValue> {
    match batch_size {
        Some(size) if size > 1 => items
            .chunks(size)
            .map(|chunk| JsonValue::Array(chunk.to_vec()))
            .collect(),
        _ => items,
    }
}

fn cache_entry_item(entry: CacheEntry) -> JsonValue {
    serde_json::json!({
        "external_id": entry.external_id,
        "value": entry.value,
        "source_updated_at": entry.source_updated_at,
        "source_checksum": entry.source_checksum,
        "size_bytes": entry.size_bytes,
    })
}

fn cache_iteration_authorization_context(
    identity_id: i64,
    identity_attributes: JsonValue,
    owner_type: OwnerType,
    owner_ref: Option<&str>,
    namespace: &str,
) -> AuthorizationContext {
    let mut context = AuthorizationContext::new(identity_id);
    if let JsonValue::Object(attributes) = identity_attributes {
        context.identity_attributes = attributes.into_iter().collect();
    }
    context.owner_type = Some(owner_type);
    context.owner_ref = owner_ref.map(str::to_string);
    context.owner_identity_id = (owner_type == OwnerType::Identity).then_some(identity_id);
    context.target_ref = Some(namespace.to_string());
    context
}

fn standard_cache_read_allowed(
    has_standard: bool,
    task_action_ref: &str,
    workflow_action_ref: &str,
    owner_type: OwnerType,
    owner_ref: Option<&str>,
) -> bool {
    has_standard
        && match (owner_type, owner_ref) {
            (OwnerType::Action, Some(reference)) => {
                reference == task_action_ref || reference == workflow_action_ref
            }
            (OwnerType::Pack, Some(reference)) => [task_action_ref, workflow_action_ref]
                .iter()
                .filter_map(|action_ref| action_ref.split_once('.').map(|(pack, _)| pack))
                .any(|pack| pack == reference),
            _ => false,
        }
}

fn named_cache_permission_refs_are_delegated(requested: &[String], delegated: &[String]) -> bool {
    requested.iter().all(|reference| {
        reference == attune_common::auth::jwt::STANDARD_EXECUTION_ACCESS_REF
            || delegated.contains(reference)
    })
}

fn cache_generation_is_stale(
    state: CacheGenerationState,
    activated: Option<DateTime<Utc>>,
    freshness_target_seconds: i64,
    now: DateTime<Utc>,
) -> bool {
    if state != CacheGenerationState::Active {
        return true;
    }
    if freshness_target_seconds <= 0 {
        return false;
    }
    activated.is_some_and(|activated| (now - activated).num_seconds() > freshness_target_seconds)
}

fn cache_iteration_item_increment(
    item: &JsonValue,
    current_count: usize,
    batched: bool,
) -> Result<usize> {
    let item_bytes = serde_json::to_vec(item)?.len();
    Ok(item_bytes
        + if batched {
            if current_count == 0 {
                2
            } else {
                1
            }
        } else {
            0
        })
}

fn try_materialize_cache_iteration_entry(
    entries: &mut Vec<JsonValue>,
    cursor: &mut Option<String>,
    materialized_bytes: &mut usize,
    entry: CacheEntry,
    batched: bool,
    max_materialization_bytes: usize,
) -> Result<bool> {
    let external_id = entry.external_id.clone();
    let item = cache_entry_item(entry);
    let increment = cache_iteration_item_increment(&item, entries.len(), batched)?;
    if *materialized_bytes + increment > max_materialization_bytes {
        return Ok(false);
    }
    *materialized_bytes += increment;
    *cursor = Some(external_id);
    entries.push(item);
    Ok(true)
}

/// Apply default values from a workflow's `param_schema` to the provided
/// parameters.
///
/// The param_schema uses the flat format where each key maps to an object
/// that may contain a `"default"` field:
///
/// ```json
/// { "n": { "type": "integer", "default": 10 } }
/// ```
///
/// Any parameter that has a default in the schema but is missing (or `null`)
/// in the supplied `params` will be filled in. Parameters already provided
/// by the caller are never overwritten.
fn apply_param_defaults(params: JsonValue, param_schema: &Option<JsonValue>) -> JsonValue {
    let schema = match param_schema {
        Some(s) if s.is_object() => s,
        _ => return params,
    };

    let mut obj = match params {
        JsonValue::Object(m) => m,
        _ => return params,
    };

    if let Some(schema_obj) = schema.as_object() {
        for (key, prop) in schema_obj {
            // Only fill in missing / null parameters
            let needs_default = matches!(obj.get(key), None | Some(JsonValue::Null));
            if needs_default {
                if let Some(default_val) = prop.get("default") {
                    debug!("Applying default for parameter '{}': {}", key, default_val);
                    obj.insert(key.clone(), default_val.clone());
                }
            }
        }
    }

    JsonValue::Object(obj)
}

/// Evaluate a workflow's `output_map` (if any) against the current
/// `WorkflowContext`, producing a JSON object whose keys are the user-defined
/// output names and whose values are the rendered template results.
///
/// Returns `None` if the definition cannot be parsed or has no `output_map`.
/// Individual render errors are logged and the offending key is omitted.
fn build_output_map_result(
    definition_json: &JsonValue,
    wf_ctx: &WorkflowContext,
) -> Option<JsonValue> {
    let definition: WorkflowDefinition = match serde_json::from_value(definition_json.clone()) {
        Ok(d) => d,
        Err(e) => {
            warn!(
                "Failed to parse workflow definition for output_map evaluation: {}",
                e
            );
            return None;
        }
    };

    let output_map = definition.output_map.as_ref()?;
    if output_map.is_empty() {
        return None;
    }

    let mut out = serde_json::Map::new();
    for (key, expr) in output_map {
        match wf_ctx.render_json(&JsonValue::String(expr.clone())) {
            Ok(rendered) => {
                out.insert(key.clone(), rendered);
            }
            Err(e) => {
                warn!(
                    "Failed to render output_map[{}] (expr={:?}): {} — omitting key",
                    key, expr, e
                );
            }
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(JsonValue::Object(out))
    }
}

/// Build the parent execution `result` payload for a completed workflow.
///
/// On success, if the workflow defined an `output_map`, the rendered outputs
/// become the top-level fields of the result, with `succeeded: true` merged in
/// (only if the user's output_map didn't already define a `succeeded` key). If
/// no output_map is defined, the legacy `{"succeeded": true}` shape is used.
///
/// On failure, returns `{"error": ..., "succeeded": false}`.
fn build_workflow_result_payload(
    success: bool,
    error_message: Option<&str>,
    output_override: Option<JsonValue>,
) -> JsonValue {
    if !success {
        return serde_json::json!({
            "error": error_message.unwrap_or("Workflow failed"),
            "succeeded": false,
        });
    }

    match output_override {
        Some(JsonValue::Object(mut map)) => {
            map.entry("succeeded".to_string())
                .or_insert(JsonValue::Bool(true));
            JsonValue::Object(map)
        }
        Some(other) => serde_json::json!({
            "succeeded": true,
            "output": other,
        }),
        None => serde_json::json!({
            "succeeded": true,
        }),
    }
}

fn workflow_result_view(result: &JsonValue) -> JsonValue {
    let Some(object) = result.as_object() else {
        return result.clone();
    };

    let mut merged = object.clone();
    if let Some(JsonValue::Object(data)) = object.get("data") {
        for (key, value) in data {
            merged.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if !merged.contains_key("data") {
        if let Some(stdout) = object.get("stdout").and_then(JsonValue::as_str) {
            if let Ok(JsonValue::Object(parsed_stdout)) =
                serde_json::from_str::<JsonValue>(stdout.trim())
            {
                for (key, value) in parsed_stdout {
                    merged.entry(key).or_insert(value);
                }
            }
        }
    }

    JsonValue::Object(merged)
}

fn workflow_result_secret_paths(result: &JsonValue) -> Vec<JsonPointer> {
    let mut paths = redacted_paths(result);
    let alias_paths = paths
        .iter()
        .filter_map(|path| path.strip_prefix("/data"))
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    paths.extend(alias_paths);
    paths.sort();
    paths.dedup();
    paths
}

fn reconcile_authoritative_task_statuses<I>(
    completed_tasks: &mut Vec<String>,
    failed_tasks: &mut Vec<String>,
    child_tasks: I,
) where
    I: IntoIterator<Item = (String, Option<i32>, ExecutionStatus, Option<String>, i32)>,
{
    #[derive(Default)]
    struct TaskState {
        completed_count: usize,
        failed_count: usize,
        non_terminal_count: usize,
    }

    struct LatestAttempt {
        retry_count: i32,
        status: ExecutionStatus,
    }

    let mut latest_attempts: HashMap<(String, Option<i32>), LatestAttempt> = HashMap::new();
    let mut task_states: HashMap<String, TaskState> = HashMap::new();
    let mut handled_failed_tasks = HashSet::new();

    for (task_name, task_index, status, triggered_by, retry_count) in child_tasks {
        if let Some(triggered_by) = triggered_by {
            handled_failed_tasks.insert(triggered_by);
        }

        let key = (task_name, task_index);
        let should_replace = latest_attempts
            .get(&key)
            .map(|existing| retry_count >= existing.retry_count)
            .unwrap_or(true);

        if should_replace {
            latest_attempts.insert(
                key,
                LatestAttempt {
                    retry_count,
                    status,
                },
            );
        }
    }

    for ((task_name, _task_index), attempt) in latest_attempts {
        let state = task_states.entry(task_name).or_default();
        match attempt.status {
            ExecutionStatus::Completed => {
                state.completed_count += 1;
            }
            ExecutionStatus::Failed | ExecutionStatus::Timeout => {
                state.failed_count += 1;
            }
            ExecutionStatus::Cancelled | ExecutionStatus::Abandoned => {
                state.failed_count += 1;
            }
            _ => {
                state.non_terminal_count += 1;
            }
        }
    }

    let mut authoritative_completed = HashSet::new();
    let mut authoritative_failed = HashSet::new();

    for (task_name, state) in task_states {
        if state.non_terminal_count > 0 {
            continue;
        }

        if state.failed_count > 0 {
            if handled_failed_tasks.contains(&task_name) {
                authoritative_completed.insert(task_name);
            } else {
                authoritative_failed.insert(task_name);
            }
        } else if state.completed_count > 0 {
            authoritative_completed.insert(task_name);
        }
    }

    completed_tasks.retain(|task_name| {
        !authoritative_completed.contains(task_name) && !authoritative_failed.contains(task_name)
    });
    failed_tasks.retain(|task_name| {
        !authoritative_completed.contains(task_name) && !authoritative_failed.contains(task_name)
    });

    for task_name in authoritative_failed {
        if !failed_tasks.contains(&task_name) {
            failed_tasks.push(task_name);
        }
    }

    for task_name in authoritative_completed {
        if !completed_tasks.contains(&task_name) {
            completed_tasks.push(task_name);
        }
    }
}

/// Payload for execution scheduled messages
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExecutionScheduledPayload {
    execution_id: i64,
    worker_id: i64,
    action_ref: String,
    config: Option<JsonValue>,
    scheduled_attempt_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct PendingExecutionRequested {
    execution_id: i64,
    action_id: i64,
    action_ref: String,
    parent_id: i64,
    enforcement_id: Option<i64>,
    config: Option<JsonValue>,
}

#[derive(Debug, Clone)]
struct PendingExecutionCompleted {
    execution_id: i64,
    action_id: i64,
    action_ref: String,
    status: ExecutionStatus,
    result: Option<JsonValue>,
    completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
struct WorkflowAdvanceOutcome {
    execution_requests: Vec<PendingExecutionRequested>,
    completed_children: Vec<PendingExecutionCompleted>,
    completed_execution: Option<PendingExecutionCompleted>,
}

#[derive(Debug, Clone)]
struct RenderedWorkflowTaskInput {
    value: JsonValue,
    secret_inputs: Vec<SecretValueInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheIterationInitializationFailure {
    SelectorRendering,
    OwnerResolution,
    NamespaceResolution,
    PermissionResolution,
    NotAuthorized,
    NoActiveGeneration,
    InvalidGeneration,
    GenerationNotReadable,
    StaleGeneration,
}

impl CacheIterationInitializationFailure {
    fn code(self) -> &'static str {
        match self {
            Self::SelectorRendering => "selector_rendering",
            Self::OwnerResolution => "owner_resolution",
            Self::NamespaceResolution => "namespace_resolution",
            Self::PermissionResolution => "permission_resolution",
            Self::NotAuthorized => "not_authorized",
            Self::NoActiveGeneration => "no_active_generation",
            Self::InvalidGeneration => "invalid_generation",
            Self::GenerationNotReadable => "generation_not_readable",
            Self::StaleGeneration => "stale_generation",
        }
    }
}

#[derive(Debug)]
enum CacheIterationInitializationError {
    Logical(CacheIterationInitializationFailure),
    Infrastructure(anyhow::Error),
}

impl CacheIterationInitializationError {
    fn infrastructure(error: impl Into<anyhow::Error>) -> Self {
        Self::Infrastructure(error.into())
    }
}

type CacheIterationInitializationResult<T> =
    std::result::Result<T, CacheIterationInitializationError>;

struct InitializedCacheIteration {
    existing: Option<WorkflowCacheIteration>,
    namespace_id: i64,
    generation_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheGenerationSelector {
    Active,
    Explicit(i64),
}

fn parse_cache_generation_selector(
    selector: &str,
) -> std::result::Result<CacheGenerationSelector, CacheIterationInitializationFailure> {
    if selector.eq_ignore_ascii_case("active") {
        Ok(CacheGenerationSelector::Active)
    } else {
        selector
            .parse::<i64>()
            .map(CacheGenerationSelector::Explicit)
            .map_err(|_| CacheIterationInitializationFailure::InvalidGeneration)
    }
}

fn cache_iteration_terminal_result(state: WorkflowCacheIterationState) -> JsonValue {
    serde_json::json!({
        "cache_iteration": {"state": format!("{:?}", state).to_lowercase()}
    })
}

fn cache_iteration_outcome_without_state(status: ExecutionStatus) -> Result<TaskOutcome> {
    if status == ExecutionStatus::Failed {
        Ok(TaskOutcome::Failed)
    } else {
        Err(anyhow::anyhow!(
            "Cache iteration state is missing for a non-failed execution"
        ))
    }
}

/// Execution scheduler that routes executions to workers
pub struct ExecutionScheduler {
    pool: PgPool,
    publisher: Arc<Publisher>,
    consumer: Arc<Consumer>,
    policy_enforcer: Arc<PolicyEnforcer>,
    /// Round-robin counter for distributing executions across workers
    round_robin_counter: AtomicUsize,
    /// Root directory for file-backed artifacts (workflow logs, etc.)
    artifacts_dir: Arc<String>,
    encryption_key: Option<String>,
    metadata_caches: Arc<SchedulerMetadataCaches>,
}

/// Default heartbeat interval in seconds (should match worker config default)
const DEFAULT_HEARTBEAT_INTERVAL: u64 = 30;

/// Maximum age multiplier for heartbeat staleness check
/// Workers are considered stale if heartbeat is older than HEARTBEAT_INTERVAL * HEARTBEAT_STALENESS_MULTIPLIER
const HEARTBEAT_STALENESS_MULTIPLIER: u64 = 3;
const SCHEDULING_RECLAIM_GRACE_SECONDS: i64 = 30;
const RUNTIME_VERSIONS_CAPABILITY_KEY: &str = "runtime_versions";
const ACTION_METADATA_CACHE_TTL: Duration = Duration::from_secs(30);
const WORKFLOW_METADATA_CACHE_TTL: Duration = Duration::from_secs(30);
const WORKFLOW_METADATA_CACHE_MAX_ENTRIES: usize = 1024;

#[derive(Debug, Clone)]
struct CachedActionEntry {
    action: Action,
    expires_at: Instant,
}

type ActionIdCache = tokio::sync::RwLock<HashMap<i64, CachedActionEntry>>;
type ActionRefCache = tokio::sync::RwLock<HashMap<String, CachedActionEntry>>;

pub(crate) struct SchedulerMetadataCaches {
    action_by_id: ActionIdCache,
    action_by_ref: ActionRefCache,
    workflow_definition_by_id: MetadataCache<String, WorkflowDefinitionModel>,
}

impl SchedulerMetadataCaches {
    pub(crate) fn new() -> Self {
        Self {
            action_by_id: tokio::sync::RwLock::new(HashMap::new()),
            action_by_ref: tokio::sync::RwLock::new(HashMap::new()),
            workflow_definition_by_id: MetadataCache::new(
                WORKFLOW_METADATA_CACHE_TTL,
                WORKFLOW_METADATA_CACHE_MAX_ENTRIES,
            ),
        }
    }

    pub(crate) async fn invalidate_action(&self, action_id: Option<i64>, action_ref: Option<&str>) {
        if let Some(id) = action_id {
            self.action_by_id.write().await.remove(&id);
        }
        if let Some(r#ref) = action_ref {
            self.action_by_ref.write().await.remove(r#ref);
        }
    }

    async fn cache_action(&self, action: &Action) {
        let entry = CachedActionEntry {
            action: action.clone(),
            expires_at: Instant::now() + ACTION_METADATA_CACHE_TTL,
        };
        self.action_by_id
            .write()
            .await
            .insert(action.id, entry.clone());
        self.action_by_ref
            .write()
            .await
            .insert(action.r#ref.clone(), entry);
    }

    async fn cached_action_by_id(&self, id: i64) -> Option<Action> {
        let mut guard = self.action_by_id.write().await;
        let expired = guard
            .get(&id)
            .is_some_and(|entry| Instant::now() >= entry.expires_at);
        if expired {
            guard.remove(&id);
            return None;
        }
        guard.get(&id).map(|entry| entry.action.clone())
    }

    async fn cached_action_by_ref(&self, action_ref: &str) -> Option<Action> {
        let mut guard = self.action_by_ref.write().await;
        let expired = guard
            .get(action_ref)
            .is_some_and(|entry| Instant::now() >= entry.expires_at);
        if expired {
            guard.remove(action_ref);
            return None;
        }
        guard.get(action_ref).map(|entry| entry.action.clone())
    }

    async fn cache_workflow_definition(&self, workflow_def: &WorkflowDefinitionModel) {
        self.workflow_definition_by_id
            .insert(workflow_def.id.to_string(), workflow_def.clone())
            .await;
    }

    async fn cached_workflow_definition_by_id(
        &self,
        workflow_def_id: i64,
    ) -> Option<WorkflowDefinitionModel> {
        self.workflow_definition_by_id
            .get(&workflow_def_id.to_string())
            .await
    }
}

impl ExecutionScheduler {
    fn workflow_delay_context(execution: &Execution) -> Option<String> {
        execution.workflow_task.as_ref().map(|workflow_task| {
            let triggered_by = workflow_task
                .triggered_by
                .as_deref()
                .map(|task| format!(", triggered by '{}'", task))
                .unwrap_or_default();

            format!(
                "workflow task '{}' (execution {}, workflow_execution {}, action '{}'{})",
                workflow_task.task_name,
                execution.id,
                workflow_task.workflow_execution,
                execution.action_ref,
                triggered_by
            )
        })
    }

    fn retryable_mq_error(error: &anyhow::Error) -> Option<MqError> {
        let mq_error = error.downcast_ref::<MqError>()?;
        Some(match mq_error {
            MqError::Connection(msg) => MqError::Connection(msg.clone()),
            MqError::Channel(msg) => MqError::Channel(msg.clone()),
            MqError::Publish(msg) => MqError::Publish(msg.clone()),
            MqError::Timeout(msg) => MqError::Timeout(msg.clone()),
            MqError::Pool(msg) => MqError::Pool(msg.clone()),
            MqError::Lapin(err) => MqError::Connection(err.to_string()),
            _ => return None,
        })
    }

    /// Create a new execution scheduler
    pub(crate) fn new(
        pool: PgPool,
        publisher: Arc<Publisher>,
        consumer: Arc<Consumer>,
        policy_enforcer: Arc<PolicyEnforcer>,
        artifacts_dir: impl Into<String>,
        encryption_key: Option<String>,
        metadata_caches: Arc<SchedulerMetadataCaches>,
    ) -> Self {
        Self {
            pool,
            publisher,
            consumer,
            policy_enforcer,
            round_robin_counter: AtomicUsize::new(0),
            artifacts_dir: Arc::new(artifacts_dir.into()),
            encryption_key,
            metadata_caches,
        }
    }

    /// Start processing execution requested messages
    pub async fn start(&self) -> Result<()> {
        info!("Starting execution scheduler");

        let pool = self.pool.clone();
        let publisher = self.publisher.clone();
        let policy_enforcer = self.policy_enforcer.clone();
        let artifacts_dir = self.artifacts_dir.clone();
        let encryption_key = self.encryption_key.clone();
        let metadata_caches = self.metadata_caches.clone();
        // Share the counter with the handler closure via Arc.
        // We wrap &self's AtomicUsize in a new Arc<AtomicUsize> by copying the
        // current value so the closure is 'static.
        let counter = Arc::new(AtomicUsize::new(
            self.round_robin_counter.load(Ordering::Relaxed),
        ));

        // Use the handler pattern to consume messages
        self.consumer
            .consume_with_handler(
                move |envelope: MessageEnvelope<ExecutionRequestedPayload>| {
                    let pool = pool.clone();
                    let publisher = publisher.clone();
                    let policy_enforcer = policy_enforcer.clone();
                    let counter = counter.clone();
                    let artifacts_dir = artifacts_dir.clone();
                    let encryption_key = encryption_key.clone();
                    let metadata_caches = metadata_caches.clone();

                    async move {
                        if let Err(e) = Self::process_execution_requested(
                            &pool,
                            &publisher,
                            &policy_enforcer,
                            &counter,
                            artifacts_dir.as_str(),
                            encryption_key.as_deref(),
                            &metadata_caches,
                            &envelope,
                        )
                        .await
                        {
                            error!("Error scheduling execution: {}", e);
                            // Return error to trigger nack with requeue
                            if let Some(mq_err) = Self::retryable_mq_error(&e) {
                                return Err(mq_err);
                            }
                            return Err(format!("Failed to schedule execution: {}", e).into());
                        }
                        Ok(())
                    }
                },
            )
            .await?;

        Ok(())
    }

    /// Process an execution requested message
    #[allow(clippy::too_many_arguments)] // Explicit service dependencies keep handler ownership clear.
    async fn process_execution_requested(
        pool: &PgPool,
        publisher: &Publisher,
        policy_enforcer: &PolicyEnforcer,
        round_robin_counter: &AtomicUsize,
        artifacts_dir: &str,
        encryption_key: Option<&str>,
        metadata_caches: &SchedulerMetadataCaches,
        envelope: &MessageEnvelope<ExecutionRequestedPayload>,
    ) -> Result<()> {
        debug!("Processing execution requested message: {:?}", envelope);

        let execution_id = envelope.payload.execution_id;

        info!("Scheduling execution: {}", execution_id);

        // Fetch execution from database
        let execution = match ExecutionRepository::find_by_id(pool, execution_id).await? {
            Some(execution) => execution,
            None => {
                warn!("Execution {} not found during scheduling", execution_id);
                Self::remove_queued_policy_execution(
                    policy_enforcer,
                    pool,
                    publisher,
                    execution_id,
                )
                .await;
                return Ok(());
            }
        };

        if execution.status == ExecutionStatus::Scheduling {
            if let Some(execution) = ExecutionRepository::reclaim_stale_scheduling(
                pool,
                execution_id,
                None,
                Utc::now() - chrono::Duration::seconds(SCHEDULING_RECLAIM_GRACE_SECONDS),
            )
            .await?
            {
                warn!(
                    "Reclaimed stale scheduling claim for execution {} after {}s",
                    execution_id, SCHEDULING_RECLAIM_GRACE_SECONDS
                );
                let request_context = SchedulingRequestContext {
                    round_robin_counter,
                    artifacts_dir,
                    encryption_key,
                    envelope,
                };
                return Self::process_claimed_execution(
                    pool,
                    publisher,
                    policy_enforcer,
                    &request_context,
                    execution,
                    metadata_caches,
                )
                .await;
            }

            return Err(MqError::Timeout(format!(
                "Execution {} is already being scheduled; retry later",
                execution_id
            ))
            .into());
        }

        if execution.status != ExecutionStatus::Requested {
            debug!(
                "Skipping execution {} with status {:?}; only Requested executions are schedulable",
                execution_id, execution.status
            );
            Self::remove_queued_policy_execution(policy_enforcer, pool, publisher, execution_id)
                .await;
            return Ok(());
        }

        let request_context = SchedulingRequestContext {
            round_robin_counter,
            artifacts_dir,
            encryption_key,
            envelope,
        };
        let execution =
            match ExecutionRepository::claim_for_scheduling(pool, execution_id, None).await? {
                Some(execution) => execution,
                None => {
                    return Self::handle_failed_scheduling_claim(
                        pool,
                        publisher,
                        policy_enforcer,
                        &request_context,
                        execution_id,
                        metadata_caches,
                    )
                    .await;
                }
            };

        Self::process_claimed_execution(
            pool,
            publisher,
            policy_enforcer,
            &request_context,
            execution,
            metadata_caches,
        )
        .await
    }

    async fn process_claimed_execution(
        pool: &PgPool,
        publisher: &Publisher,
        policy_enforcer: &PolicyEnforcer,
        request_context: &SchedulingRequestContext<'_>,
        execution: Execution,
        metadata_caches: &SchedulerMetadataCaches,
    ) -> Result<()> {
        let execution_id = execution.id;

        // Fetch action to determine runtime requirements
        let action = Self::get_action_for_execution(pool, metadata_caches, &execution).await?;
        if !action.enabled {
            Self::fail_unschedulable_execution(
                pool,
                publisher,
                request_context.envelope,
                execution_id,
                action.id,
                &action.r#ref,
                &format!("Action '{}' is disabled", action.r#ref),
            )
            .await?;
            return Ok(());
        }

        // Check if this action is a workflow (has workflow_def set)
        if action.workflow_def.is_some() {
            info!(
                "Action '{}' is a workflow, orchestrating instead of dispatching to worker",
                action.r#ref
            );
            let result = Self::process_workflow_execution(
                pool,
                publisher,
                request_context.round_robin_counter,
                request_context.artifacts_dir,
                request_context.encryption_key,
                &execution,
                &action,
                metadata_caches,
            )
            .await;
            if result.is_err() {
                Self::revert_scheduling_claim(pool, execution_id).await?;
            }
            return result;
        }

        // Apply parameter defaults from the action's param_schema.
        // This mirrors what `process_workflow_execution` does for workflows
        // so that non-workflow executions also get missing parameters filled
        // in from the action's declared defaults.
        let execution_config = {
            let raw_config = execution.config.clone();
            let params = extract_workflow_params(&raw_config);
            let params_with_defaults = apply_param_defaults(params, &action.param_schema);
            // Config is already flat — just use the defaults-applied version
            if params_with_defaults.is_object()
                && !params_with_defaults.as_object().unwrap().is_empty()
            {
                Some(params_with_defaults)
            } else {
                raw_config
            }
        };

        match policy_enforcer
            .enforce_for_scheduling(
                action.id,
                Some(action.pack),
                execution_id,
                execution_config.as_ref(),
            )
            .await
        {
            Ok(SchedulingPolicyOutcome::Queued) => {
                if ExecutionRepository::update_if_status(
                    pool,
                    execution_id,
                    ExecutionStatus::Scheduling,
                    UpdateExecutionInput {
                        status: Some(ExecutionStatus::Requested),
                        ..Default::default()
                    },
                )
                .await?
                .is_none()
                {
                    warn!(
                        "Execution {} could not be returned to Requested after queueing",
                        execution_id
                    );
                }
                if let Some(context) = Self::workflow_delay_context(&execution) {
                    warn!(
                        "Delayed {}: worker selection deferred because the execution was queued by scheduling policy",
                        context
                    );
                }
                info!(
                    "Execution {} queued by policy for action {}; deferring worker selection",
                    execution_id, action.id
                );
                return Ok(());
            }
            Ok(SchedulingPolicyOutcome::Ready) => {}
            Err(err) => {
                if Self::is_policy_cancellation_error(&err) {
                    Self::remove_queued_policy_execution(
                        policy_enforcer,
                        pool,
                        publisher,
                        execution_id,
                    )
                    .await;
                    Self::cancel_execution_for_policy_violation(
                        pool,
                        publisher,
                        request_context.envelope,
                        execution_id,
                        action.id,
                        &action.r#ref,
                        &err.to_string(),
                    )
                    .await?;
                    return Ok(());
                }

                if ExecutionRepository::update_if_status(
                    pool,
                    execution_id,
                    ExecutionStatus::Scheduling,
                    UpdateExecutionInput {
                        status: Some(ExecutionStatus::Requested),
                        ..Default::default()
                    },
                )
                .await?
                .is_none()
                {
                    warn!(
                        "Execution {} lost its scheduling claim before policy retry cleanup",
                        execution_id
                    );
                }
                return Err(err);
            }
        }

        // Regular action: select appropriate worker only after policy
        // readiness is confirmed, so queued executions don't reserve stale
        // workers while they wait.
        let worker = match Self::select_worker_for_action_execution(
            pool,
            &action,
            Some(&execution),
            request_context.round_robin_counter,
        )
        .await
        {
            Ok(worker) => worker,
            Err(err) if Self::is_unschedulable_error(&err) => {
                Self::release_acquired_policy_slot(policy_enforcer, pool, publisher, execution_id)
                    .await?;
                Self::fail_unschedulable_execution(
                    pool,
                    publisher,
                    request_context.envelope,
                    execution_id,
                    action.id,
                    &action.r#ref,
                    &err.to_string(),
                )
                .await?;
                return Ok(());
            }
            Err(err) => {
                Self::release_acquired_policy_slot(policy_enforcer, pool, publisher, execution_id)
                    .await?;
                if ExecutionRepository::update_if_status(
                    pool,
                    execution_id,
                    ExecutionStatus::Scheduling,
                    UpdateExecutionInput {
                        status: Some(ExecutionStatus::Requested),
                        ..Default::default()
                    },
                )
                .await?
                .is_none()
                {
                    warn!(
                        "Execution {} lost its scheduling claim before worker-selection retry cleanup",
                        execution_id
                    );
                }
                if let Some(context) = Self::workflow_delay_context(&execution) {
                    warn!(
                        "Delayed {}: transient worker-selection failure left the task waiting to be retried: {}",
                        context, err
                    );
                }
                return Err(err);
            }
        };

        info!(
            "Selected worker {} for execution {}",
            worker.id, execution_id
        );

        // Persist the selected worker so later cancellation requests can be
        // routed to the correct per-worker cancel queue.
        let scheduled_execution = match ExecutionRepository::update_if_status(
            pool,
            execution_id,
            ExecutionStatus::Scheduling,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Scheduled),
                worker: Some(worker.id),
                ..Default::default()
            },
        )
        .await?
        {
            Some(execution) => execution,
            None => {
                warn!(
                    "Execution {} left Scheduling before worker {} could be assigned",
                    execution_id, worker.id
                );
                Self::release_acquired_policy_slot(policy_enforcer, pool, publisher, execution_id)
                    .await?;
                return Ok(());
            }
        };

        // Publish message to worker-specific queue
        if let Err(err) = Self::queue_to_worker(
            publisher,
            &execution_id,
            &worker.id,
            &request_context.envelope.payload.action_ref,
            &execution_config,
            scheduled_execution.updated,
            &action,
        )
        .await
        {
            if let Err(revert_err) =
                Self::revert_scheduled_execution(pool, execution_id, policy_enforcer, publisher)
                    .await
            {
                warn!(
                    "Failed to revert execution {} back to Requested after worker publish error: {}",
                    execution_id, revert_err
                );
            }
            if let Some(context) = Self::workflow_delay_context(&execution) {
                warn!(
                    "Delayed {}: failed to publish the execution to a worker queue, the task will remain pending until retried: {}",
                    context, err
                );
            }
            return Err(err);
        }

        info!(
            "Execution {} scheduled to worker {}",
            execution_id,
            scheduled_execution.worker.unwrap_or(worker.id)
        );

        Ok(())
    }

    async fn handle_failed_scheduling_claim(
        pool: &PgPool,
        publisher: &Publisher,
        policy_enforcer: &PolicyEnforcer,
        request_context: &SchedulingRequestContext<'_>,
        execution_id: i64,
        metadata_caches: &SchedulerMetadataCaches,
    ) -> Result<()> {
        let execution = match ExecutionRepository::find_by_id(pool, execution_id).await? {
            Some(execution) => execution,
            None => {
                Self::remove_queued_policy_execution(
                    policy_enforcer,
                    pool,
                    publisher,
                    execution_id,
                )
                .await;
                return Ok(());
            }
        };

        match execution.status {
            ExecutionStatus::Requested => {
                if let Some(context) = Self::workflow_delay_context(&execution) {
                    warn!(
                        "Delayed {}: the scheduler could not immediately acquire the execution claim; retrying later",
                        context
                    );
                }
                Err(MqError::Timeout(format!(
                    "Execution {} changed while claiming; retry later",
                    execution_id
                ))
                .into())
            }
            ExecutionStatus::Scheduling => {
                if let Some(execution) = ExecutionRepository::reclaim_stale_scheduling(
                    pool,
                    execution_id,
                    None,
                    Utc::now() - chrono::Duration::seconds(SCHEDULING_RECLAIM_GRACE_SECONDS),
                )
                .await?
                {
                    warn!(
                        "Recovered stale scheduling claim for execution {} after failed initial claim",
                        execution_id
                    );
                    return Self::process_claimed_execution(
                        pool,
                        publisher,
                        policy_enforcer,
                        request_context,
                        execution,
                        metadata_caches,
                    )
                    .await;
                }

                if let Some(context) = Self::workflow_delay_context(&execution) {
                    warn!(
                        "Delayed {}: the execution is still being scheduled elsewhere, so this attempt will retry later",
                        context
                    );
                }
                Err(MqError::Timeout(format!(
                    "Execution {} is still being scheduled; retry later",
                    execution_id
                ))
                .into())
            }
            _ => {
                Self::cleanup_unclaimable_execution(policy_enforcer, pool, publisher, execution_id)
                    .await?;
                Ok(())
            }
        }
    }

    // -----------------------------------------------------------------------
    // Workflow orchestration
    // -----------------------------------------------------------------------

    /// Handle a workflow execution by loading its definition, creating a
    /// `workflow_execution` record, and dispatching the entry-point tasks as
    /// child executions that workers *can* handle.
    #[allow(clippy::too_many_arguments)] // Explicit workflow dependencies keep orchestration inputs clear.
    async fn process_workflow_execution(
        pool: &PgPool,
        publisher: &Publisher,
        round_robin_counter: &AtomicUsize,
        artifacts_dir: &str,
        encryption_key: Option<&str>,
        execution: &Execution,
        action: &Action,
        metadata_caches: &SchedulerMetadataCaches,
    ) -> Result<()> {
        let logger = WorkflowLogger::new(
            pool.clone(),
            artifacts_dir,
            action.r#ref.as_str(),
            execution.id,
        );

        let workflow_def_id = action
            .workflow_def
            .ok_or_else(|| anyhow::anyhow!("Action '{}' has no workflow_def", action.r#ref))?;

        // Load workflow definition
        let workflow_def = if let Some(cached) = metadata_caches
            .cached_workflow_definition_by_id(workflow_def_id)
            .await
        {
            cached
        } else {
            let workflow_def = WorkflowDefinitionRepository::find_by_id(pool, workflow_def_id)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Workflow definition {} not found for action '{}'",
                        workflow_def_id,
                        action.r#ref
                    )
                })?;
            metadata_caches
                .cache_workflow_definition(&workflow_def)
                .await;
            workflow_def
        };

        // Parse workflow definition JSON into the strongly-typed struct
        let definition: WorkflowDefinition =
            serde_json::from_value(workflow_def.definition.clone()).map_err(|e| {
                anyhow::anyhow!(
                    "Invalid workflow definition for '{}': {}",
                    workflow_def.r#ref,
                    e
                )
            })?;

        // Build the task graph to determine entry points and transitions
        let graph = TaskGraph::from_workflow(&definition).map_err(|e| {
            anyhow::anyhow!(
                "Failed to build task graph for workflow '{}': {}",
                workflow_def.r#ref,
                e
            )
        })?;

        let task_graph_json: JsonValue = serde_json::to_value(&graph).unwrap_or_default();

        // Gather initial variables from the definition
        let initial_vars: JsonValue =
            serde_json::to_value(&definition.vars).unwrap_or_else(|_| serde_json::json!({}));

        let workflow_execution_result = WorkflowExecutionRepository::create_or_get_by_execution(
            pool,
            CreateWorkflowExecutionInput {
                execution: execution.id,
                workflow_def: workflow_def.id,
                task_graph: task_graph_json,
                variables: initial_vars,
                status: ExecutionStatus::Running,
            },
        )
        .await?;
        let workflow_execution = workflow_execution_result.workflow_execution;

        if workflow_execution_result.created {
            info!(
                "Created workflow_execution {} for workflow '{}' (parent execution {})",
                workflow_execution.id, workflow_def.r#ref, execution.id
            );
            logger
                .info(format!(
                    "Workflow '{}' started (workflow_execution {})",
                    workflow_def.r#ref, workflow_execution.id
                ))
                .await;
        } else {
            info!(
                "Reusing existing workflow_execution {} for workflow '{}' (parent execution {})",
                workflow_execution.id, workflow_def.r#ref, execution.id
            );
            logger
                .info(format!(
                    "Workflow '{}' resumed (workflow_execution {})",
                    workflow_def.r#ref, workflow_execution.id
                ))
                .await;
        }

        if graph.entry_points.is_empty() {
            warn!(
                "Workflow '{}' has no entry-point tasks, completing immediately",
                workflow_def.r#ref
            );
            logger
                .warn("Workflow has no entry-point tasks; completing immediately")
                .await;
            Self::complete_workflow(pool, execution.id, workflow_execution.id, true, None, None)
                .await?;
            logger.info("Workflow completed").await;
            return Ok(());
        }

        if ExecutionRepository::update_if_status(
            pool,
            execution.id,
            ExecutionStatus::Scheduling,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Running),
                ..Default::default()
            },
        )
        .await?
        .is_none()
        {
            let current = ExecutionRepository::find_by_id(pool, execution.id).await?;
            if !matches!(
                current.as_ref().map(|execution| execution.status),
                Some(
                    ExecutionStatus::Running | ExecutionStatus::Completed | ExecutionStatus::Failed
                )
            ) {
                return Err(anyhow::anyhow!(
                    "Workflow parent execution {} left Scheduling before entry dispatch",
                    execution.id
                ));
            }
        }

        // Build initial workflow context from execution parameters and
        // workflow-level vars so that entry-point task inputs are rendered.
        // Apply defaults from the workflow's param_schema for any parameters
        // that were not supplied by the caller.
        let restored_execution_config = Self::restore_secret_entity(
            pool,
            encryption_key,
            ENTITY_EXECUTION_CONFIG,
            execution.id,
            execution.config.clone().unwrap_or(JsonValue::Null),
        )
        .await?;
        let workflow_params = extract_workflow_params(&Some(restored_execution_config));
        let workflow_params = apply_param_defaults(workflow_params, &workflow_def.param_schema);
        let wf_ctx = WorkflowContext::new(
            workflow_params,
            definition
                .vars
                .iter()
                .map(|(k, v)| {
                    let jv: JsonValue =
                        serde_json::to_value(v).unwrap_or(JsonValue::String(v.to_string()));
                    (k.clone(), jv)
                })
                .collect(),
        );
        Self::mark_workflow_parameter_secret_sources(&wf_ctx, execution);

        // For each entry-point task, create a child execution and dispatch it
        for entry_task_name in &graph.entry_points {
            if let Some(task_node) = graph.get_task(entry_task_name) {
                logger
                    .info(format!(
                        "Dispatching entry task '{}' (action '{}')",
                        task_node.name,
                        task_node.action.as_deref().unwrap_or("(none)")
                    ))
                    .await;
                Self::dispatch_or_resume_entry_workflow_task(
                    pool,
                    publisher,
                    round_robin_counter,
                    execution,
                    &workflow_execution.id,
                    task_node,
                    &wf_ctx,
                    encryption_key,
                    None, // entry-point task — no predecessor
                )
                .await?;
            } else {
                warn!(
                    "Entry-point task '{}' not found in graph for workflow '{}'",
                    entry_task_name, workflow_def.r#ref
                );
                logger
                    .warn(format!(
                        "Entry-point task '{}' not found in workflow graph",
                        entry_task_name
                    ))
                    .await;
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_or_resume_entry_workflow_task(
        pool: &PgPool,
        publisher: &Publisher,
        round_robin_counter: &AtomicUsize,
        parent_execution: &Execution,
        workflow_execution_id: &i64,
        task_node: &crate::workflow::graph::TaskNode,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
    ) -> Result<()> {
        let existing_children: Vec<(i64, Option<i64>, ExecutionStatus)> = sqlx::query_as(
            "SELECT id, action, status \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND workflow_task->>'task_name' = $2 \
             ORDER BY created ASC",
        )
        .bind(workflow_execution_id.to_string())
        .bind(task_node.name.as_str())
        .fetch_all(pool)
        .await?;

        if existing_children.is_empty() {
            return Self::dispatch_workflow_task(
                pool,
                publisher,
                round_robin_counter,
                parent_execution,
                workflow_execution_id,
                task_node,
                wf_ctx,
                encryption_key,
                triggered_by,
            )
            .await;
        }

        if task_node.with_items.is_some() || task_node.iterate_cache.is_some() {
            return Self::dispatch_workflow_task(
                pool,
                publisher,
                round_robin_counter,
                parent_execution,
                workflow_execution_id,
                task_node,
                wf_ctx,
                encryption_key,
                triggered_by,
            )
            .await;
        }

        for (child_id, action_id, status) in existing_children {
            if status == ExecutionStatus::Requested {
                let action_id = action_id.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Workflow child execution {} has no action id while resuming task '{}'",
                        child_id,
                        task_node.name
                    )
                })?;
                let child = ExecutionRepository::find_by_id(pool, child_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Execution {} not found", child_id))?;

                Self::publish_execution_requested(
                    pool,
                    publisher,
                    child_id,
                    action_id,
                    &child.action_ref,
                    parent_execution,
                )
                .await?;
            }
        }

        Ok(())
    }

    fn mark_workflow_parameter_secret_sources(wf_ctx: &WorkflowContext, execution: &Execution) {
        let paths = redacted_paths(&execution.config.clone().unwrap_or(JsonValue::Null));
        wf_ctx.mark_secret_pointer_paths("parameters", &paths, |path| {
            SecretSource::WorkflowParameter {
                execution_id: execution.id,
                path: path.clone(),
            }
        });
    }

    fn mark_workflow_task_result_secret_sources(
        wf_ctx: &WorkflowContext,
        child_executions: &[Execution],
        workflow_execution_id: i64,
    ) {
        for child in child_executions {
            let Some(workflow_task) = &child.workflow_task else {
                continue;
            };
            if workflow_task.workflow_execution != workflow_execution_id {
                continue;
            }
            let paths = redacted_paths(&child.result.clone().unwrap_or(JsonValue::Null));
            wf_ctx.mark_secret_pointer_paths(
                &format!("task.{}", workflow_task.task_name),
                &paths,
                |path| SecretSource::ExecutionResult {
                    execution_id: child.id,
                    path: path.clone(),
                },
            );
            wf_ctx.mark_secret_pointer_paths(
                &format!("tasks.{}", workflow_task.task_name),
                &paths,
                |path| SecretSource::ExecutionResult {
                    execution_id: child.id,
                    path: path.clone(),
                },
            );
        }
    }

    fn render_workflow_task_input(
        parent_execution: &Execution,
        parent_execution_config: &JsonValue,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        wf_ctx: &WorkflowContext,
    ) -> Result<RenderedWorkflowTaskInput> {
        let rendered =
            if task_node.input.is_object() && !task_node.input.as_object().unwrap().is_empty() {
                wf_ctx
                    .render_json_with_sensitivity(&task_node.input)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "Template rendering failed for task '{}': {}",
                            task_node.name,
                            error
                        )
                    })?
            } else {
                RenderedJson::plain(task_node.input.clone())
            };

        let mut secret_path_sources = rendered.secret_path_sources;
        let rendered_value = if rendered.value.is_object()
            && !rendered.value.as_object().unwrap().is_empty()
        {
            rendered.value
        } else {
            for path in redacted_paths(&parent_execution.config.clone().unwrap_or(JsonValue::Null))
            {
                secret_path_sources.push(SecretPathSource {
                    path: path.clone(),
                    source: SecretSource::WorkflowParameter {
                        execution_id: parent_execution.id,
                        path,
                    },
                });
            }
            parent_execution_config.clone()
        };

        let secret_paths = secret_path_sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>();
        validate_secret_destination_paths(task_action.param_schema.as_ref(), &secret_paths)?;
        let (redacted_input, secret_inputs) = merge_schema_secret_redactions(
            rendered_value,
            &secret_path_sources,
            task_action.param_schema.as_ref(),
        );

        Ok(RenderedWorkflowTaskInput {
            value: redacted_input,
            secret_inputs,
        })
    }

    async fn persist_execution_config_secrets(
        pool: &PgPool,
        encryption_key: Option<&str>,
        child_execution_id: i64,
        secret_inputs: Vec<SecretValueInput>,
    ) -> Result<()> {
        if secret_inputs.is_empty() {
            return Ok(());
        }

        let encryption_key = encryption_key.ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot store secret workflow execution parameters without security.encryption_key"
            )
        })?;
        let prepared = prepare_secret_values(secret_inputs, encryption_key)?;
        ExecutionSecretValueRepository::upsert_many(
            pool,
            ENTITY_EXECUTION_CONFIG,
            child_execution_id,
            &prepared,
        )
        .await?;
        Ok(())
    }

    async fn persist_execution_config_secrets_with_conn(
        conn: &mut PgConnection,
        encryption_key: Option<&str>,
        child_execution_id: i64,
        secret_inputs: Vec<SecretValueInput>,
    ) -> Result<()> {
        if secret_inputs.is_empty() {
            return Ok(());
        }

        let encryption_key = encryption_key.ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot store secret workflow execution parameters without security.encryption_key"
            )
        })?;
        let prepared = prepare_secret_values(secret_inputs, encryption_key)?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            conn,
            ENTITY_EXECUTION_CONFIG,
            child_execution_id,
            &prepared,
        )
        .await?;
        Ok(())
    }

    async fn restore_secret_entity<'e, E>(
        executor: E,
        encryption_key: Option<&str>,
        entity_type: &str,
        entity_id: i64,
        redacted_value: JsonValue,
    ) -> Result<JsonValue>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        let secrets =
            ExecutionSecretValueRepository::find_stored_by_entity(executor, entity_type, entity_id)
                .await?;
        if secrets.is_empty() {
            return Ok(redacted_value);
        }

        let encryption_key = encryption_key.ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot restore secret {} {} without security.encryption_key",
                entity_type,
                entity_id
            )
        })?;
        Ok(restore_secret_values(
            redacted_value,
            &secrets,
            encryption_key,
        )?)
    }

    fn workflow_task_permission_set_refs(
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        wf_ctx: &WorkflowContext,
    ) -> Result<Vec<String>> {
        let Some(template) = &task_node.permission_set_refs else {
            return Ok(task_action.default_execution_permission_set_refs.clone());
        };

        let rendered = wf_ctx.render_json(template).map_err(|e| {
            anyhow::anyhow!(
                "Failed to render permission_set_refs for workflow task '{}': {}",
                task_node.name,
                e
            )
        })?;

        Self::normalize_workflow_permission_set_refs(&task_node.name, rendered)
    }

    fn normalize_workflow_permission_set_refs(
        task_name: &str,
        value: JsonValue,
    ) -> Result<Vec<String>> {
        let raw_refs: Vec<String> = match value {
            JsonValue::Null => Vec::new(),
            JsonValue::String(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    Vec::new()
                } else {
                    vec![trimmed.to_string()]
                }
            }
            JsonValue::Array(values) => values
                .into_iter()
                .map(|value| match value {
                    JsonValue::String(value) => Ok(value.trim().to_string()),
                    other => Err(anyhow::anyhow!(
                        "permission_set_refs for workflow task '{}' must render to a string or array of strings; found array item {}",
                        task_name,
                        other
                    )),
                })
                .collect::<Result<Vec<_>>>()?,
            other => {
                return Err(anyhow::anyhow!(
                    "permission_set_refs for workflow task '{}' must render to a string or array of strings; found {}",
                    task_name,
                    other
                ));
            }
        };

        let mut seen = HashSet::new();
        Ok(raw_refs
            .into_iter()
            .filter(|value| !value.is_empty())
            .filter(|value| seen.insert(value.clone()))
            .collect())
    }

    /// Resolve a workflow task's timeout to a concrete number of seconds.
    ///
    /// The task's `timeout` may be a literal integer or a template expression
    /// (e.g., `{{ parameters.task_timeout_seconds }}`) that must resolve to an
    /// integer at scheduling time. Returns `None` when the task declares no
    /// timeout.
    fn resolve_workflow_task_timeout(
        task_node: &crate::workflow::graph::TaskNode,
        wf_ctx: &WorkflowContext,
    ) -> Result<Option<i64>> {
        let Some(value) = &task_node.timeout else {
            return Ok(None);
        };

        let rendered = wf_ctx.render_json(value).map_err(|error| {
            anyhow::anyhow!(
                "Failed to render timeout for workflow task '{}': {}",
                task_node.name,
                error
            )
        })?;

        let seconds = match rendered {
            JsonValue::Null => return Ok(None),
            JsonValue::Number(number) => number.as_i64().ok_or_else(|| {
                anyhow::anyhow!(
                    "timeout for workflow task '{}' must resolve to an integer; found {}",
                    task_node.name,
                    number
                )
            })?,
            JsonValue::String(text) => text.trim().parse::<i64>().map_err(|_| {
                anyhow::anyhow!(
                    "timeout for workflow task '{}' must resolve to an integer; found '{}'",
                    task_node.name,
                    text
                )
            })?,
            other => {
                return Err(anyhow::anyhow!(
                    "timeout for workflow task '{}' must resolve to an integer; found {}",
                    task_node.name,
                    other
                ));
            }
        };

        if seconds < 0 {
            return Err(anyhow::anyhow!(
                "timeout for workflow task '{}' must be a non-negative integer; found {}",
                task_node.name,
                seconds
            ));
        }

        Ok(Some(seconds))
    }

    fn render_cache_selector(
        _task_name: &str,
        field: &str,
        template: &str,
        wf_ctx: &WorkflowContext,
    ) -> CacheIterationInitializationResult<String> {
        let rendered = wf_ctx
            .render_json(&JsonValue::String(template.to_string()))
            .map_err(|_| {
                CacheIterationInitializationError::Logical(
                    CacheIterationInitializationFailure::SelectorRendering,
                )
            })?;
        match rendered {
            JsonValue::String(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
            JsonValue::Number(value) if field == "generation" => Ok(value.to_string()),
            _ => Err(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::SelectorRendering,
            )),
        }
    }

    async fn resolve_cache_owner_scope(
        conn: &mut PgConnection,
        config: &IterateCacheConfig,
        task_name: &str,
        parent_execution: &Execution,
        wf_ctx: &WorkflowContext,
    ) -> CacheIterationInitializationResult<(CacheOwnerScope, Option<String>)> {
        let default_pack_ref = parent_execution
            .action_ref
            .split_once('.')
            .map(|(pack_ref, _)| pack_ref.to_string());
        let owner_type = match config.owner_type.as_deref() {
            Some(template) => {
                let value = Self::render_cache_selector(task_name, "owner_type", template, wf_ctx)?
                    .to_ascii_lowercase();
                serde_json::from_value::<OwnerType>(JsonValue::String(value)).map_err(|_| {
                    CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    )
                })?
            }
            None => OwnerType::Pack,
        };
        let rendered_owner_ref = config
            .owner_ref
            .as_deref()
            .map(|template| Self::render_cache_selector(task_name, "owner_ref", template, wf_ctx))
            .transpose()?;

        let (scope, owner_ref) = match owner_type {
            OwnerType::System => {
                if rendered_owner_ref.is_some() {
                    return Err(CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ));
                }
                (CacheOwnerScope::system(), None)
            }
            OwnerType::Identity => {
                if rendered_owner_ref.is_some() {
                    return Err(CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ));
                }
                let identity_id =
                    parent_execution
                        .executor
                        .ok_or(CacheIterationInitializationError::Logical(
                            CacheIterationInitializationFailure::OwnerResolution,
                        ))?;
                (CacheOwnerScope::identity(identity_id), None)
            }
            OwnerType::Pack => {
                let reference = rendered_owner_ref.or(default_pack_ref).ok_or(
                    CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ),
                )?;
                let pack = PackRepository::find_by_ref(&mut *conn, &reference)
                    .await
                    .map_err(CacheIterationInitializationError::infrastructure)?
                    .ok_or(CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ))?;
                (
                    CacheOwnerScope::pack(pack.id, Some(reference.clone())),
                    Some(reference),
                )
            }
            OwnerType::Action => {
                let reference =
                    rendered_owner_ref.ok_or(CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ))?;
                let action = ActionRepository::find_by_ref(&mut *conn, &reference)
                    .await
                    .map_err(CacheIterationInitializationError::infrastructure)?
                    .ok_or(CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ))?;
                (
                    CacheOwnerScope::action(action.id, Some(reference.clone())),
                    Some(reference),
                )
            }
            OwnerType::Sensor => {
                let reference =
                    rendered_owner_ref.ok_or(CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ))?;
                let sensor = SensorRepository::find_by_ref(&mut *conn, &reference)
                    .await
                    .map_err(CacheIterationInitializationError::infrastructure)?
                    .ok_or(CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::OwnerResolution,
                    ))?;
                (
                    CacheOwnerScope::sensor(sensor.id, Some(reference.clone())),
                    Some(reference),
                )
            }
        };
        Ok((scope, owner_ref))
    }

    async fn authorize_cache_iteration_read(
        conn: &mut PgConnection,
        parent_execution: &Execution,
        task_action: &Action,
        owner_type: OwnerType,
        owner_ref: Option<&str>,
        namespace: &str,
        refs: &[String],
    ) -> CacheIterationInitializationResult<()> {
        let identity_id =
            parent_execution
                .executor
                .ok_or(CacheIterationInitializationError::Logical(
                    CacheIterationInitializationFailure::NotAuthorized,
                ))?;
        let identity = IdentityRepository::find_by_id(&mut *conn, identity_id)
            .await
            .map_err(CacheIterationInitializationError::infrastructure)?
            .ok_or(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::NotAuthorized,
            ))?;
        let named_refs = refs
            .iter()
            .filter(|reference| {
                reference.as_str() != attune_common::auth::jwt::STANDARD_EXECUTION_ACCESS_REF
            })
            .cloned()
            .collect::<Vec<_>>();
        let permission_sets = PermissionSetRepository::find_by_refs(&mut *conn, &named_refs)
            .await
            .map_err(CacheIterationInitializationError::infrastructure)?;
        if permission_sets.len() != named_refs.len() {
            return Err(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::PermissionResolution,
            ));
        }

        let mut grants = Vec::<Grant>::new();
        for permission_set in permission_sets {
            grants.extend(
                serde_json::from_value::<Vec<Grant>>(permission_set.grants).map_err(|_| {
                    CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::PermissionResolution,
                    )
                })?,
            );
        }
        let context = cache_iteration_authorization_context(
            identity_id,
            identity.attributes,
            owner_type,
            owner_ref,
            namespace,
        );
        let named_allowed = grants
            .iter()
            .any(|grant| grant.allows(Resource::Caches, RbacAction::Read, &context));

        let standard_allowed = standard_cache_read_allowed(
            refs.iter().any(|reference| {
                reference == attune_common::auth::jwt::STANDARD_EXECUTION_ACCESS_REF
            }),
            &task_action.r#ref,
            &parent_execution.action_ref,
            owner_type,
            owner_ref,
        );

        if named_allowed || standard_allowed {
            Ok(())
        } else {
            Err(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::NotAuthorized,
            ))
        }
    }

    fn workflow_task_placement_overrides(
        task_node: &crate::workflow::graph::TaskNode,
        wf_ctx: &WorkflowContext,
    ) -> Result<(Option<JsonValue>, Option<JsonValue>, Option<JsonValue>)> {
        let worker_selector = Self::render_workflow_placement_field(
            task_node,
            wf_ctx,
            "worker_selector",
            task_node.worker_selector.as_ref(),
            parse_worker_selector,
        )?;
        let worker_tolerations = Self::render_workflow_placement_field(
            task_node,
            wf_ctx,
            "worker_tolerations",
            task_node.worker_tolerations.as_ref(),
            parse_worker_tolerations,
        )?;
        let worker_affinity = Self::render_workflow_placement_field(
            task_node,
            wf_ctx,
            "worker_affinity",
            task_node.worker_affinity.as_ref(),
            parse_worker_affinity,
        )?;

        Ok((worker_selector, worker_tolerations, worker_affinity))
    }

    fn workflow_task_trace_tag(
        task_node: &crate::workflow::graph::TaskNode,
        parent_execution: &Execution,
        wf_ctx: &WorkflowContext,
    ) -> Result<Option<String>> {
        let Some(template) = &task_node.trace_tag_template else {
            return Ok(parent_execution.trace_tag.clone());
        };

        let rendered = wf_ctx
            .render_json(&JsonValue::String(template.clone()))
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to render trace_tag_template for workflow task '{}': {}",
                    task_node.name,
                    e
                )
            })?;

        let rendered_string = match rendered {
            JsonValue::Null => String::new(),
            JsonValue::String(value) => value,
            other => other.to_string(),
        };

        if rendered_string.trim().is_empty() {
            return Ok(parent_execution.trace_tag.clone());
        }

        Ok(Some(normalize_trace_tag(&rendered_string)?))
    }

    fn render_workflow_placement_field<T, E>(
        task_node: &crate::workflow::graph::TaskNode,
        wf_ctx: &WorkflowContext,
        field_name: &str,
        template: Option<&JsonValue>,
        validate: impl FnOnce(&JsonValue) -> std::result::Result<T, E>,
    ) -> Result<Option<JsonValue>>
    where
        E: std::fmt::Display,
    {
        let Some(template) = template else {
            return Ok(None);
        };

        let rendered = wf_ctx.render_json(template).map_err(|e| {
            anyhow::anyhow!(
                "Failed to render {} for workflow task '{}': {}",
                field_name,
                task_node.name,
                e
            )
        })?;
        validate(&rendered).map_err(|e| {
            anyhow::anyhow!(
                "Invalid {} for workflow task '{}': {}",
                field_name,
                task_node.name,
                e
            )
        })?;
        Ok(Some(rendered))
    }

    /// Create a child execution for a single workflow task and dispatch it to
    /// a worker. The child execution references the parent workflow execution
    /// via `workflow_task` metadata.
    ///
    /// `triggered_by` is the name of the predecessor task whose completion
    /// caused this task to be scheduled.  Pass `None` for entry-point tasks
    /// dispatched at workflow start.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_workflow_task(
        pool: &PgPool,
        publisher: &Publisher,
        _round_robin_counter: &AtomicUsize,
        parent_execution: &Execution,
        workflow_execution_id: &i64,
        task_node: &crate::workflow::graph::TaskNode,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
    ) -> Result<()> {
        let action_ref: String = match &task_node.action {
            Some(a) => a.clone(),
            None => {
                warn!(
                    "Workflow task '{}' has no action reference, skipping",
                    task_node.name
                );
                return Ok(());
            }
        };

        // Resolve the task's action from the database
        let task_action = ActionRepository::find_by_ref(pool, &action_ref).await?;
        let task_action = match task_action {
            Some(a) => a,
            None => {
                error!(
                    "Action '{}' not found for workflow task '{}'",
                    action_ref, task_node.name
                );
                return Err(anyhow::anyhow!(
                    "Action '{}' not found for workflow task '{}'",
                    action_ref,
                    task_node.name
                ));
            }
        };

        if task_node.iterate_cache.is_some() {
            return Self::dispatch_cache_iteration_task(
                pool,
                publisher,
                parent_execution,
                workflow_execution_id,
                task_node,
                &task_action,
                &action_ref,
                wf_ctx,
                encryption_key,
                triggered_by,
            )
            .await;
        }

        // -----------------------------------------------------------------
        // with_items expansion: if the task declares `with_items`, resolve
        // the list expression and create one child execution per item (up
        // to `concurrency` in parallel — though concurrency limiting is
        // left for a future enhancement; we fan out all items now).
        // -----------------------------------------------------------------
        if let Some(ref with_items_expr) = task_node.with_items {
            return Self::dispatch_with_items_task(
                pool,
                publisher,
                parent_execution,
                workflow_execution_id,
                task_node,
                &task_action,
                &action_ref,
                with_items_expr,
                wf_ctx,
                encryption_key,
                triggered_by,
            )
            .await;
        }

        // -----------------------------------------------------------------
        // Render task input templates through the WorkflowContext
        // -----------------------------------------------------------------
        let parent_execution_config = Self::restore_secret_entity(
            pool,
            encryption_key,
            ENTITY_EXECUTION_CONFIG,
            parent_execution.id,
            parent_execution.config.clone().unwrap_or(JsonValue::Null),
        )
        .await?;
        let rendered_input = Self::render_workflow_task_input(
            parent_execution,
            &parent_execution_config,
            task_node,
            &task_action,
            wf_ctx,
        )?;
        let task_config = if rendered_input.value.is_object()
            && !rendered_input.value.as_object().unwrap().is_empty()
        {
            Some(rendered_input.value.clone())
        } else {
            parent_execution.config.clone()
        };

        let permission_set_refs =
            Self::workflow_task_permission_set_refs(task_node, &task_action, wf_ctx)?;
        let (worker_selector, worker_tolerations, worker_affinity) =
            Self::workflow_task_placement_overrides(task_node, wf_ctx)?;
        let task_timeout_seconds = Self::resolve_workflow_task_timeout(task_node, wf_ctx)?;

        // Build workflow task metadata
        let workflow_task = WorkflowTaskMetadata {
            workflow_execution: *workflow_execution_id,
            task_name: task_node.name.clone(),
            triggered_by: triggered_by.map(String::from),
            task_index: None,
            task_batch: None,
            retry_count: 0,
            max_retries: task_node
                .retry
                .as_ref()
                .map(|r| r.count as i32)
                .unwrap_or(0),
            next_retry_at: None,
            timeout_seconds: task_timeout_seconds.map(|seconds| seconds as i32),
            timed_out: false,
            duration_ms: None,
            started_at: None,
            completed_at: None,
        };

        // Create child execution record, or reuse an existing one if another
        // scheduler/advance path already dispatched this workflow task.
        let child_execution_result = ExecutionRepository::create_workflow_task_if_absent(
            pool,
            CreateExecutionInput {
                action: Some(task_action.id),
                action_ref: action_ref.clone(),
                config: task_config,
                env_vars: parent_execution.env_vars.clone(),
                parent: Some(parent_execution.id),
                enforcement: parent_execution.enforcement,
                executor: parent_execution.executor,
                permission_set_refs,
                artifact_retention_policy: parent_execution
                    .artifact_retention_policy
                    .or(task_action.artifact_retention_policy),
                artifact_retention_limit: parent_execution
                    .artifact_retention_limit
                    .or(task_action.artifact_retention_limit),
                worker_selector,
                worker_tolerations,
                worker_affinity,
                worker: None,
                status: ExecutionStatus::Requested,
                trace_tag: Self::workflow_task_trace_tag(task_node, parent_execution, wf_ctx)?,
                timeout_seconds: Some(
                    task_timeout_seconds
                        .map(|seconds| seconds as i32)
                        .or(task_action.timeout_seconds)
                        .unwrap_or(
                            attune_common::config::app_default_execution_timeout_seconds() as i32,
                        ),
                ),
                result: None,
                workflow_task: Some(workflow_task),
            },
            *workflow_execution_id,
            &task_node.name,
            None,
        )
        .await?;
        let child_execution = child_execution_result.execution;
        if child_execution_result.created {
            Self::persist_execution_config_secrets(
                pool,
                encryption_key,
                child_execution.id,
                rendered_input.secret_inputs,
            )
            .await?;
        }

        if child_execution_result.created {
            info!(
                "Created child execution {} for workflow task '{}' (action '{}', workflow_execution {})",
                child_execution.id, task_node.name, action_ref, workflow_execution_id
            );
        } else {
            debug!(
                "Reusing child execution {} for workflow task '{}' (workflow_execution {})",
                child_execution.id, task_node.name, workflow_execution_id
            );
        }

        if child_execution.status == ExecutionStatus::Requested && action_ref == "core.ask" {
            let mut conn = pool.acquire().await?;
            Self::create_core_ask_inquiry(
                &mut conn,
                &child_execution,
                task_timeout_seconds,
                &rendered_input.value,
            )
            .await?;
            return Ok(());
        }

        if child_execution.status == ExecutionStatus::Requested {
            // If the task's action is itself a workflow, the recursive
            // `process_execution_requested` call will detect that and orchestrate
            // it in turn. For regular actions it will be dispatched to a worker.
            let payload = ExecutionRequestedPayload {
                execution_id: child_execution.id,
                action_id: Some(task_action.id),
                action_ref: action_ref.clone(),
                parent_id: Some(parent_execution.id),
                enforcement_id: parent_execution.enforcement,
                config: child_execution.config.clone(),
            };

            let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
                .with_source("executor-scheduler");

            publisher.publish_envelope(&envelope).await?;

            info!(
                "Published ExecutionRequested for child execution {} (task '{}')",
                child_execution.id, task_node.name
            );
        }

        Ok(())
    }

    /// If a failed workflow child has retry attempts remaining, create and
    /// publish the next attempt and leave workflow advancement paused until that
    /// retry reaches a terminal state.
    pub async fn maybe_retry_workflow_task(
        pool: &PgPool,
        publisher: &Publisher,
        execution: &Execution,
    ) -> Result<bool> {
        if !matches!(
            execution.status,
            ExecutionStatus::Failed | ExecutionStatus::Timeout
        ) {
            return Ok(false);
        }

        let Some(workflow_task) = execution.workflow_task.as_ref() else {
            return Ok(false);
        };

        if workflow_task.retry_count >= workflow_task.max_retries {
            return Ok(false);
        }

        let workflow_execution =
            WorkflowExecutionRepository::find_by_id(pool, workflow_task.workflow_execution)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Workflow execution {} not found for retry of execution {}",
                        workflow_task.workflow_execution,
                        execution.id
                    )
                })?;

        let graph: TaskGraph = serde_json::from_value(workflow_execution.task_graph.clone())
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to deserialize task graph for workflow_execution {}: {}",
                    workflow_task.workflow_execution,
                    e
                )
            })?;

        let Some(task_node) = graph.nodes.get(&workflow_task.task_name) else {
            warn!(
                "Workflow task '{}' not found in workflow_execution {}, cannot retry execution {}",
                workflow_task.task_name, workflow_task.workflow_execution, execution.id
            );
            return Ok(false);
        };

        let Some(retry_config) = task_node.retry.as_ref() else {
            return Ok(false);
        };

        let next_retry_count = workflow_task.retry_count + 1;
        if next_retry_count > retry_config.count as i32 {
            return Ok(false);
        }

        let base_delay = retry_config.delay;
        let mut delay_seconds = match retry_config.backoff {
            BackoffStrategy::Constant => base_delay,
            BackoffStrategy::Linear => base_delay.saturating_mul(next_retry_count as u32),
            BackoffStrategy::Exponential => {
                base_delay.saturating_mul(2_u32.saturating_pow((next_retry_count - 1) as u32))
            }
        };
        if let Some(max_delay) = retry_config.max_delay {
            delay_seconds = delay_seconds.min(max_delay);
        }

        let mut retry_metadata = workflow_task.clone();
        retry_metadata.retry_count = next_retry_count;
        retry_metadata.max_retries = retry_config.count as i32;
        retry_metadata.next_retry_at =
            Some(Utc::now() + chrono::Duration::seconds(delay_seconds as i64));
        retry_metadata.started_at = None;
        retry_metadata.completed_at = None;
        retry_metadata.duration_ms = None;
        retry_metadata.timed_out = false;

        let original_execution = execution.original_execution.unwrap_or(execution.id);
        let retry_execution = ExecutionRepository::create_retry(
            pool,
            CreateExecutionInput {
                action: execution.action,
                action_ref: execution.action_ref.clone(),
                config: execution.config.clone(),
                env_vars: execution.env_vars.clone(),
                parent: execution.parent,
                enforcement: execution.enforcement,
                executor: execution.executor,
                permission_set_refs: execution.permission_set_refs.clone(),
                artifact_retention_policy: execution.artifact_retention_policy,
                artifact_retention_limit: execution.artifact_retention_limit,
                worker_selector: execution.worker_selector.clone(),
                worker_tolerations: execution.worker_tolerations.clone(),
                worker_affinity: execution.worker_affinity.clone(),
                worker: None,
                status: ExecutionStatus::Requested,
                trace_tag: execution.trace_tag.clone(),
                timeout_seconds: execution.timeout_seconds,
                result: None,
                workflow_task: Some(retry_metadata),
            },
            next_retry_count,
            Some(retry_config.count as i32),
            Some(format!("{:?}", execution.status).to_lowercase()),
            original_execution,
        )
        .await?;

        info!(
            "Scheduled retry execution {} for workflow task '{}' after {}s ({}/{})",
            retry_execution.id,
            workflow_task.task_name,
            delay_seconds,
            next_retry_count,
            retry_config.count
        );

        if delay_seconds > 0 {
            tokio::time::sleep(Duration::from_secs(delay_seconds as u64)).await;
        }

        let payload = ExecutionRequestedPayload {
            execution_id: retry_execution.id,
            action_id: retry_execution.action,
            action_ref: retry_execution.action_ref.clone(),
            parent_id: retry_execution.parent,
            enforcement_id: retry_execution.enforcement,
            config: retry_execution.config.clone(),
        };
        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("executor-scheduler");
        publisher.publish_envelope(&envelope).await?;

        Ok(true)
    }

    async fn create_core_ask_inquiry(
        conn: &mut PgConnection,
        child_execution: &Execution,
        timeout_seconds: Option<i64>,
        rendered_input: &JsonValue,
    ) -> Result<()> {
        let prompt = rendered_input
            .get("prompt")
            .and_then(JsonValue::as_str)
            .unwrap_or("Approval required")
            .to_string();
        let response_schema = rendered_input.get("response_schema").cloned();
        let assigned_to = rendered_input
            .get("assigned_to")
            .and_then(JsonValue::as_i64);
        let timeout_at =
            timeout_seconds.map(|seconds| Utc::now() + chrono::Duration::seconds(seconds));

        let inquiry = InquiryRepository::create(
            &mut *conn,
            CreateInquiryInput {
                execution: child_execution.id,
                prompt,
                response_schema,
                assigned_to,
                status: InquiryStatus::Pending,
                response: None,
                timeout_at,
            },
        )
        .await?;

        let result = serde_json::json!({
            "inquiry_id": inquiry.id,
            "status": "pending"
        });
        let mut workflow_task = child_execution.workflow_task.clone();
        if let Some(metadata) = workflow_task.as_mut() {
            metadata.started_at = Some(Utc::now());
        }

        ExecutionRepository::update(
            &mut *conn,
            child_execution.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Running),
                result: Some(result),
                started_at: Some(Utc::now()),
                workflow_task,
                ..Default::default()
            },
        )
        .await?;

        info!(
            "Created inquiry {} for core.ask workflow execution {}",
            inquiry.id, child_execution.id
        );

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_workflow_task_with_conn(
        conn: &mut PgConnection,
        _round_robin_counter: &AtomicUsize,
        parent_execution: &Execution,
        workflow_execution_id: &i64,
        task_node: &crate::workflow::graph::TaskNode,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
        pending_messages: &mut Vec<PendingExecutionRequested>,
        pending_completions: &mut Vec<PendingExecutionCompleted>,
    ) -> Result<()> {
        let action_ref: String = match &task_node.action {
            Some(a) => a.clone(),
            None => {
                warn!(
                    "Workflow task '{}' has no action reference, skipping",
                    task_node.name
                );
                return Ok(());
            }
        };

        let task_action = ActionRepository::find_by_ref(&mut *conn, &action_ref).await?;
        let task_action = match task_action {
            Some(a) => a,
            None => {
                error!(
                    "Action '{}' not found for workflow task '{}'",
                    action_ref, task_node.name
                );
                return Err(anyhow::anyhow!(
                    "Action '{}' not found for workflow task '{}'",
                    action_ref,
                    task_node.name
                ));
            }
        };

        if task_node.iterate_cache.is_some() {
            return Self::dispatch_cache_iteration_task_with_conn(
                conn,
                parent_execution,
                workflow_execution_id,
                task_node,
                &task_action,
                &action_ref,
                wf_ctx,
                encryption_key,
                triggered_by,
                pending_messages,
                pending_completions,
            )
            .await;
        }

        if let Some(ref with_items_expr) = task_node.with_items {
            return Self::dispatch_with_items_task_with_conn(
                conn,
                parent_execution,
                workflow_execution_id,
                task_node,
                &task_action,
                &action_ref,
                with_items_expr,
                wf_ctx,
                encryption_key,
                triggered_by,
                pending_messages,
            )
            .await;
        }

        let parent_execution_config = Self::restore_secret_entity(
            &mut *conn,
            encryption_key,
            ENTITY_EXECUTION_CONFIG,
            parent_execution.id,
            parent_execution.config.clone().unwrap_or(JsonValue::Null),
        )
        .await?;
        let rendered_input = Self::render_workflow_task_input(
            parent_execution,
            &parent_execution_config,
            task_node,
            &task_action,
            wf_ctx,
        )?;
        let task_config: Option<JsonValue> = if rendered_input.value.is_object()
            && !rendered_input.value.as_object().unwrap().is_empty()
        {
            Some(rendered_input.value.clone())
        } else {
            parent_execution.config.clone()
        };

        let permission_set_refs =
            Self::workflow_task_permission_set_refs(task_node, &task_action, wf_ctx)?;
        let (worker_selector, worker_tolerations, worker_affinity) =
            Self::workflow_task_placement_overrides(task_node, wf_ctx)?;
        let task_timeout_seconds = Self::resolve_workflow_task_timeout(task_node, wf_ctx)?;

        let workflow_task = WorkflowTaskMetadata {
            workflow_execution: *workflow_execution_id,
            task_name: task_node.name.clone(),
            triggered_by: triggered_by.map(String::from),
            task_index: None,
            task_batch: None,
            retry_count: 0,
            max_retries: task_node
                .retry
                .as_ref()
                .map(|r| r.count as i32)
                .unwrap_or(0),
            next_retry_at: None,
            timeout_seconds: task_timeout_seconds.map(|seconds| seconds as i32),
            timed_out: false,
            duration_ms: None,
            started_at: None,
            completed_at: None,
        };

        let child_execution_result = ExecutionRepository::create_workflow_task_if_absent_with_conn(
            &mut *conn,
            CreateExecutionInput {
                action: Some(task_action.id),
                action_ref: action_ref.clone(),
                config: task_config,
                env_vars: parent_execution.env_vars.clone(),
                parent: Some(parent_execution.id),
                enforcement: parent_execution.enforcement,
                executor: parent_execution.executor,
                permission_set_refs,
                artifact_retention_policy: parent_execution
                    .artifact_retention_policy
                    .or(task_action.artifact_retention_policy),
                artifact_retention_limit: parent_execution
                    .artifact_retention_limit
                    .or(task_action.artifact_retention_limit),
                worker_selector,
                worker_tolerations,
                worker_affinity,
                worker: None,
                status: ExecutionStatus::Requested,
                trace_tag: Self::workflow_task_trace_tag(task_node, parent_execution, wf_ctx)?,
                timeout_seconds: Some(
                    task_timeout_seconds
                        .map(|seconds| seconds as i32)
                        .or(task_action.timeout_seconds)
                        .unwrap_or(
                            attune_common::config::app_default_execution_timeout_seconds() as i32,
                        ),
                ),
                result: None,
                workflow_task: Some(workflow_task),
            },
            *workflow_execution_id,
            &task_node.name,
            None,
        )
        .await?;
        let child_execution = child_execution_result.execution;
        if child_execution_result.created {
            Self::persist_execution_config_secrets_with_conn(
                &mut *conn,
                encryption_key,
                child_execution.id,
                rendered_input.secret_inputs,
            )
            .await?;
        }

        if child_execution_result.created {
            info!(
                "Created child execution {} for workflow task '{}' (action '{}', workflow_execution {})",
                child_execution.id, task_node.name, action_ref, workflow_execution_id
            );
        } else {
            debug!(
                "Reusing child execution {} for workflow task '{}' (workflow_execution {})",
                child_execution.id, task_node.name, workflow_execution_id
            );
        }

        if child_execution.status == ExecutionStatus::Requested && action_ref == "core.ask" {
            Self::create_core_ask_inquiry(
                &mut *conn,
                &child_execution,
                task_timeout_seconds,
                &rendered_input.value,
            )
            .await?;
            return Ok(());
        }

        if child_execution.status == ExecutionStatus::Requested {
            pending_messages.push(PendingExecutionRequested {
                execution_id: child_execution.id,
                action_id: task_action.id,
                action_ref: action_ref.clone(),
                parent_id: parent_execution.id,
                enforcement_id: parent_execution.enforcement,
                config: child_execution.config.clone(),
            });
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_cache_iteration_task(
        pool: &PgPool,
        publisher: &Publisher,
        parent_execution: &Execution,
        workflow_execution_id: &i64,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        action_ref: &str,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
    ) -> Result<()> {
        let mut tx = pool.begin().await?;
        let mut pending_messages = Vec::new();
        let mut pending_completions = Vec::new();
        let result = Self::dispatch_cache_iteration_task_with_conn(
            &mut tx,
            parent_execution,
            workflow_execution_id,
            task_node,
            task_action,
            action_ref,
            wf_ctx,
            encryption_key,
            triggered_by,
            &mut pending_messages,
            &mut pending_completions,
        )
        .await;
        match result {
            Ok(()) => tx.commit().await?,
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
        }
        for pending in pending_messages {
            Self::publish_execution_requested_payload(publisher, pending).await?;
        }
        for completed in pending_completions {
            Self::publish_execution_completed_payload(publisher, completed).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_cache_iteration_task_with_conn(
        conn: &mut PgConnection,
        parent_execution: &Execution,
        workflow_execution_id: &i64,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        action_ref: &str,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
        pending_messages: &mut Vec<PendingExecutionRequested>,
        pending_completions: &mut Vec<PendingExecutionCompleted>,
    ) -> Result<()> {
        let config = task_node.iterate_cache.as_ref().ok_or_else(|| {
            anyhow::anyhow!("cache iteration dispatch requires iterate_cache configuration")
        })?;
        let task_timeout_seconds = Self::resolve_workflow_task_timeout(task_node, wf_ctx)?;
        let existing = WorkflowCacheIterationRepository::find_by_workflow_task_for_update(
            &mut *conn,
            *workflow_execution_id,
            &task_node.name,
        )
        .await?;
        let initialized = match Self::initialize_cache_iteration_with_conn(
            conn,
            parent_execution,
            task_node,
            task_action,
            wf_ctx,
            config,
            existing,
        )
        .await
        {
            Ok(initialized) => initialized,
            Err(CacheIterationInitializationError::Infrastructure(error)) => return Err(error),
            Err(CacheIterationInitializationError::Logical(failure)) => {
                let existing = WorkflowCacheIterationRepository::find_by_workflow_task_for_update(
                    &mut *conn,
                    *workflow_execution_id,
                    &task_node.name,
                )
                .await?;
                if let Some(iteration) = existing.as_ref() {
                    if iteration.state == WorkflowCacheIterationState::Scanning {
                        WorkflowCacheIterationRepository::mark_terminal(
                            &mut *conn,
                            iteration.id,
                            WorkflowCacheIterationState::Failed,
                            Some("cache iteration initialization failed"),
                        )
                        .await?;
                    }
                }
                if existing
                    .as_ref()
                    .is_none_or(|iteration| iteration.dispatched_count == 0)
                {
                    Self::create_cache_iteration_terminal_child_with_conn(
                        conn,
                        parent_execution,
                        task_node,
                        task_action,
                        action_ref,
                        *workflow_execution_id,
                        triggered_by,
                        task_timeout_seconds,
                        WorkflowCacheIterationState::Failed,
                        pending_completions,
                    )
                    .await?;
                }
                warn!(
                    "Cache iteration initialization failed for workflow task '{}' ({})",
                    task_node.name,
                    failure.code()
                );
                return Ok(());
            }
        };

        let iteration = if let Some(iteration) = initialized.existing {
            iteration
        } else {
            WorkflowCacheIterationRepository::create_or_find_for_update(
                conn,
                CreateWorkflowCacheIterationInput {
                    workflow_execution: *workflow_execution_id,
                    task_name: task_node.name.clone(),
                    namespace: initialized.namespace_id,
                    generation: initialized.generation_id,
                    page_size: i32::try_from(config.page_size)?,
                    batch_size: i32::try_from(task_node.batch_size.unwrap_or(1))?,
                    concurrency: i32::try_from(task_node.concurrency.unwrap_or(1))?,
                },
            )
            .await?
        };
        if iteration.state != WorkflowCacheIterationState::Scanning {
            if iteration.dispatched_count == 0 {
                Self::create_cache_iteration_terminal_child_with_conn(
                    conn,
                    parent_execution,
                    task_node,
                    task_action,
                    action_ref,
                    iteration.workflow_execution,
                    triggered_by,
                    task_timeout_seconds,
                    iteration.state,
                    pending_completions,
                )
                .await?;
            }
            return Ok(());
        }

        let iteration_id = iteration.id;
        let refill_result = Self::refill_cache_iteration_with_conn(
            conn,
            parent_execution,
            task_node,
            task_action,
            action_ref,
            wf_ctx,
            encryption_key,
            triggered_by,
            iteration,
            pending_messages,
            pending_completions,
        )
        .await;
        if let Err(error) = refill_result {
            WorkflowCacheIterationRepository::mark_terminal(
                &mut *conn,
                iteration_id,
                WorkflowCacheIterationState::Failed,
                Some("cache iteration materialization failed"),
            )
            .await?;
            let iteration = WorkflowCacheIterationRepository::find_by_id(&mut *conn, iteration_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Cache iteration state is missing"))?;
            if iteration.dispatched_count == 0 {
                Self::create_cache_iteration_terminal_child_with_conn(
                    conn,
                    parent_execution,
                    task_node,
                    task_action,
                    action_ref,
                    iteration.workflow_execution,
                    triggered_by,
                    task_timeout_seconds,
                    WorkflowCacheIterationState::Failed,
                    pending_completions,
                )
                .await?;
            }
            warn!(
                "Cache iteration for workflow task '{}' failed: {}",
                task_node.name, error
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn initialize_cache_iteration_with_conn(
        conn: &mut PgConnection,
        parent_execution: &Execution,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        wf_ctx: &WorkflowContext,
        config: &IterateCacheConfig,
        existing: Option<WorkflowCacheIteration>,
    ) -> CacheIterationInitializationResult<InitializedCacheIteration> {
        let namespace_name =
            Self::render_cache_selector(&task_node.name, "namespace", &config.namespace, wf_ctx)?
                .to_ascii_lowercase();
        let permission_set_refs =
            Self::workflow_task_permission_set_refs(task_node, task_action, wf_ctx).map_err(
                |_| {
                    CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::PermissionResolution,
                    )
                },
            )?;
        if !named_cache_permission_refs_are_delegated(
            &permission_set_refs,
            &parent_execution.permission_set_refs,
        ) {
            return Err(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::NotAuthorized,
            ));
        }
        let (owner_scope, owner_ref) = Self::resolve_cache_owner_scope(
            conn,
            config,
            &task_node.name,
            parent_execution,
            wf_ctx,
        )
        .await?;
        Self::authorize_cache_iteration_read(
            conn,
            parent_execution,
            task_action,
            owner_scope.owner_type,
            owner_ref.as_deref(),
            &namespace_name,
            &permission_set_refs,
        )
        .await?;
        let namespace =
            CacheNamespaceRepository::resolve(&mut *conn, &owner_scope, &namespace_name)
                .await
                .map_err(CacheIterationInitializationError::infrastructure)?
                .ok_or(CacheIterationInitializationError::Logical(
                    CacheIterationInitializationFailure::NamespaceResolution,
                ))?;

        let generation_id = if let Some(iteration) = existing.as_ref() {
            if iteration.namespace != namespace.id {
                return Err(CacheIterationInitializationError::Logical(
                    CacheIterationInitializationFailure::NamespaceResolution,
                ));
            }
            iteration.generation
        } else {
            let selector = Self::render_cache_selector(
                &task_node.name,
                "generation",
                &config.generation,
                wf_ctx,
            )?;
            match parse_cache_generation_selector(&selector)
                .map_err(CacheIterationInitializationError::Logical)?
            {
                CacheGenerationSelector::Active => namespace.active_generation.ok_or(
                    CacheIterationInitializationError::Logical(
                        CacheIterationInitializationFailure::NoActiveGeneration,
                    ),
                )?,
                CacheGenerationSelector::Explicit(id) => id,
            }
        };
        let generation = CacheGenerationRepository::find_by_id_for_share(conn, generation_id)
            .await
            .map_err(CacheIterationInitializationError::infrastructure)?
            .filter(|generation| generation.namespace == namespace.id)
            .ok_or(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::InvalidGeneration,
            ))?;
        let readable = generation.state == CacheGenerationState::Active
            || (generation.state == CacheGenerationState::Retired
                && generation
                    .readable_until
                    .is_some_and(|until| until > Utc::now()))
            || existing.as_ref().is_some_and(|iteration| {
                iteration.state == WorkflowCacheIterationState::Scanning
                    && iteration.generation == generation.id
            });
        if !readable {
            return Err(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::GenerationNotReadable,
            ));
        }
        let stale = cache_generation_is_stale(
            generation.state,
            generation.activated,
            namespace.freshness_target_seconds,
            Utc::now(),
        );
        if config.require_fresh && stale && existing.is_none() {
            return Err(CacheIterationInitializationError::Logical(
                CacheIterationInitializationFailure::StaleGeneration,
            ));
        }

        Ok(InitializedCacheIteration {
            existing,
            namespace_id: namespace.id,
            generation_id,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_cache_iteration_terminal_child_with_conn(
        conn: &mut PgConnection,
        parent_execution: &Execution,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        action_ref: &str,
        workflow_execution_id: i64,
        triggered_by: Option<&str>,
        timeout_seconds: Option<i64>,
        state: WorkflowCacheIterationState,
        pending_completions: &mut Vec<PendingExecutionCompleted>,
    ) -> Result<()> {
        let status = match state {
            WorkflowCacheIterationState::Completed => ExecutionStatus::Completed,
            WorkflowCacheIterationState::Failed => ExecutionStatus::Failed,
            WorkflowCacheIterationState::Cancelled => ExecutionStatus::Cancelled,
            WorkflowCacheIterationState::Scanning => {
                return Err(anyhow::anyhow!("terminal cache iteration state required"));
            }
        };
        let metadata = WorkflowTaskMetadata {
            workflow_execution: workflow_execution_id,
            task_name: task_node.name.clone(),
            triggered_by: triggered_by.map(str::to_string),
            task_index: None,
            task_batch: Some(0),
            retry_count: 0,
            max_retries: 0,
            next_retry_at: None,
            timeout_seconds: timeout_seconds.map(|seconds| seconds as i32),
            timed_out: false,
            duration_ms: Some(0),
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
        };
        let result = ExecutionRepository::create_workflow_task_if_absent_with_conn(
            &mut *conn,
            CreateExecutionInput {
                action: Some(task_action.id),
                action_ref: action_ref.to_string(),
                config: None,
                env_vars: parent_execution.env_vars.clone(),
                parent: Some(parent_execution.id),
                enforcement: parent_execution.enforcement,
                executor: parent_execution.executor,
                permission_set_refs: Vec::new(),
                artifact_retention_policy: parent_execution
                    .artifact_retention_policy
                    .or(task_action.artifact_retention_policy),
                artifact_retention_limit: parent_execution
                    .artifact_retention_limit
                    .or(task_action.artifact_retention_limit),
                worker_selector: None,
                worker_tolerations: None,
                worker_affinity: None,
                worker: None,
                status,
                trace_tag: parent_execution.trace_tag.clone(),
                timeout_seconds: Some(
                    attune_common::config::app_default_execution_timeout_seconds() as i32,
                ),
                result: Some(cache_iteration_terminal_result(state)),
                workflow_task: Some(metadata),
            },
            workflow_execution_id,
            &task_node.name,
            None,
        )
        .await?;
        let execution = result.execution;
        pending_completions.push(PendingExecutionCompleted {
            execution_id: execution.id,
            action_id: task_action.id,
            action_ref: action_ref.to_string(),
            status,
            result: execution.result,
            completed_at: Utc::now(),
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn refill_cache_iteration_with_conn(
        conn: &mut PgConnection,
        parent_execution: &Execution,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        action_ref: &str,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
        mut iteration: attune_common::models::WorkflowCacheIteration,
        pending_messages: &mut Vec<PendingExecutionRequested>,
        pending_completions: &mut Vec<PendingExecutionCompleted>,
    ) -> Result<()> {
        let attempt_summary = ExecutionRepository::summarize_workflow_task_latest_attempts(
            &mut *conn,
            iteration.workflow_execution,
            &iteration.task_name,
        )
        .await?;
        if attempt_summary.has_failed {
            WorkflowCacheIterationRepository::mark_terminal(
                &mut *conn,
                iteration.id,
                WorkflowCacheIterationState::Failed,
                Some("one or more cache iteration batches failed"),
            )
            .await?;
            return Ok(());
        }

        let in_flight = usize::try_from(attempt_summary.in_flight)?;
        let slots = usize::try_from(iteration.concurrency)?.saturating_sub(in_flight);
        let task_timeout_seconds = Self::resolve_workflow_task_timeout(task_node, wf_ctx)?;
        let parent_execution_config = Self::restore_secret_entity(
            &mut *conn,
            encryption_key,
            ENTITY_EXECUTION_CONFIG,
            parent_execution.id,
            parent_execution.config.clone().unwrap_or(JsonValue::Null),
        )
        .await?;
        let mut exhausted = false;
        let mut created_batches = 0usize;
        for _ in 0..slots {
            let mut entries = Vec::new();
            let mut cursor = iteration.last_external_id.clone();
            let mut materialized_bytes = 0usize;
            let batched = iteration.batch_size > 1;
            let mut budget_exhausted = false;
            while entries.len() < usize::try_from(iteration.batch_size)? {
                let remaining = usize::try_from(iteration.batch_size)? - entries.len();
                let limit = usize::try_from(iteration.page_size)?.min(remaining);
                let remaining_bytes = usize::try_from(MAX_SCAN_MATERIALIZATION_BYTES)?
                    .saturating_sub(materialized_bytes);
                if remaining_bytes == 0 {
                    break;
                }
                let page = CacheEntryRepository::scan_pinned_page_with_budget_conn(
                    conn,
                    iteration.namespace,
                    iteration.generation,
                    cursor.as_deref(),
                    i64::try_from(limit)?,
                    i64::try_from(remaining_bytes)?,
                )
                .await?;
                if page.entries.is_empty() {
                    exhausted = true;
                    break;
                }
                for entry in page.entries {
                    if !try_materialize_cache_iteration_entry(
                        &mut entries,
                        &mut cursor,
                        &mut materialized_bytes,
                        entry,
                        batched,
                        usize::try_from(MAX_SCAN_MATERIALIZATION_BYTES)?,
                    )? {
                        budget_exhausted = true;
                        break;
                    }
                }
                if budget_exhausted {
                    break;
                }
                if !page.has_more {
                    exhausted = true;
                    break;
                }
            }
            if entries.is_empty() {
                break;
            }

            let batch_index = i32::try_from(iteration.next_batch_index)?;
            let item = if iteration.batch_size == 1 {
                entries.remove(0)
            } else {
                JsonValue::Array(entries)
            };
            let batch_count = if let JsonValue::Array(values) = &item {
                values.len()
            } else {
                1
            };
            let mut item_ctx = wf_ctx.clone();
            item_ctx.set_current_item(item, usize::try_from(batch_index)?);
            let batch_timeout_seconds =
                Self::resolve_workflow_task_timeout(task_node, &item_ctx)?;
            let rendered_input = Self::render_workflow_task_input(
                parent_execution,
                &parent_execution_config,
                task_node,
                task_action,
                &item_ctx,
            )?;
            let task_config = if rendered_input.value.is_object()
                && !rendered_input
                    .value
                    .as_object()
                    .expect("checked object")
                    .is_empty()
            {
                Some(rendered_input.value.clone())
            } else {
                parent_execution.config.clone()
            };
            let permission_set_refs =
                Self::workflow_task_permission_set_refs(task_node, task_action, &item_ctx)?;
            let (worker_selector, worker_tolerations, worker_affinity) =
                Self::workflow_task_placement_overrides(task_node, &item_ctx)?;
            let workflow_task = WorkflowTaskMetadata {
                workflow_execution: iteration.workflow_execution,
                task_name: task_node.name.clone(),
                triggered_by: triggered_by.map(str::to_string),
                task_index: Some(batch_index),
                task_batch: Some(i32::try_from(batch_count)?),
                retry_count: 0,
                max_retries: task_node
                    .retry
                    .as_ref()
                    .map(|retry| retry.count as i32)
                    .unwrap_or(0),
                next_retry_at: None,
                timeout_seconds: batch_timeout_seconds.map(|seconds| seconds as i32),
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            };
            let child_result = ExecutionRepository::create_workflow_task_if_absent_with_conn(
                &mut *conn,
                CreateExecutionInput {
                    action: Some(task_action.id),
                    action_ref: action_ref.to_string(),
                    config: task_config,
                    env_vars: parent_execution.env_vars.clone(),
                    parent: Some(parent_execution.id),
                    enforcement: parent_execution.enforcement,
                    executor: parent_execution.executor,
                    permission_set_refs,
                    artifact_retention_policy: parent_execution
                        .artifact_retention_policy
                        .or(task_action.artifact_retention_policy),
                    artifact_retention_limit: parent_execution
                        .artifact_retention_limit
                        .or(task_action.artifact_retention_limit),
                    worker_selector,
                    worker_tolerations,
                    worker_affinity,
                    worker: None,
                    status: ExecutionStatus::Requested,
                    trace_tag: Self::workflow_task_trace_tag(
                        task_node,
                        parent_execution,
                        &item_ctx,
                    )?,
                    timeout_seconds: Some(
                        batch_timeout_seconds
                            .map(|seconds| seconds as i32)
                            .or(task_action.timeout_seconds)
                            .unwrap_or(
                                attune_common::config::app_default_execution_timeout_seconds()
                                    as i32,
                            ),
                    ),
                    result: None,
                    workflow_task: Some(workflow_task),
                },
                iteration.workflow_execution,
                &task_node.name,
                Some(batch_index),
            )
            .await?;
            if child_result.created {
                created_batches += 1;
                Self::persist_execution_config_secrets_with_conn(
                    &mut *conn,
                    encryption_key,
                    child_result.execution.id,
                    rendered_input.secret_inputs,
                )
                .await?;
            }
            if child_result.execution.status == ExecutionStatus::Requested {
                Self::publish_execution_requested_with_conn(
                    &mut *conn,
                    child_result.execution.id,
                    task_action.id,
                    action_ref,
                    parent_execution,
                    pending_messages,
                )
                .await?;
            }
            iteration = WorkflowCacheIterationRepository::update_scan_progress(
                &mut *conn,
                iteration.id,
                UpdateWorkflowCacheIterationProgressInput {
                    last_external_id: cursor.expect("non-empty cache batch has a cursor"),
                    next_batch_index: iteration.next_batch_index + 1,
                    scanned_count: iteration.scanned_count + i64::try_from(batch_count)?,
                    dispatched_count: iteration.dispatched_count + 1,
                },
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("cache iteration cursor did not advance"))?;
            if exhausted {
                break;
            }
        }

        if exhausted && in_flight == 0 && created_batches == 0 {
            WorkflowCacheIterationRepository::mark_terminal(
                &mut *conn,
                iteration.id,
                WorkflowCacheIterationState::Completed,
                None,
            )
            .await?;
            if iteration.dispatched_count == 0 {
                Self::create_cache_iteration_terminal_child_with_conn(
                    conn,
                    parent_execution,
                    task_node,
                    task_action,
                    action_ref,
                    iteration.workflow_execution,
                    triggered_by,
                    task_timeout_seconds,
                    WorkflowCacheIterationState::Completed,
                    pending_completions,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Expand a `with_items` task into child executions.
    ///
    /// The `with_items` expression (e.g. `"{{ number_list }}"`) is resolved
    /// via the workflow context to produce a JSON array.  ALL child execution
    /// records are created in the database up front so that the sibling-count
    /// query in [`advance_workflow`] sees the complete set.
    ///
    /// When a `concurrency` limit is set on the task, only the first
    /// `concurrency` items are published to the message queue.  The remaining
    /// children stay at `Requested` status in the database.  As each item
    /// completes, [`advance_workflow`] publishes the next `Requested` sibling
    /// to keep the concurrency window full.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_with_items_task(
        pool: &PgPool,
        publisher: &Publisher,
        parent_execution: &Execution,
        workflow_execution_id: &i64,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        action_ref: &str,
        with_items_expr: &str,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
    ) -> Result<()> {
        // Resolve the with_items expression to a JSON array
        let items_value = wf_ctx
            .render_json(&JsonValue::String(with_items_expr.to_string()))
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to resolve with_items expression '{}' for task '{}': {}",
                    with_items_expr,
                    task_node.name,
                    e
                )
            })?;

        let items = match items_value.as_array() {
            Some(arr) => arr.clone(),
            None => {
                warn!(
                    "with_items for task '{}' resolved to non-array value: {:?}. \
                     Wrapping in single-element array.",
                    task_node.name, items_value
                );
                vec![items_value]
            }
        };
        let items = iteration_items(items, task_node.batch_size);

        let total = items.len();
        let concurrency_limit = task_node.concurrency.unwrap_or(1);
        let dispatch_count = total.min(concurrency_limit);

        info!(
            "Expanding with_items for task '{}': {} items (concurrency: {}, dispatching first {})",
            task_node.name, total, concurrency_limit, dispatch_count
        );

        // Phase 1: Create ALL child execution records in the database.
        // Each row captures the fully-rendered input so we never need to
        // re-render templates later when publishing deferred items.
        let existing_children: Vec<(i64, i32, ExecutionStatus)> = sqlx::query_as(
            "SELECT id, COALESCE((workflow_task->>'task_index')::int, -1) as task_index, status \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND workflow_task->>'task_name' = $2 \
               AND workflow_task->>'task_index' IS NOT NULL \
             ORDER BY (workflow_task->>'task_index')::int ASC",
        )
        .bind(workflow_execution_id.to_string())
        .bind(task_node.name.as_str())
        .fetch_all(pool)
        .await?;

        let existing_by_index: std::collections::HashMap<usize, (i64, ExecutionStatus)> =
            existing_children
                .into_iter()
                .filter_map(|(id, task_index, status)| {
                    usize::try_from(task_index)
                        .ok()
                        .map(|index| (index, (id, status)))
                })
                .collect();

        let mut child_ids: Vec<i64> = Vec::with_capacity(total);

        for (index, item) in items.iter().enumerate() {
            if let Some((existing_id, _)) = existing_by_index.get(&index) {
                child_ids.push(*existing_id);
                continue;
            }

            let mut item_ctx = wf_ctx.clone();
            item_ctx.set_current_item(item.clone(), index);

            let parent_execution_config = Self::restore_secret_entity(
                pool,
                encryption_key,
                ENTITY_EXECUTION_CONFIG,
                parent_execution.id,
                parent_execution.config.clone().unwrap_or(JsonValue::Null),
            )
            .await?;
            let rendered_input = Self::render_workflow_task_input(
                parent_execution,
                &parent_execution_config,
                task_node,
                task_action,
                &item_ctx,
            )?;

            // Store as flat parameters (consistent with manual and rule-triggered
            // executions) — no {"parameters": ...} wrapper.
            let task_config: Option<JsonValue> = if rendered_input.value.is_object()
                && !rendered_input.value.as_object().unwrap().is_empty()
            {
                Some(rendered_input.value.clone())
            } else {
                parent_execution.config.clone()
            };

            let permission_set_refs =
                Self::workflow_task_permission_set_refs(task_node, task_action, &item_ctx)?;
            let (worker_selector, worker_tolerations, worker_affinity) =
                Self::workflow_task_placement_overrides(task_node, &item_ctx)?;
            let item_timeout_seconds =
                Self::resolve_workflow_task_timeout(task_node, &item_ctx)?;

            let workflow_task = WorkflowTaskMetadata {
                workflow_execution: *workflow_execution_id,
                task_name: task_node.name.clone(),
                triggered_by: triggered_by.map(String::from),
                task_index: Some(index as i32),
                task_batch: None,
                retry_count: 0,
                max_retries: task_node
                    .retry
                    .as_ref()
                    .map(|r| r.count as i32)
                    .unwrap_or(0),
                next_retry_at: None,
                timeout_seconds: item_timeout_seconds.map(|seconds| seconds as i32),
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            };

            let child_execution_result = ExecutionRepository::create_workflow_task_if_absent(
                pool,
                CreateExecutionInput {
                    action: Some(task_action.id),
                    action_ref: action_ref.to_string(),
                    config: task_config,
                    env_vars: parent_execution.env_vars.clone(),
                    parent: Some(parent_execution.id),
                    enforcement: parent_execution.enforcement,
                    executor: parent_execution.executor,
                    permission_set_refs,
                    artifact_retention_policy: parent_execution
                        .artifact_retention_policy
                        .or(task_action.artifact_retention_policy),
                    artifact_retention_limit: parent_execution
                        .artifact_retention_limit
                        .or(task_action.artifact_retention_limit),
                    worker_selector,
                    worker_tolerations,
                    worker_affinity,
                    worker: None,
                    status: ExecutionStatus::Requested,
                    trace_tag: Self::workflow_task_trace_tag(
                        task_node,
                        parent_execution,
                        &item_ctx,
                    )?,
                    timeout_seconds: Some(
                        item_timeout_seconds
                            .map(|seconds| seconds as i32)
                            .or(task_action.timeout_seconds)
                            .unwrap_or(
                                attune_common::config::app_default_execution_timeout_seconds()
                                    as i32,
                            ),
                    ),
                    result: None,
                    workflow_task: Some(workflow_task),
                },
                *workflow_execution_id,
                &task_node.name,
                Some(index as i32),
            )
            .await?;
            let child_execution = child_execution_result.execution;
            if child_execution_result.created {
                Self::persist_execution_config_secrets(
                    pool,
                    encryption_key,
                    child_execution.id,
                    rendered_input.secret_inputs,
                )
                .await?;
            }

            if child_execution_result.created {
                info!(
                    "Created with_items child execution {} for task '{}' item {} \
                     (action '{}', workflow_execution {})",
                    child_execution.id, task_node.name, index, action_ref, workflow_execution_id
                );
            } else {
                debug!(
                    "Reusing with_items child execution {} for task '{}' item {} \
                     (workflow_execution {})",
                    child_execution.id, task_node.name, index, workflow_execution_id
                );
            }

            child_ids.push(child_execution.id);
        }

        // Phase 2: Publish only the first `dispatch_count` to the MQ.
        // The rest stay at Requested status until advance_workflow picks
        // them up as earlier items complete.
        for &child_id in child_ids.iter().take(dispatch_count) {
            let child = ExecutionRepository::find_by_id(pool, child_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Execution {} not found", child_id))?;

            if child.status == ExecutionStatus::Requested {
                Self::publish_execution_requested(
                    pool,
                    publisher,
                    child_id,
                    task_action.id,
                    action_ref,
                    parent_execution,
                )
                .await?;
            }
        }

        info!(
            "Dispatched {} of {} with_items child executions for task '{}'",
            dispatch_count, total, task_node.name
        );

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_with_items_task_with_conn(
        conn: &mut PgConnection,
        parent_execution: &Execution,
        workflow_execution_id: &i64,
        task_node: &crate::workflow::graph::TaskNode,
        task_action: &Action,
        action_ref: &str,
        with_items_expr: &str,
        wf_ctx: &WorkflowContext,
        encryption_key: Option<&str>,
        triggered_by: Option<&str>,
        pending_messages: &mut Vec<PendingExecutionRequested>,
    ) -> Result<()> {
        let items_value = wf_ctx
            .render_json(&JsonValue::String(with_items_expr.to_string()))
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to resolve with_items expression '{}' for task '{}': {}",
                    with_items_expr,
                    task_node.name,
                    e
                )
            })?;

        let items = match items_value.as_array() {
            Some(arr) => arr.clone(),
            None => {
                warn!(
                    "with_items for task '{}' resolved to non-array value: {:?}. \
                     Wrapping in single-element array.",
                    task_node.name, items_value
                );
                vec![items_value]
            }
        };
        let items = iteration_items(items, task_node.batch_size);

        let total = items.len();
        let concurrency_limit = task_node.concurrency.unwrap_or(1);
        let dispatch_count = total.min(concurrency_limit);

        info!(
            "Expanding with_items for task '{}': {} items (concurrency: {}, dispatching first {})",
            task_node.name, total, concurrency_limit, dispatch_count
        );

        let existing_children: Vec<(i64, i32, ExecutionStatus)> = sqlx::query_as(
            "SELECT id, COALESCE((workflow_task->>'task_index')::int, -1) as task_index, status \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND workflow_task->>'task_name' = $2 \
               AND workflow_task->>'task_index' IS NOT NULL \
             ORDER BY (workflow_task->>'task_index')::int ASC",
        )
        .bind(workflow_execution_id.to_string())
        .bind(task_node.name.as_str())
        .fetch_all(&mut *conn)
        .await?;

        let existing_by_index: HashMap<usize, (i64, ExecutionStatus)> = existing_children
            .into_iter()
            .filter_map(|(id, task_index, status)| {
                usize::try_from(task_index)
                    .ok()
                    .map(|index| (index, (id, status)))
            })
            .collect();

        let mut child_ids: Vec<i64> = Vec::with_capacity(total);

        for (index, item) in items.iter().enumerate() {
            if let Some((existing_id, _)) = existing_by_index.get(&index) {
                child_ids.push(*existing_id);
                continue;
            }

            let mut item_ctx = wf_ctx.clone();
            item_ctx.set_current_item(item.clone(), index);

            let parent_execution_config = Self::restore_secret_entity(
                &mut *conn,
                encryption_key,
                ENTITY_EXECUTION_CONFIG,
                parent_execution.id,
                parent_execution.config.clone().unwrap_or(JsonValue::Null),
            )
            .await?;
            let rendered_input = Self::render_workflow_task_input(
                parent_execution,
                &parent_execution_config,
                task_node,
                task_action,
                &item_ctx,
            )?;

            let task_config: Option<JsonValue> = if rendered_input.value.is_object()
                && !rendered_input.value.as_object().unwrap().is_empty()
            {
                Some(rendered_input.value.clone())
            } else {
                parent_execution.config.clone()
            };

            let permission_set_refs =
                Self::workflow_task_permission_set_refs(task_node, task_action, &item_ctx)?;
            let (worker_selector, worker_tolerations, worker_affinity) =
                Self::workflow_task_placement_overrides(task_node, &item_ctx)?;
            let item_timeout_seconds =
                Self::resolve_workflow_task_timeout(task_node, &item_ctx)?;

            let workflow_task = WorkflowTaskMetadata {
                workflow_execution: *workflow_execution_id,
                task_name: task_node.name.clone(),
                triggered_by: triggered_by.map(String::from),
                task_index: Some(index as i32),
                task_batch: None,
                retry_count: 0,
                max_retries: task_node
                    .retry
                    .as_ref()
                    .map(|r| r.count as i32)
                    .unwrap_or(0),
                next_retry_at: None,
                timeout_seconds: item_timeout_seconds.map(|seconds| seconds as i32),
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            };

            let child_execution_result =
                ExecutionRepository::create_workflow_task_if_absent_with_conn(
                    &mut *conn,
                    CreateExecutionInput {
                        action: Some(task_action.id),
                        action_ref: action_ref.to_string(),
                        config: task_config,
                        env_vars: parent_execution.env_vars.clone(),
                        parent: Some(parent_execution.id),
                        enforcement: parent_execution.enforcement,
                        executor: parent_execution.executor,
                        permission_set_refs,
                        artifact_retention_policy: parent_execution
                            .artifact_retention_policy
                            .or(task_action.artifact_retention_policy),
                        artifact_retention_limit: parent_execution
                            .artifact_retention_limit
                            .or(task_action.artifact_retention_limit),
                        worker_selector,
                        worker_tolerations,
                        worker_affinity,
                        worker: None,
                        status: ExecutionStatus::Requested,
                        trace_tag: Self::workflow_task_trace_tag(
                            task_node,
                            parent_execution,
                            &item_ctx,
                        )?,
                        timeout_seconds: Some(
                            item_timeout_seconds
                                .map(|seconds| seconds as i32)
                                .or(task_action.timeout_seconds)
                                .unwrap_or(
                                    attune_common::config::app_default_execution_timeout_seconds()
                                        as i32,
                                ),
                        ),
                        result: None,
                        workflow_task: Some(workflow_task),
                    },
                    *workflow_execution_id,
                    &task_node.name,
                    Some(index as i32),
                )
                .await?;
            let child_execution = child_execution_result.execution;
            if child_execution_result.created {
                Self::persist_execution_config_secrets_with_conn(
                    &mut *conn,
                    encryption_key,
                    child_execution.id,
                    rendered_input.secret_inputs,
                )
                .await?;
            }

            if child_execution_result.created {
                info!(
                    "Created with_items child execution {} for task '{}' item {} \
                     (action '{}', workflow_execution {})",
                    child_execution.id, task_node.name, index, action_ref, workflow_execution_id
                );
            } else {
                debug!(
                    "Reusing with_items child execution {} for task '{}' item {} \
                     (workflow_execution {})",
                    child_execution.id, task_node.name, index, workflow_execution_id
                );
            }

            child_ids.push(child_execution.id);
        }

        for &child_id in child_ids.iter().take(dispatch_count) {
            let child = ExecutionRepository::find_by_id(&mut *conn, child_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Execution {} not found", child_id))?;

            if child.status == ExecutionStatus::Requested {
                Self::publish_execution_requested_with_conn(
                    &mut *conn,
                    child_id,
                    task_action.id,
                    action_ref,
                    parent_execution,
                    pending_messages,
                )
                .await?;
            }
        }

        info!(
            "Dispatched {} of {} with_items child executions for task '{}'",
            dispatch_count, total, task_node.name
        );

        Ok(())
    }

    /// Publish an `ExecutionRequested` message for an existing execution row.
    ///
    /// Used to MQ-publish child executions that were created in the database
    /// but not yet dispatched (deferred by concurrency limiting).
    async fn publish_execution_requested(
        pool: &PgPool,
        publisher: &Publisher,
        execution_id: i64,
        action_id: i64,
        action_ref: &str,
        parent_execution: &Execution,
    ) -> Result<()> {
        let child = ExecutionRepository::find_by_id(pool, execution_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Execution {} not found", execution_id))?;

        let payload = ExecutionRequestedPayload {
            execution_id: child.id,
            action_id: Some(action_id),
            action_ref: action_ref.to_string(),
            parent_id: Some(parent_execution.id),
            enforcement_id: parent_execution.enforcement,
            config: child.config.clone(),
        };

        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("executor-scheduler");

        publisher.publish_envelope(&envelope).await?;

        debug!(
            "Published deferred ExecutionRequested for child execution {}",
            execution_id
        );

        Ok(())
    }

    async fn publish_execution_requested_payload(
        publisher: &Publisher,
        pending: PendingExecutionRequested,
    ) -> Result<()> {
        let payload = ExecutionRequestedPayload {
            execution_id: pending.execution_id,
            action_id: Some(pending.action_id),
            action_ref: pending.action_ref,
            parent_id: Some(pending.parent_id),
            enforcement_id: pending.enforcement_id,
            config: pending.config,
        };

        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("executor-scheduler");

        publisher.publish_envelope(&envelope).await?;

        debug!(
            "Published deferred ExecutionRequested for child execution {}",
            envelope.payload.execution_id
        );

        Ok(())
    }

    async fn publish_execution_completed_payload(
        publisher: &Publisher,
        pending: PendingExecutionCompleted,
    ) -> Result<()> {
        let envelope = MessageEnvelope::new(
            MessageType::ExecutionCompleted,
            ExecutionCompletedPayload {
                execution_id: pending.execution_id,
                action_id: pending.action_id,
                action_ref: pending.action_ref,
                status: match pending.status {
                    ExecutionStatus::Completed => "completed".to_string(),
                    ExecutionStatus::Failed => "failed".to_string(),
                    ExecutionStatus::Timeout => "timeout".to_string(),
                    ExecutionStatus::Cancelled => "cancelled".to_string(),
                    other => format!("{:?}", other).to_lowercase(),
                },
                result: pending.result,
                completed_at: pending.completed_at,
            },
        )
        .with_source("executor-scheduler");

        publisher.publish_envelope(&envelope).await?;

        debug!(
            "Published synthetic ExecutionCompleted for workflow execution {}",
            envelope.payload.execution_id
        );

        Ok(())
    }

    async fn publish_execution_requested_with_conn(
        conn: &mut PgConnection,
        execution_id: i64,
        action_id: i64,
        action_ref: &str,
        parent_execution: &Execution,
        pending_messages: &mut Vec<PendingExecutionRequested>,
    ) -> Result<()> {
        let child = ExecutionRepository::find_by_id(&mut *conn, execution_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Execution {} not found", execution_id))?;

        pending_messages.push(PendingExecutionRequested {
            execution_id: child.id,
            action_id,
            action_ref: action_ref.to_string(),
            parent_id: parent_execution.id,
            enforcement_id: parent_execution.enforcement,
            config: child.config.clone(),
        });

        Ok(())
    }

    /// Publish the next `Requested`-status with_items siblings to fill freed
    /// concurrency slots.
    ///
    /// When a with_items child completes, this method queries for siblings
    /// that are still at `Requested` status (created in DB but never
    /// published to MQ) and publishes enough of them to restore the
    /// concurrency window.
    ///
    /// Returns the number of items dispatched.
    #[allow(dead_code)]
    async fn publish_pending_with_items_children(
        pool: &PgPool,
        publisher: &Publisher,
        parent_execution: &Execution,
        workflow_execution_id: i64,
        task_name: &str,
        slots: usize,
    ) -> Result<usize> {
        if slots == 0 {
            return Ok(0);
        }

        // Find siblings still at Requested status, ordered by task_index.
        let pending_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT id, COALESCE(action, 0) as action_id \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND workflow_task->>'task_name' = $2 \
               AND status = 'requested' \
             ORDER BY (workflow_task->>'task_index')::int ASC \
             LIMIT $3",
        )
        .bind(workflow_execution_id.to_string())
        .bind(task_name)
        .bind(slots as i64)
        .fetch_all(pool)
        .await?;

        let mut dispatched = 0usize;
        for (child_id, action_id) in &pending_rows {
            // Read action_ref from the execution row
            let child = match ExecutionRepository::find_by_id(pool, *child_id).await? {
                Some(c) => c,
                None => continue,
            };

            if let Err(e) = Self::publish_execution_requested(
                pool,
                publisher,
                *child_id,
                *action_id,
                &child.action_ref,
                parent_execution,
            )
            .await
            {
                error!(
                    "Failed to publish pending with_items child {}: {}",
                    child_id, e
                );
            } else {
                dispatched += 1;
            }
        }

        if dispatched > 0 {
            info!(
                "Published {} pending with_items children for task '{}' \
                 (workflow_execution {})",
                dispatched, task_name, workflow_execution_id
            );
        }

        Ok(dispatched)
    }

    async fn publish_pending_with_items_children_with_conn(
        conn: &mut PgConnection,
        parent_execution: &Execution,
        workflow_execution_id: i64,
        task_name: &str,
        slots: usize,
        pending_messages: &mut Vec<PendingExecutionRequested>,
    ) -> Result<usize> {
        if slots == 0 {
            return Ok(0);
        }

        let pending_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT id, COALESCE(action, 0) as action_id \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND workflow_task->>'task_name' = $2 \
               AND status = 'requested' \
             ORDER BY (workflow_task->>'task_index')::int ASC \
             LIMIT $3",
        )
        .bind(workflow_execution_id.to_string())
        .bind(task_name)
        .bind(slots as i64)
        .fetch_all(&mut *conn)
        .await?;

        let mut dispatched = 0usize;
        for (child_id, action_id) in &pending_rows {
            let child = match ExecutionRepository::find_by_id(&mut *conn, *child_id).await? {
                Some(c) => c,
                None => continue,
            };

            if let Err(e) = Self::publish_execution_requested_with_conn(
                &mut *conn,
                *child_id,
                *action_id,
                &child.action_ref,
                parent_execution,
                pending_messages,
            )
            .await
            {
                error!(
                    "Failed to publish pending with_items child {}: {}",
                    child_id, e
                );
            } else {
                dispatched += 1;
            }
        }

        if dispatched > 0 {
            info!(
                "Published {} pending with_items children for task '{}' \
                 (workflow_execution {})",
                dispatched, task_name, workflow_execution_id
            );
        }

        Ok(dispatched)
    }

    /// Advance a workflow after a child task completes. Called from the
    /// completion listener when it detects that the completed execution has
    /// `workflow_task` metadata.
    ///
    /// This evaluates transitions from the completed task, schedules successor
    /// tasks, and completes the workflow when all tasks are done.
    pub(crate) async fn advance_workflow(
        pool: &PgPool,
        publisher: &Publisher,
        round_robin_counter: &AtomicUsize,
        artifacts_dir: &str,
        encryption_key: Option<&str>,
        execution: &Execution,
        metadata_caches: &SchedulerMetadataCaches,
    ) -> Result<()> {
        let workflow_task = match execution.workflow_task.as_ref() {
            Some(workflow_task) => workflow_task.clone(),
            None => return Ok(()),
        };
        let workflow_execution_id = workflow_task.workflow_execution;

        // Look up the parent execution id and its action_ref so we can write
        // to the per-action workflow log.
        let parent_info: Option<(i64, String)> = sqlx::query_as(
            "SELECT we.execution, e.action_ref \
             FROM workflow_execution we \
             JOIN execution e ON e.id = we.execution \
             WHERE we.id = $1",
        )
        .bind(workflow_execution_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        let logger = parent_info.as_ref().map(|(pid, action_ref)| {
            WorkflowLogger::new(pool.clone(), artifacts_dir, action_ref.as_str(), *pid)
        });

        let task_outcome_label = match execution.status {
            ExecutionStatus::Completed => "Succeeded",
            ExecutionStatus::Timeout => "TimedOut",
            ExecutionStatus::Cancelled => "Cancelled",
            _ => "Failed",
        };
        if let Some(l) = logger.as_ref() {
            let item_suffix = workflow_task
                .task_index
                .map(|idx| format!(" (item {})", idx))
                .unwrap_or_default();
            l.info(format!(
                "Task '{}'{} {}",
                workflow_task.task_name, item_suffix, task_outcome_label
            ))
            .await;
        }

        let mut lock_conn = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(workflow_execution_id)
            .execute(&mut *lock_conn)
            .await?;

        let result = async {
            sqlx::query("BEGIN").execute(&mut *lock_conn).await?;

            let advance_result = Self::advance_workflow_serialized(
                &mut lock_conn,
                round_robin_counter,
                encryption_key,
                execution,
                metadata_caches,
            )
            .await;

            match advance_result {
                Ok(outcome) => {
                    sqlx::query("COMMIT").execute(&mut *lock_conn).await?;

                    if let Some(l) = logger.as_ref() {
                        for pending in &outcome.execution_requests {
                            // We avoid logging task inputs; only metadata.
                            // The pending message references a child execution
                            // we just created — fetch its workflow_task name.
                            if let Ok(Some(child)) =
                                ExecutionRepository::find_by_id(pool, pending.execution_id).await
                            {
                                if let Some(child_wt) = child.workflow_task.as_ref() {
                                    let item_suffix = child_wt
                                        .task_index
                                        .map(|idx| format!(" (item {})", idx))
                                        .unwrap_or_default();
                                    let trigger_suffix = child_wt
                                        .triggered_by
                                        .as_deref()
                                        .map(|t| format!(", triggered by '{}'", t))
                                        .unwrap_or_default();
                                    l.info(format!(
                                        "Dispatched task '{}'{}{}",
                                        child_wt.task_name, item_suffix, trigger_suffix
                                    ))
                                    .await;
                                }
                            }
                        }
                    }

                    for pending in outcome.execution_requests {
                        Self::publish_execution_requested_payload(publisher, pending).await?;
                    }
                    for completed in outcome.completed_children {
                        Self::publish_execution_completed_payload(publisher, completed).await?;
                    }

                    if let Some(completed) = outcome.completed_execution {
                        Self::publish_execution_completed_payload(publisher, completed).await?;
                    }

                    Ok(())
                }
                Err(err) => {
                    let rollback_result = sqlx::query("ROLLBACK").execute(&mut *lock_conn).await;
                    if let Err(rollback_err) = rollback_result {
                        error!(
                            "Failed to roll back workflow_execution {} advancement transaction: {}",
                            workflow_execution_id, rollback_err
                        );
                    }
                    Err(err)
                }
            }
        }
        .await;
        let unlock_result = sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(workflow_execution_id)
            .execute(&mut *lock_conn)
            .await;

        result?;
        unlock_result?;

        // After successful advancement, check whether the workflow
        // transitioned to a terminal state and log it.
        if let Some(l) = logger.as_ref() {
            if let Ok(Some(wf_exec)) =
                WorkflowExecutionRepository::find_by_id(pool, workflow_execution_id).await
            {
                match wf_exec.status {
                    ExecutionStatus::Completed => l.info("Workflow Completed").await,
                    ExecutionStatus::Failed => l.error("Workflow Failed").await,
                    ExecutionStatus::Cancelled => l.warn("Workflow Cancelled").await,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    async fn advance_workflow_serialized(
        conn: &mut PgConnection,
        round_robin_counter: &AtomicUsize,
        encryption_key: Option<&str>,
        execution: &Execution,
        metadata_caches: &SchedulerMetadataCaches,
    ) -> Result<WorkflowAdvanceOutcome> {
        let workflow_task = match &execution.workflow_task {
            Some(wt) => wt,
            None => return Ok(WorkflowAdvanceOutcome::default()), // Not a workflow task, nothing to do
        };

        let workflow_execution_id = workflow_task.workflow_execution;
        let task_name = &workflow_task.task_name;
        let mut task_succeeded = execution.status == ExecutionStatus::Completed;
        let mut task_timed_out = execution.status == ExecutionStatus::Timeout;

        let mut task_outcome = if task_succeeded {
            TaskOutcome::Succeeded
        } else if task_timed_out {
            TaskOutcome::TimedOut
        } else {
            TaskOutcome::Failed
        };

        info!(
            "Advancing workflow_execution {} after task '{}' {:?} (execution {})",
            workflow_execution_id, task_name, task_outcome, execution.id,
        );

        // Load the workflow execution record
        let workflow_execution =
            WorkflowExecutionRepository::find_by_id_for_update(&mut *conn, workflow_execution_id)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!("Workflow execution {} not found", workflow_execution_id)
                })?;

        // Already fully terminal (Completed / Failed) — nothing to do
        if matches!(
            workflow_execution.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed
        ) {
            debug!(
                "Workflow execution {} already in terminal state {:?}, skipping advance",
                workflow_execution_id, workflow_execution.status
            );
            return Ok(WorkflowAdvanceOutcome::default());
        }

        let mut pending_messages = Vec::new();
        let mut pending_completed_children = Vec::new();
        let mut pending_completed_execution = None;

        let parent_execution =
            ExecutionRepository::find_by_id(&mut *conn, workflow_execution.execution)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Parent execution {} not found for workflow_execution {}",
                        workflow_execution.execution,
                        workflow_execution_id
                    )
                })?;

        // Cancellation must be a hard stop for workflow orchestration. Once
        // either the workflow record, the parent execution, or the completed
        // child itself is in a cancellation state, do not evaluate transitions,
        // release more with_items siblings, or dispatch any successor tasks.
        if Self::should_halt_workflow_advancement(
            workflow_execution.status,
            parent_execution.status,
            execution.status,
        ) {
            if let Some(iteration) =
                WorkflowCacheIterationRepository::find_by_workflow_task_for_update(
                    &mut *conn,
                    workflow_execution_id,
                    task_name,
                )
                .await?
            {
                if iteration.state == WorkflowCacheIterationState::Scanning {
                    WorkflowCacheIterationRepository::mark_terminal(
                        &mut *conn,
                        iteration.id,
                        WorkflowCacheIterationState::Cancelled,
                        Some("workflow cancellation stopped cache iteration"),
                    )
                    .await?;
                }
            }
            if workflow_execution.status == ExecutionStatus::Cancelled {
                let running = Self::count_running_workflow_children_with_conn(
                    &mut *conn,
                    workflow_execution_id,
                    &workflow_execution.completed_tasks,
                    &workflow_execution.failed_tasks,
                )
                .await?;

                if running == 0 {
                    info!(
                        "Cancelled workflow_execution {} has no more running children, \
                         finalizing parent execution {} as Cancelled",
                        workflow_execution_id, workflow_execution.execution
                    );
                    Self::finalize_cancelled_workflow_with_conn(
                        &mut *conn,
                        workflow_execution.execution,
                        workflow_execution_id,
                    )
                    .await?;
                } else {
                    debug!(
                        "Workflow_execution {} is cancelling/cancelled with {} running children, \
                         skipping advancement",
                        workflow_execution_id, running
                    );
                }
            } else {
                debug!(
                    "Workflow_execution {} advancement halted due to cancellation state \
                     (workflow: {:?}, parent: {:?}, child: {:?})",
                    workflow_execution_id,
                    workflow_execution.status,
                    parent_execution.status,
                    execution.status
                );
            }

            return Ok(WorkflowAdvanceOutcome {
                execution_requests: pending_messages,
                completed_children: pending_completed_children,
                completed_execution: None,
            });
        }

        // Load the workflow definition so we can apply param_schema defaults
        let workflow_def = if let Some(cached) = metadata_caches
            .cached_workflow_definition_by_id(workflow_execution.workflow_def)
            .await
        {
            cached
        } else {
            let workflow_def = WorkflowDefinitionRepository::find_by_id(
                &mut *conn,
                workflow_execution.workflow_def,
            )
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Workflow definition {} not found for workflow_execution {}",
                    workflow_execution.workflow_def,
                    workflow_execution_id
                )
            })?;
            metadata_caches
                .cache_workflow_definition(&workflow_def)
                .await;
            workflow_def
        };

        // Rebuild the task graph from the stored JSON
        let graph: TaskGraph = serde_json::from_value(workflow_execution.task_graph.clone())
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to deserialize task graph for workflow_execution {}: {}",
                    workflow_execution_id,
                    e
                )
            })?;

        // Update completed/failed task lists
        let mut completed_tasks: Vec<String> = workflow_execution.completed_tasks.clone();
        let mut failed_tasks: Vec<String> = workflow_execution.failed_tasks.clone();

        // For with_items tasks, only mark completed/failed when ALL items
        // for this task are done (no more running children with the same
        // task_name).
        let is_cache_iteration = graph
            .get_task(task_name)
            .is_some_and(|node| node.iterate_cache.is_some());
        let is_with_items = workflow_task.task_index.is_some() && !is_cache_iteration;
        if is_with_items {
            // ---------------------------------------------------------
            // Concurrency: publish next Requested-status sibling(s) to
            // fill the slot freed by this completion.
            // ---------------------------------------------------------
            let parent_for_pending =
                ExecutionRepository::find_by_id(&mut *conn, workflow_execution.execution)
                    .await?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Parent execution {} not found for workflow_execution {}",
                            workflow_execution.execution,
                            workflow_execution_id
                        )
                    })?;

            // Count siblings that are actively in-flight (Scheduling,
            // Scheduled, or Running — NOT Requested, which means "created
            // but not yet published to MQ").
            let in_flight_count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) \
                 FROM execution \
                 WHERE workflow_task->>'workflow_execution' = $1::text \
                   AND workflow_task->>'task_name' = $2 \
                   AND status IN ('scheduling', 'scheduled', 'running') \
                   AND id != $3",
            )
            .bind(workflow_execution_id.to_string())
            .bind(task_name)
            .bind(execution.id)
            .fetch_one(&mut *conn)
            .await?;

            // Determine the concurrency limit from the task graph
            let concurrency_limit = graph
                .get_task(task_name)
                .and_then(|n| n.concurrency)
                .unwrap_or(1);

            let free_slots = concurrency_limit
                .saturating_sub(in_flight_count.0 as usize)
                .min(1);

            if free_slots > 0 {
                if let Err(e) = Self::publish_pending_with_items_children_with_conn(
                    &mut *conn,
                    &parent_for_pending,
                    workflow_execution_id,
                    task_name,
                    free_slots,
                    &mut pending_messages,
                )
                .await
                {
                    error!(
                        "Failed to publish pending with_items for task '{}': {}",
                        task_name, e
                    );
                }
            }

            // Count how many siblings are NOT in a terminal state
            // (Requested items are pending, in-flight items are working).
            let siblings_remaining: Vec<(String,)> = sqlx::query_as(
                "SELECT workflow_task->>'task_name' as task_name \
                 FROM execution \
                 WHERE workflow_task->>'workflow_execution' = $1::text \
                   AND workflow_task->>'task_name' = $2 \
                   AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled') \
                   AND id != $3",
            )
            .bind(workflow_execution_id.to_string())
            .bind(task_name)
            .bind(execution.id)
            .fetch_all(&mut *conn)
            .await?;

            if !siblings_remaining.is_empty() {
                debug!(
                    "with_items task '{}' item {} done, but {} siblings remaining — \
                     not advancing yet",
                    task_name,
                    workflow_task.task_index.unwrap_or(-1),
                    siblings_remaining.len(),
                );
                return Ok(WorkflowAdvanceOutcome {
                    execution_requests: pending_messages,
                    completed_children: pending_completed_children,
                    completed_execution: None,
                });
            }

            // ---------------------------------------------------------
            // Race-condition guard: when multiple with_items children
            // complete nearly simultaneously, the worker updates their
            // DB status to Completed *before* the completion MQ message
            // is processed.  This means several advance_workflow calls
            // (processed sequentially by the completion listener) can
            // each see "0 siblings remaining" and fall through to
            // transition evaluation, dispatching successor tasks
            // multiple times.
            //
            // To prevent this we re-check the *persisted*
            // completed/failed task lists that were loaded from the
            // workflow_execution record at the top of this function.
            // If `task_name` is already present, a previous
            // advance_workflow invocation already handled the final
            // completion of this with_items task and dispatched its
            // successors — we can safely return.
            // ---------------------------------------------------------
            if workflow_execution
                .completed_tasks
                .contains(&task_name.to_string())
                || workflow_execution
                    .failed_tasks
                    .contains(&task_name.to_string())
            {
                debug!(
                    "with_items task '{}' already in persisted completed/failed list — \
                     another advance_workflow call already handled final completion, skipping",
                    task_name,
                );
                return Ok(WorkflowAdvanceOutcome {
                    execution_requests: pending_messages,
                    completed_children: pending_completed_children,
                    completed_execution: None,
                });
            }

            // All items done — check if any failed
            let any_failed: Vec<(i64,)> = sqlx::query_as(
                "SELECT id \
                 FROM execution \
                 WHERE workflow_task->>'workflow_execution' = $1::text \
                   AND workflow_task->>'task_name' = $2 \
                   AND status IN ('failed', 'timeout') \
                 LIMIT 1",
            )
            .bind(workflow_execution_id.to_string())
            .bind(task_name)
            .fetch_all(&mut *conn)
            .await?;

            if any_failed.is_empty() {
                if !completed_tasks.contains(task_name) {
                    completed_tasks.push(task_name.clone());
                }
            } else if !failed_tasks.contains(task_name) {
                failed_tasks.push(task_name.clone());
            }
        } else {
            // Normal (non-with_items) task
            if task_succeeded {
                if !completed_tasks.contains(task_name) {
                    completed_tasks.push(task_name.clone());
                }
            } else if !failed_tasks.contains(task_name) {
                failed_tasks.push(task_name.clone());
            }
        }

        let restored_parent_config = Self::restore_secret_entity(
            &mut *conn,
            encryption_key,
            ENTITY_EXECUTION_CONFIG,
            parent_execution.id,
            parent_execution.config.clone().unwrap_or(JsonValue::Null),
        )
        .await?;
        let child_executions = if is_cache_iteration {
            ExecutionRepository::find_by_parent_excluding_workflow_task(
                &mut *conn,
                parent_execution.id,
                workflow_execution_id,
                task_name,
            )
            .await?
        } else {
            ExecutionRepository::find_by_parent(&mut *conn, parent_execution.id).await?
        };
        let mut task_results_map: HashMap<String, JsonValue> = HashMap::new();
        for child in &child_executions {
            if let Some(ref wt) = child.workflow_task {
                if wt.workflow_execution == workflow_execution_id
                    && matches!(
                        child.status,
                        ExecutionStatus::Completed
                            | ExecutionStatus::Failed
                            | ExecutionStatus::Timeout
                    )
                {
                    let result_val = Self::restore_secret_entity(
                        &mut *conn,
                        encryption_key,
                        ENTITY_EXECUTION_RESULT,
                        child.id,
                        child.result.clone().unwrap_or(serde_json::json!({})),
                    )
                    .await?;
                    task_results_map
                        .insert(wt.task_name.clone(), workflow_result_view(&result_val));
                }
            }
        }

        reconcile_authoritative_task_statuses(
            &mut completed_tasks,
            &mut failed_tasks,
            child_executions.iter().filter_map(|child| {
                child.workflow_task.as_ref().and_then(|wt| {
                    (wt.workflow_execution == workflow_execution_id).then(|| {
                        (
                            wt.task_name.clone(),
                            wt.task_index,
                            child.status,
                            wt.triggered_by.clone(),
                            wt.retry_count,
                        )
                    })
                })
            }),
        );

        // -----------------------------------------------------------------
        // Rebuild the WorkflowContext from persisted state + completed task
        // results so that successor task inputs can be rendered.
        // -----------------------------------------------------------------
        let workflow_params = extract_workflow_params(&Some(restored_parent_config));
        let workflow_params = apply_param_defaults(workflow_params, &workflow_def.param_schema);

        let mut wf_ctx = WorkflowContext::rebuild(
            workflow_params,
            &workflow_execution.variables,
            task_results_map,
        );
        Self::mark_workflow_parameter_secret_sources(&wf_ctx, &parent_execution);
        Self::mark_workflow_task_result_secret_sources(
            &wf_ctx,
            &child_executions,
            workflow_execution_id,
        );

        // Set the just-completed task's outcome so that `result()`,
        // `succeeded()`, `failed()` resolve correctly for publish and
        // transition conditions.
        let completed_result = Self::restore_secret_entity(
            &mut *conn,
            encryption_key,
            ENTITY_EXECUTION_RESULT,
            execution.id,
            execution.result.clone().unwrap_or(serde_json::json!({})),
        )
        .await?;
        let completed_result = workflow_result_view(&completed_result);
        let completed_secret_paths = workflow_result_secret_paths(
            &execution.result.clone().unwrap_or(serde_json::json!({})),
        );
        wf_ctx.set_last_task_outcome_with_secret_paths(
            completed_result.clone(),
            task_outcome,
            &completed_secret_paths,
            |path| SecretSource::ExecutionResult {
                execution_id: execution.id,
                path: path.clone(),
            },
        );

        if is_cache_iteration {
            if workflow_execution.completed_tasks.contains(task_name)
                || workflow_execution.failed_tasks.contains(task_name)
            {
                return Ok(WorkflowAdvanceOutcome {
                    execution_requests: pending_messages,
                    completed_children: pending_completed_children,
                    completed_execution: None,
                });
            }
            let task_node = graph
                .get_task(task_name)
                .ok_or_else(|| anyhow::anyhow!("Cache iteration task '{}' not found", task_name))?;
            let action_ref = task_node.action.as_deref().ok_or_else(|| {
                anyhow::anyhow!("Cache iteration task '{}' has no action", task_name)
            })?;
            let mut iteration = WorkflowCacheIterationRepository::find_by_workflow_task_for_update(
                &mut *conn,
                workflow_execution_id,
                task_name,
            )
            .await?;
            if iteration.is_some() {
                let task_action = ActionRepository::find_by_ref(&mut *conn, action_ref)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Action '{}' not found", action_ref))?;
                Self::dispatch_cache_iteration_task_with_conn(
                    &mut *conn,
                    &parent_execution,
                    &workflow_execution_id,
                    task_node,
                    &task_action,
                    action_ref,
                    &wf_ctx,
                    encryption_key,
                    workflow_task.triggered_by.as_deref(),
                    &mut pending_messages,
                    &mut pending_completed_children,
                )
                .await?;
                iteration = WorkflowCacheIterationRepository::find_by_workflow_task_for_update(
                    &mut *conn,
                    workflow_execution_id,
                    task_name,
                )
                .await?;
            }

            completed_tasks.retain(|name| name != task_name);
            failed_tasks.retain(|name| name != task_name);
            if let Some(iteration) = iteration {
                if iteration.state == WorkflowCacheIterationState::Scanning {
                    return Ok(WorkflowAdvanceOutcome {
                        execution_requests: pending_messages,
                        completed_children: pending_completed_children,
                        completed_execution: None,
                    });
                }
                let attempt_summary = ExecutionRepository::summarize_workflow_task_latest_attempts(
                    &mut *conn,
                    workflow_execution_id,
                    task_name,
                )
                .await?;
                if attempt_summary.in_flight > 0 {
                    return Ok(WorkflowAdvanceOutcome {
                        execution_requests: pending_messages,
                        completed_children: pending_completed_children,
                        completed_execution: None,
                    });
                }

                match iteration.state {
                    WorkflowCacheIterationState::Completed => {
                        completed_tasks.push(task_name.clone());
                        task_succeeded = true;
                        task_timed_out = false;
                        task_outcome = TaskOutcome::Succeeded;
                    }
                    WorkflowCacheIterationState::Failed
                    | WorkflowCacheIterationState::Cancelled => {
                        failed_tasks.push(task_name.clone());
                        task_succeeded = false;
                        task_timed_out = false;
                        task_outcome = TaskOutcome::Failed;
                    }
                    WorkflowCacheIterationState::Scanning => unreachable!(),
                }
            } else {
                // A synthetic failed child without an iteration row represents
                // a logical initialization failure and must drive failed().
                task_outcome = cache_iteration_outcome_without_state(execution.status)?;
                if !failed_tasks.contains(task_name) {
                    failed_tasks.push(task_name.clone());
                }
                task_succeeded = false;
                task_timed_out = false;
            }
            wf_ctx.set_last_task_outcome(completed_result.clone(), task_outcome);
        }

        // -----------------------------------------------------------------
        // Process transitions: evaluate conditions, process publish
        // directives, collect successor tasks.
        // -----------------------------------------------------------------
        let mut tasks_to_schedule: Vec<String> = Vec::new();
        let mut deferred_join_tasks: Vec<String> = Vec::new();

        if let Some(completed_task_node) = graph.get_task(task_name) {
            for transition in &completed_task_node.transitions {
                let should_fire = match transition.kind() {
                    crate::workflow::graph::TransitionKind::Succeeded => task_succeeded,
                    crate::workflow::graph::TransitionKind::Failed => {
                        !task_succeeded && !task_timed_out
                    }
                    crate::workflow::graph::TransitionKind::Always => true,
                    crate::workflow::graph::TransitionKind::TimedOut => task_timed_out,
                    crate::workflow::graph::TransitionKind::Custom => {
                        // Try to evaluate via the workflow context
                        if let Some(ref when_expr) = transition.when {
                            match wf_ctx.evaluate_condition(when_expr) {
                                Ok(val) => val,
                                Err(e) => {
                                    warn!(
                                        "Custom condition '{}' evaluation failed: {}. \
                                         Defaulting to fire-on-success.",
                                        when_expr, e
                                    );
                                    task_succeeded
                                }
                            }
                        } else {
                            task_succeeded
                        }
                    }
                };

                if should_fire {
                    // Process publish directives from this transition
                    if !transition.publish.is_empty() {
                        let publish_map: HashMap<String, JsonValue> = transition
                            .publish
                            .iter()
                            .map(|p| (p.name.clone(), p.value.clone()))
                            .collect();
                        if let Err(e) = wf_ctx.publish_from_result(
                            &serde_json::json!({}),
                            &[],
                            Some(&publish_map),
                        ) {
                            warn!("Failed to process publish for task '{}': {}", task_name, e);
                        } else {
                            debug!(
                                "Published {} variables from task '{}' transition",
                                publish_map.len(),
                                task_name
                            );
                        }
                    }

                    for next_task_name in &transition.do_tasks {
                        // Skip tasks that are already completed or failed
                        if completed_tasks.contains(next_task_name)
                            || failed_tasks.contains(next_task_name)
                        {
                            debug!(
                                "Skipping task '{}' — already completed or failed",
                                next_task_name
                            );
                            continue;
                        }

                        // Check join barrier: if the task has a `join` count,
                        // only schedule it when enough predecessors are done.
                        if let Some(next_node) = graph.get_task(next_task_name) {
                            if let Some(join_count) = next_node.join {
                                let inbound_completed = next_node
                                    .inbound_tasks
                                    .iter()
                                    .filter(|t| completed_tasks.contains(*t))
                                    .count();
                                if inbound_completed < join_count {
                                    debug!(
                                        "Task '{}' join barrier not met ({}/{} predecessors done)",
                                        next_task_name, inbound_completed, join_count
                                    );
                                    if !deferred_join_tasks.contains(next_task_name) {
                                        deferred_join_tasks.push(next_task_name.clone());
                                    }
                                    continue;
                                }
                            }
                        }

                        if !tasks_to_schedule.contains(next_task_name) {
                            tasks_to_schedule.push(next_task_name.clone());
                        }
                    }
                }
            }
        }

        if !task_succeeded && !tasks_to_schedule.is_empty() {
            failed_tasks.retain(|failed_task| failed_task != task_name);
            if !completed_tasks.contains(task_name) {
                completed_tasks.push(task_name.clone());
            }
        }

        // Check if any tasks are still running (children of this workflow
        // that haven't completed yet). We query child executions that have
        // workflow_task metadata pointing to our workflow_execution.
        let running_children = Self::count_running_workflow_children_with_conn(
            &mut *conn,
            workflow_execution_id,
            &completed_tasks,
            &failed_tasks,
        )
        .await?;

        // Dispatch successor tasks, passing the updated workflow context
        for next_task_name in &tasks_to_schedule {
            if let Some(task_node) = graph.get_task(next_task_name) {
                if let Err(e) = Self::dispatch_workflow_task_with_conn(
                    &mut *conn,
                    round_robin_counter,
                    &parent_execution,
                    &workflow_execution_id,
                    task_node,
                    &wf_ctx,
                    encryption_key,
                    Some(task_name), // predecessor that triggered this task
                    &mut pending_messages,
                    &mut pending_completed_children,
                )
                .await
                {
                    error!(
                        "Failed to dispatch workflow task '{}': {}",
                        next_task_name, e
                    );
                    if !failed_tasks.contains(next_task_name) {
                        failed_tasks.push(next_task_name.clone());
                    }
                }
            }
        }

        // Determine current executing tasks (for the workflow_execution record)
        let current_tasks: Vec<String> = tasks_to_schedule.clone();

        // Persist updated workflow variables (from publish directives) and
        // completed/failed task lists.
        let updated_variables = wf_ctx.export_variables();
        WorkflowExecutionRepository::update(
            &mut *conn,
            workflow_execution_id,
            attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
                current_tasks: Some(current_tasks),
                completed_tasks: Some(completed_tasks.clone()),
                failed_tasks: Some(failed_tasks.clone()),
                skipped_tasks: None,
                variables: Some(updated_variables),
                status: None, // Updated below if terminal
                error_message: None,
                paused: None,
                pause_reason: None,
            },
        )
        .await?;

        // Check if workflow is complete: no more tasks to schedule and no
        // children still running (excluding the ones we just scheduled).
        let all_done =
            tasks_to_schedule.is_empty() && deferred_join_tasks.is_empty() && running_children == 0;

        if all_done {
            let has_failures = !failed_tasks.is_empty();
            let error_msg = if has_failures {
                Some(format!(
                    "Workflow failed: {} task(s) failed: {}",
                    failed_tasks.len(),
                    failed_tasks.join(", ")
                ))
            } else {
                None
            };

            // Evaluate the workflow's `output_map` (if any) using the
            // current WorkflowContext so the parent execution's `result`
            // surfaces user-defined outputs (e.g., a markdown summary,
            // structured fields composed from task results).
            let output_map_result = if !has_failures {
                build_output_map_result(&workflow_def.definition, &wf_ctx)
            } else {
                None
            };

            let completed_execution = Self::complete_workflow_with_conn(
                &mut *conn,
                parent_execution.id,
                workflow_execution_id,
                !has_failures,
                error_msg.as_deref(),
                output_map_result,
            )
            .await?;
            if completed_execution.parent.is_some() {
                let action_id = completed_execution.action.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Completed nested workflow execution {} has no action id",
                        completed_execution.id
                    )
                })?;
                pending_completed_execution = Some(PendingExecutionCompleted {
                    execution_id: completed_execution.id,
                    action_id,
                    action_ref: completed_execution.action_ref.clone(),
                    status: completed_execution.status,
                    result: completed_execution.result.clone(),
                    completed_at: Utc::now(),
                });
            }
        }

        Ok(WorkflowAdvanceOutcome {
            execution_requests: pending_messages,
            completed_children: pending_completed_children,
            completed_execution: pending_completed_execution,
        })
    }

    /// Count child executions that are still in progress for a workflow.
    #[allow(dead_code)]
    async fn count_running_workflow_children(
        pool: &PgPool,
        workflow_execution_id: i64,
        completed_tasks: &[String],
        failed_tasks: &[String],
    ) -> Result<usize> {
        // Query child executions that reference this workflow_execution and
        // are not yet in a terminal state. We use the workflow_task JSONB
        // field to filter.
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT workflow_task->>'task_name' as task_name \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled')",
        )
        .bind(workflow_execution_id.to_string())
        .fetch_all(pool)
        .await?;

        let count = rows
            .iter()
            .filter(|(tn,)| !completed_tasks.contains(tn) && !failed_tasks.contains(tn))
            .count();

        Ok(count)
    }

    async fn count_running_workflow_children_with_conn(
        conn: &mut PgConnection,
        workflow_execution_id: i64,
        completed_tasks: &[String],
        failed_tasks: &[String],
    ) -> Result<usize> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT workflow_task->>'task_name' as task_name \
             FROM execution \
             WHERE workflow_task->>'workflow_execution' = $1::text \
               AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled')",
        )
        .bind(workflow_execution_id.to_string())
        .fetch_all(&mut *conn)
        .await?;

        let count = rows
            .iter()
            .filter(|(tn,)| !completed_tasks.contains(tn) && !failed_tasks.contains(tn))
            .count();

        Ok(count)
    }

    fn should_halt_workflow_advancement(
        workflow_status: ExecutionStatus,
        parent_status: ExecutionStatus,
        child_status: ExecutionStatus,
    ) -> bool {
        matches!(
            workflow_status,
            ExecutionStatus::Canceling | ExecutionStatus::Cancelled
        ) || matches!(
            parent_status,
            ExecutionStatus::Canceling | ExecutionStatus::Cancelled
        ) || matches!(
            child_status,
            ExecutionStatus::Canceling | ExecutionStatus::Cancelled
        )
    }

    /// Finalize a cancelled workflow by updating the parent `execution` record
    /// to `Cancelled`.  The `workflow_execution` record is already `Cancelled`
    /// (set by `cancel_workflow_children`); this only touches the parent.
    #[allow(dead_code)]
    async fn finalize_cancelled_workflow(
        pool: &PgPool,
        parent_execution_id: i64,
        workflow_execution_id: i64,
    ) -> Result<()> {
        info!(
            "Finalizing cancelled workflow: parent execution {} (workflow_execution {})",
            parent_execution_id, workflow_execution_id
        );

        let update = UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            result: Some(serde_json::json!({
                "error": "Workflow cancelled",
                "succeeded": false,
            })),
            ..Default::default()
        };
        ExecutionRepository::update(pool, parent_execution_id, update).await?;

        Ok(())
    }

    async fn finalize_cancelled_workflow_with_conn(
        conn: &mut PgConnection,
        parent_execution_id: i64,
        workflow_execution_id: i64,
    ) -> Result<()> {
        info!(
            "Finalizing cancelled workflow: parent execution {} (workflow_execution {})",
            parent_execution_id, workflow_execution_id
        );

        let update = UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            result: Some(serde_json::json!({
                "error": "Workflow cancelled",
                "succeeded": false,
            })),
            ..Default::default()
        };
        ExecutionRepository::update(&mut *conn, parent_execution_id, update).await?;

        Ok(())
    }

    /// Mark a workflow as completed (success or failure) and update both the
    /// `workflow_execution` and parent `execution` records.
    async fn complete_workflow(
        pool: &PgPool,
        parent_execution_id: i64,
        workflow_execution_id: i64,
        success: bool,
        error_message: Option<&str>,
        result_override: Option<JsonValue>,
    ) -> Result<()> {
        let status = if success {
            ExecutionStatus::Completed
        } else {
            ExecutionStatus::Failed
        };

        info!(
            "Completing workflow_execution {} with status {:?} (parent execution {})",
            workflow_execution_id, status, parent_execution_id
        );

        // Update workflow_execution status
        WorkflowExecutionRepository::update(
            pool,
            workflow_execution_id,
            attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
                current_tasks: Some(vec![]),
                completed_tasks: None,
                failed_tasks: None,
                skipped_tasks: None,
                variables: None,
                status: Some(status),
                error_message: error_message.map(|s| s.to_string()),
                paused: None,
                pause_reason: None,
            },
        )
        .await?;

        // Update parent execution
        let parent = ExecutionRepository::find_by_id(pool, parent_execution_id).await?;
        if let Some(mut parent) = parent {
            parent.status = status;
            parent.result = Some(build_workflow_result_payload(
                success,
                error_message,
                result_override,
            ));
            ExecutionRepository::update(pool, parent.id, parent.into()).await?;
        }

        Ok(())
    }

    async fn complete_workflow_with_conn(
        conn: &mut PgConnection,
        parent_execution_id: i64,
        workflow_execution_id: i64,
        success: bool,
        error_message: Option<&str>,
        result_override: Option<JsonValue>,
    ) -> Result<Execution> {
        let status = if success {
            ExecutionStatus::Completed
        } else {
            ExecutionStatus::Failed
        };

        info!(
            "Completing workflow_execution {} with status {:?} (parent execution {})",
            workflow_execution_id, status, parent_execution_id
        );

        WorkflowExecutionRepository::update(
            &mut *conn,
            workflow_execution_id,
            attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
                current_tasks: Some(vec![]),
                completed_tasks: None,
                failed_tasks: None,
                skipped_tasks: None,
                variables: None,
                status: Some(status),
                error_message: error_message.map(|s| s.to_string()),
                paused: None,
                pause_reason: None,
            },
        )
        .await?;

        let parent = ExecutionRepository::find_by_id(&mut *conn, parent_execution_id).await?;
        if let Some(mut parent) = parent {
            parent.status = status;
            parent.result = Some(build_workflow_result_payload(
                success,
                error_message,
                result_override,
            ));
            return ExecutionRepository::update(&mut *conn, parent.id, parent.into())
                .await
                .map_err(Into::into);
        }

        Err(anyhow::anyhow!(
            "Parent execution {} not found for workflow_execution {}",
            parent_execution_id,
            workflow_execution_id
        ))
    }

    // -----------------------------------------------------------------------
    // Regular action scheduling helpers
    // -----------------------------------------------------------------------

    /// Get the action associated with an execution
    async fn get_action_for_execution(
        pool: &PgPool,
        metadata_caches: &SchedulerMetadataCaches,
        execution: &Execution,
    ) -> Result<Action> {
        let started = Instant::now();
        // Try to get action by ID first
        if let Some(action_id) = execution.action {
            if let Some(action) = metadata_caches.cached_action_by_id(action_id).await {
                debug!(
                    entity = "action",
                    operation = "find_for_execution",
                    caller = "executor.scheduler",
                    cache_hit = true,
                    cache_key = "id",
                    execution_id = execution.id,
                    latency_ms = started.elapsed().as_millis() as u64,
                    "metadata read"
                );
                return Ok(action);
            }
            if let Some(action) = ActionRepository::find_by_id(pool, action_id).await? {
                metadata_caches.cache_action(&action).await;
                debug!(
                    entity = "action",
                    operation = "find_for_execution",
                    caller = "executor.scheduler",
                    cache_hit = false,
                    cache_key = "id",
                    execution_id = execution.id,
                    latency_ms = started.elapsed().as_millis() as u64,
                    "metadata read"
                );
                return Ok(action);
            }
        }

        // Fall back to action_ref
        if let Some(action) = metadata_caches
            .cached_action_by_ref(&execution.action_ref)
            .await
        {
            debug!(
                entity = "action",
                operation = "find_for_execution",
                caller = "executor.scheduler",
                cache_hit = true,
                cache_key = "ref",
                execution_id = execution.id,
                latency_ms = started.elapsed().as_millis() as u64,
                "metadata read"
            );
            return Ok(action);
        }

        let action = ActionRepository::find_by_ref(pool, &execution.action_ref)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Action not found for execution: {}", execution.id))?;
        metadata_caches.cache_action(&action).await;
        debug!(
            entity = "action",
            operation = "find_for_execution",
            caller = "executor.scheduler",
            cache_hit = false,
            cache_key = "ref",
            execution_id = execution.id,
            latency_ms = started.elapsed().as_millis() as u64,
            "metadata read"
        );
        Ok(action)
    }

    /// Select an appropriate worker for the execution
    ///
    /// Uses round-robin selection among compatible, active, and healthy workers
    /// to distribute load evenly across the worker pool.
    #[allow(dead_code)]
    pub async fn select_worker(
        pool: &PgPool,
        action: &Action,
        round_robin_counter: &AtomicUsize,
    ) -> Result<attune_common::models::Worker> {
        Self::select_worker_for_action_execution(pool, action, None, round_robin_counter).await
    }

    async fn select_worker_for_action_execution(
        pool: &PgPool,
        action: &Action,
        execution: Option<&Execution>,
        round_robin_counter: &AtomicUsize,
    ) -> Result<attune_common::models::Worker> {
        let placement = Self::effective_placement(action, execution)?;
        // Get runtime requirements for the action
        let runtime = if let Some(runtime_id) = action.runtime {
            RuntimeRepository::find_by_id(pool, runtime_id).await?
        } else {
            None
        };

        // Find available action workers (role = 'action')
        let workers = WorkerRepository::find_action_workers(pool).await?;

        if workers.is_empty() {
            return Err(anyhow::anyhow!("No action workers available"));
        }

        // Filter workers by runtime compatibility if runtime is specified
        let runtime_compatible_workers: Vec<_> = if let Some(ref runtime) = runtime {
            workers
                .into_iter()
                .filter(|w| Self::worker_supports_runtime(w, runtime))
                .filter(|w| {
                    Self::worker_supports_runtime_constraint(
                        w,
                        runtime,
                        action.runtime_version_constraint.as_deref(),
                    )
                })
                .filter(|w| {
                    Self::worker_supports_required_runtimes(w, &action.required_worker_runtimes)
                })
                .collect()
        } else {
            workers
                .into_iter()
                .filter(|w| {
                    Self::worker_supports_required_runtimes(w, &action.required_worker_runtimes)
                })
                .collect()
        };

        let compatible_workers: Vec<_> = runtime_compatible_workers
            .into_iter()
            .filter(|w| Self::worker_satisfies_placement(w, action, &placement))
            .collect();

        if compatible_workers.is_empty() {
            let runtime_name = runtime.as_ref().map(|r| r.name.as_str()).unwrap_or("any");
            let version_constraint = action
                .runtime_version_constraint
                .as_deref()
                .unwrap_or("none");
            let required_runtimes = if action.required_worker_runtime_constraints().is_empty() {
                "none".to_string()
            } else {
                action
                    .required_worker_runtime_constraints()
                    .into_iter()
                    .map(|(runtime, constraint)| format!("{} {}", runtime, constraint))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(anyhow::anyhow!(
                "No compatible workers found for action: {} (requires runtime: {}, version constraint: {}, required worker runtimes: {}, worker placement: {})",
                action.r#ref,
                runtime_name,
                version_constraint,
                required_runtimes,
                Self::placement_description(&placement),
            ));
        }

        // Filter by worker status (only active workers)
        let active_workers: Vec<_> = compatible_workers
            .into_iter()
            .filter(|w| {
                w.status == Some(attune_common::models::enums::WorkerStatus::Active) && !w.cordoned
            })
            .collect();

        if active_workers.is_empty() {
            return Err(anyhow::anyhow!("No active, uncordoned workers available"));
        }

        // Filter by heartbeat freshness (only workers with recent heartbeats)
        let fresh_workers: Vec<_> = active_workers
            .into_iter()
            .filter(Self::is_worker_heartbeat_fresh)
            .collect();

        if fresh_workers.is_empty() {
            warn!("No workers with fresh heartbeats available. All active workers have stale heartbeats.");
            return Err(anyhow::anyhow!(
                "No workers with fresh heartbeats available (heartbeat older than {} seconds)",
                DEFAULT_HEARTBEAT_INTERVAL * HEARTBEAT_STALENESS_MULTIPLIER
            ));
        }

        let max_preference_score = fresh_workers
            .iter()
            .map(|worker| Self::worker_preference_score(worker, &placement))
            .max()
            .unwrap_or(0);
        let preferred_workers: Vec<_> = fresh_workers
            .into_iter()
            .filter(|worker| {
                Self::worker_preference_score(worker, &placement) == max_preference_score
            })
            .collect();

        // Round-robin selection: distribute executions evenly across the best
        // scoring workers after hard placement constraints are enforced.
        let count = round_robin_counter.fetch_add(1, Ordering::Relaxed);
        let index = count % preferred_workers.len();
        let selected = preferred_workers
            .into_iter()
            .nth(index)
            .expect("Worker list should not be empty");

        info!(
            "Selected worker {} (id={}) via round-robin (index {} of best-scoring workers, placement score {})",
            selected.name, selected.id, index, max_preference_score
        );

        Ok(selected)
    }

    /// Select an appropriate worker for a persisted execution using the same
    /// action lookup and worker selection path as normal scheduling.
    #[allow(dead_code)]
    pub async fn select_worker_for_execution(
        pool: &PgPool,
        execution_id: i64,
        round_robin_counter: &AtomicUsize,
    ) -> Result<attune_common::models::Worker> {
        let execution = ExecutionRepository::find_by_id(pool, execution_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Execution not found: {}", execution_id))?;
        let metadata_caches = SchedulerMetadataCaches::new();
        let action = Self::get_action_for_execution(pool, &metadata_caches, &execution).await?;
        Self::select_worker_for_action_execution(
            pool,
            &action,
            Some(&execution),
            round_robin_counter,
        )
        .await
    }

    /// Select an active, fresh worker that supports the runtimes required to
    /// execute a pack's test suite.
    ///
    /// `required_runtimes` are runtime names/aliases derived from the pack's
    /// test runner config (e.g. `["python"]` for unittest/pytest runners).
    /// When empty, any active worker is acceptable.
    pub(crate) async fn select_worker_for_pack_test(
        pool: &PgPool,
        required_runtimes: &[String],
        round_robin_counter: &AtomicUsize,
    ) -> Result<attune_common::models::Worker> {
        let workers = WorkerRepository::find_action_workers(pool).await?;
        if workers.is_empty() {
            return Err(anyhow::anyhow!("No action workers available"));
        }

        let normalized_required: Vec<String> = required_runtimes
            .iter()
            .map(|name| normalize_runtime_name(name))
            .collect();

        let capable_workers: Vec<_> = workers
            .into_iter()
            .filter(|w| {
                w.status == Some(attune_common::models::enums::WorkerStatus::Active) && !w.cordoned
            })
            .filter(|w| {
                if normalized_required.is_empty() {
                    return true;
                }
                let Some(capabilities) = w.capabilities.as_ref() else {
                    return false;
                };
                let Some(runtimes) = capabilities.get("runtimes").and_then(|r| r.as_array()) else {
                    return false;
                };
                let advertised: Vec<String> = runtimes
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(normalize_runtime_name)
                    .collect();
                normalized_required
                    .iter()
                    .all(|required| advertised.contains(required))
            })
            .filter(Self::is_worker_heartbeat_fresh)
            .collect();

        if capable_workers.is_empty() {
            let required = if normalized_required.is_empty() {
                "any".to_string()
            } else {
                normalized_required.join(", ")
            };
            return Err(anyhow::anyhow!(
                "No active workers support the runtimes required for pack tests: {}",
                required
            ));
        }

        let count = round_robin_counter.fetch_add(1, Ordering::Relaxed);
        let index = count % capable_workers.len();
        Ok(capable_workers[index].clone())
    }

    fn effective_placement(
        action: &Action,
        execution: Option<&Execution>,
    ) -> Result<EffectiveWorkerPlacement> {
        let selector = if let Some(selector) = execution.and_then(|e| e.worker_selector.as_ref()) {
            parse_worker_selector(selector)?
        } else {
            action.worker_selector_labels()
        };
        let tolerations =
            if let Some(tolerations) = execution.and_then(|e| e.worker_tolerations.as_ref()) {
                parse_worker_tolerations(tolerations)?
            } else {
                action.worker_toleration_specs()
            };
        let affinity = if let Some(affinity) = execution.and_then(|e| e.worker_affinity.as_ref()) {
            parse_worker_affinity(affinity)?
        } else {
            action.worker_affinity_spec()
        };

        Ok(EffectiveWorkerPlacement {
            selector,
            tolerations,
            affinity,
        })
    }

    fn worker_satisfies_placement(
        worker: &attune_common::models::Worker,
        action: &Action,
        placement: &EffectiveWorkerPlacement,
    ) -> bool {
        let labels = worker_labels_from_capabilities(worker.capabilities.as_ref());
        let taints = worker_taints_from_capabilities(worker.capabilities.as_ref());

        let matches = worker_matches_placement(
            &labels,
            &taints,
            &placement.selector,
            &placement.tolerations,
            &placement.affinity,
        );
        if !matches {
            debug!(
                "Worker {} rejected by placement constraints for action {}",
                worker.name, action.r#ref
            );
        }
        matches
    }

    fn worker_preference_score(
        worker: &attune_common::models::Worker,
        placement: &EffectiveWorkerPlacement,
    ) -> i32 {
        let labels = worker_labels_from_capabilities(worker.capabilities.as_ref());
        preferred_affinity_score(&labels, &placement.affinity)
    }

    fn placement_description(placement: &EffectiveWorkerPlacement) -> String {
        let selector = &placement.selector;
        let tolerations = &placement.tolerations;
        let affinity = &placement.affinity;

        if selector.is_empty() && tolerations.is_empty() && affinity.is_empty() {
            return "none".to_string();
        }

        format!(
            "selector={:?}, tolerations={}, required_affinity_terms={}, preferred_affinity_terms={}, anti_affinity_terms={}",
            selector,
            tolerations.len(),
            affinity.required.len(),
            affinity.preferred.len(),
            affinity.anti_affinity.len(),
        )
    }

    /// Check if a worker supports a given runtime
    ///
    /// This checks the worker's capabilities.runtimes array against the runtime's aliases.
    /// If aliases are missing, fall back to the runtime's canonical name.
    fn worker_supports_runtime(worker: &attune_common::models::Worker, runtime: &Runtime) -> bool {
        let runtime_names = Self::runtime_capability_names(runtime);

        // Try to parse capabilities and check runtimes array
        if let Some(ref capabilities) = worker.capabilities {
            if let Some(runtimes) = capabilities.get("runtimes") {
                if let Some(runtime_array) = runtimes.as_array() {
                    // Check if any runtime in the array matches via aliases
                    for runtime_value in runtime_array {
                        if let Some(runtime_str) = runtime_value.as_str() {
                            if runtime_names
                                .iter()
                                .any(|candidate| candidate.eq_ignore_ascii_case(runtime_str))
                                || runtime_aliases_contain(&runtime.aliases, runtime_str)
                            {
                                debug!(
                                    "Worker {} supports runtime '{}' via capabilities (matched '{}', candidates: {:?})",
                                    worker.name, runtime.name, runtime_str, runtime_names
                                );
                                return true;
                            }
                        }
                    }
                }
            }
        }

        debug!(
            "Worker {} does not support runtime '{}' (candidates: {:?})",
            worker.name, runtime.name, runtime_names
        );
        false
    }

    fn worker_supports_required_runtimes(
        worker: &attune_common::models::Worker,
        required_runtimes: &JsonValue,
    ) -> bool {
        let constraints = required_runtimes.as_object().cloned().unwrap_or_default();

        if constraints.is_empty() {
            return true;
        }

        let advertised_runtimes: HashSet<String> = worker
            .capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.get("runtimes"))
            .and_then(|runtimes| runtimes.as_array())
            .into_iter()
            .flatten()
            .filter_map(|runtime| runtime.as_str())
            .map(normalize_runtime_name)
            .collect();

        let Some(capabilities) = worker.capabilities.as_ref() else {
            return false;
        };

        for (runtime_name, constraint) in constraints {
            let Some(constraint) = constraint.as_str() else {
                warn!(
                    "Required worker runtime constraint for '{}' is not a string during scheduling",
                    runtime_name
                );
                return false;
            };

            let normalized_runtime_name = normalize_runtime_name(&runtime_name);
            if !advertised_runtimes.contains(&normalized_runtime_name) {
                return false;
            }

            if constraint.trim() == "*" {
                continue;
            }

            let advertised_versions = Self::worker_runtime_versions(
                capabilities,
                std::slice::from_ref(&normalized_runtime_name),
            );

            if advertised_versions.is_empty() {
                debug!(
                    "Worker {} does not advertise versions for required runtime '{}' and constraint '{}'",
                    worker.name,
                    runtime_name,
                    constraint,
                );
                return false;
            }

            let matches = advertised_versions.iter().any(|version| match matches_constraint(version, constraint) {
                Ok(result) => result,
                Err(e) => {
                    warn!(
                        "Invalid required runtime version constraint '{}' for runtime '{}' against worker {} version '{}': {}",
                        constraint,
                        runtime_name,
                        worker.name,
                        version,
                        e,
                    );
                    false
                }
            });

            if !matches {
                debug!(
                    "Worker {} does not satisfy required runtime version '{}' for runtime '{}'",
                    worker.name, constraint, runtime_name,
                );
                return false;
            }
        }

        true
    }

    fn worker_supports_runtime_constraint(
        worker: &attune_common::models::Worker,
        runtime: &Runtime,
        constraint: Option<&str>,
    ) -> bool {
        let Some(constraint) = constraint.filter(|constraint| !constraint.trim().is_empty()) else {
            return true;
        };

        let Some(capabilities) = worker.capabilities.as_ref() else {
            debug!(
                "Worker {} has no capabilities; cannot satisfy runtime constraint '{}' for runtime '{}'",
                worker.name,
                constraint,
                runtime.name,
            );
            return false;
        };

        let candidate_runtime_names: Vec<String> = Self::runtime_capability_names(runtime)
            .into_iter()
            .map(|name| normalize_runtime_name(&name))
            .collect();

        let advertised_versions =
            Self::worker_runtime_versions(capabilities, &candidate_runtime_names);

        if advertised_versions.is_empty() {
            debug!(
                "Worker {} does not advertise compatible runtime versions for runtime '{}' and constraint '{}'",
                worker.name,
                runtime.name,
                constraint,
            );
            return false;
        }

        for version in advertised_versions {
            match matches_constraint(&version, constraint) {
                Ok(true) => {
                    debug!(
                        "Worker {} satisfies runtime constraint '{}' for runtime '{}' via version '{}'",
                        worker.name,
                        constraint,
                        runtime.name,
                        version,
                    );
                    return true;
                }
                Ok(false) => continue,
                Err(e) => {
                    warn!(
                        "Invalid runtime version comparison for worker {} runtime '{}' version '{}' constraint '{}': {}",
                        worker.name,
                        runtime.name,
                        version,
                        constraint,
                        e,
                    );
                }
            }
        }

        debug!(
            "Worker {} does not satisfy runtime constraint '{}' for runtime '{}'",
            worker.name, constraint, runtime.name,
        );
        false
    }

    fn worker_runtime_versions(
        capabilities: &JsonValue,
        candidate_runtime_names: &[String],
    ) -> Vec<String> {
        let mut versions = Vec::new();

        let Some(capabilities_obj) = capabilities.as_object() else {
            return versions;
        };

        if let Some(runtime_versions) = capabilities_obj.get(RUNTIME_VERSIONS_CAPABILITY_KEY) {
            if let Some(runtime_versions_obj) = runtime_versions.as_object() {
                for runtime_name in candidate_runtime_names {
                    if let Some(version_values) = runtime_versions_obj.get(runtime_name) {
                        if let Some(version_array) = version_values.as_array() {
                            versions.extend(
                                version_array
                                    .iter()
                                    .filter_map(|value| value.as_str().map(ToOwned::to_owned)),
                            );
                        }
                    }
                }
            }
        }

        if versions.is_empty() {
            if let Some(detected_interpreters) = capabilities_obj.get("detected_interpreters") {
                if let Some(interpreters) = detected_interpreters.as_array() {
                    for interpreter in interpreters {
                        let Some(name) = interpreter.get("name").and_then(|value| value.as_str())
                        else {
                            continue;
                        };

                        if !candidate_runtime_names
                            .iter()
                            .any(|candidate| candidate == &normalize_runtime_name(name))
                        {
                            continue;
                        }

                        if let Some(version) =
                            interpreter.get("version").and_then(|value| value.as_str())
                        {
                            versions.push(version.to_string());
                        }
                    }
                }
            }
        }

        versions.sort();
        versions.dedup();
        versions
    }

    fn runtime_capability_names(runtime: &Runtime) -> Vec<String> {
        let mut names: Vec<String> = runtime
            .aliases
            .iter()
            .map(|alias| alias.to_ascii_lowercase())
            .filter(|alias| !alias.is_empty())
            .collect();

        let runtime_name = runtime.name.to_ascii_lowercase();
        if !runtime_name.is_empty() && !names.iter().any(|name| name == &runtime_name) {
            names.push(runtime_name);
        }

        names
    }

    fn is_unschedulable_error(error: &anyhow::Error) -> bool {
        let message = error.to_string();
        message.starts_with("No compatible workers found")
            || message.starts_with("No action workers available")
            || message.starts_with("No active workers available")
            || message.starts_with("No workers with fresh heartbeats available")
    }

    fn is_policy_cancellation_error(error: &anyhow::Error) -> bool {
        let message = error.to_string();
        message.contains("Policy violation:")
            || message.starts_with("Queue full for action ")
            || message.starts_with("Queue timeout for execution ")
    }

    async fn release_acquired_policy_slot(
        policy_enforcer: &PolicyEnforcer,
        pool: &PgPool,
        publisher: &Publisher,
        execution_id: i64,
    ) -> Result<()> {
        let release = match policy_enforcer.release_execution_slot(execution_id).await {
            Ok(release) => release,
            Err(release_err) => {
                warn!(
                    "Failed to release acquired policy slot for execution {} after scheduling error: {}",
                    execution_id, release_err
                );
                return Err(release_err);
            }
        };

        let Some(release) = release else {
            return Ok(());
        };

        if let Some(next_execution_id) = release.next_execution_id {
            if let Err(republish_err) =
                Self::republish_execution_requested(pool, publisher, next_execution_id).await
            {
                warn!(
                    "Failed to republish deferred execution {} after releasing slot from execution {}: {}",
                    next_execution_id, execution_id, republish_err
                );
                if let Err(restore_err) = policy_enforcer
                    .restore_execution_slot(execution_id, &release)
                    .await
                {
                    warn!(
                        "Failed to restore policy slot for execution {} after republish error: {}",
                        execution_id, restore_err
                    );
                }
                return Err(republish_err);
            }
        }

        Ok(())
    }

    async fn remove_queued_policy_execution(
        policy_enforcer: &PolicyEnforcer,
        pool: &PgPool,
        publisher: &Publisher,
        execution_id: i64,
    ) {
        let removal = match policy_enforcer.remove_queued_execution(execution_id).await {
            Ok(removal) => removal,
            Err(remove_err) => {
                warn!(
                    "Failed to remove queued policy execution {} during scheduler cleanup: {}",
                    execution_id, remove_err
                );
                return;
            }
        };

        let Some(removal) = removal else {
            return;
        };

        if let Some(next_execution_id) = removal.next_execution_id {
            if let Err(republish_err) =
                Self::republish_execution_requested(pool, publisher, next_execution_id).await
            {
                warn!(
                    "Failed to republish successor {} after removing queued execution {}: {}",
                    next_execution_id, execution_id, republish_err
                );
                if let Err(restore_err) = policy_enforcer.restore_queued_execution(&removal).await {
                    warn!(
                        "Failed to restore queued execution {} after republish error: {}",
                        execution_id, restore_err
                    );
                }
            }
        }
    }

    async fn republish_execution_requested(
        pool: &PgPool,
        publisher: &Publisher,
        execution_id: i64,
    ) -> Result<()> {
        let execution = ExecutionRepository::find_by_id(pool, execution_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Execution {} not found", execution_id))?;

        let action_id = execution
            .action
            .ok_or_else(|| anyhow::anyhow!("Execution {} has no action", execution_id))?;
        let payload = ExecutionRequestedPayload {
            execution_id,
            action_id: Some(action_id),
            action_ref: execution.action_ref.clone(),
            parent_id: execution.parent,
            enforcement_id: execution.enforcement,
            config: execution.config.clone(),
        };

        let envelope = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
            .with_source("executor-scheduler");

        publisher.publish_envelope(&envelope).await?;

        debug!(
            "Republished deferred ExecutionRequested for execution {}",
            execution_id
        );

        Ok(())
    }

    async fn fail_unschedulable_execution(
        pool: &PgPool,
        publisher: &Publisher,
        envelope: &MessageEnvelope<ExecutionRequestedPayload>,
        execution_id: i64,
        action_id: i64,
        action_ref: &str,
        error_message: &str,
    ) -> Result<()> {
        let completed_at = Utc::now();
        let result = serde_json::json!({
            "error": "Execution is unschedulable",
            "message": error_message,
            "action_ref": action_ref,
            "failed_by": "execution_scheduler",
            "failed_at": completed_at.to_rfc3339(),
        });

        let updated = ExecutionRepository::update_if_status(
            pool,
            execution_id,
            ExecutionStatus::Scheduling,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Failed),
                result: Some(result.clone()),
                ..Default::default()
            },
        )
        .await?;

        if updated.is_none() {
            warn!(
                "Skipping unschedulable failure for execution {} because it already left Scheduling",
                execution_id
            );
            return Ok(());
        }

        let completed = MessageEnvelope::new(
            MessageType::ExecutionCompleted,
            ExecutionCompletedPayload {
                execution_id,
                action_id,
                action_ref: action_ref.to_string(),
                status: "failed".to_string(),
                result: Some(result),
                completed_at,
            },
        )
        .with_correlation_id(envelope.correlation_id)
        .with_source("attune-executor");

        publisher.publish_envelope(&completed).await?;

        warn!(
            "Execution {} marked failed as unschedulable: {}",
            execution_id, error_message
        );

        Ok(())
    }

    async fn cancel_execution_for_policy_violation(
        pool: &PgPool,
        publisher: &Publisher,
        envelope: &MessageEnvelope<ExecutionRequestedPayload>,
        execution_id: i64,
        action_id: i64,
        action_ref: &str,
        error_message: &str,
    ) -> Result<()> {
        let completed_at = Utc::now();
        let result = serde_json::json!({
            "error": "Execution cancelled by policy",
            "message": error_message,
            "action_ref": action_ref,
            "cancelled_by": "execution_scheduler",
            "cancelled_at": completed_at.to_rfc3339(),
        });

        let updated = ExecutionRepository::update_if_status(
            pool,
            execution_id,
            ExecutionStatus::Scheduling,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Cancelled),
                result: Some(result.clone()),
                ..Default::default()
            },
        )
        .await?;

        if updated.is_none() {
            warn!(
                "Skipping policy cancellation for execution {} because it already left Scheduling",
                execution_id
            );
            return Ok(());
        }

        let completed = MessageEnvelope::new(
            MessageType::ExecutionCompleted,
            ExecutionCompletedPayload {
                execution_id,
                action_id,
                action_ref: action_ref.to_string(),
                status: "cancelled".to_string(),
                result: Some(result),
                completed_at,
            },
        )
        .with_correlation_id(envelope.correlation_id)
        .with_source("attune-executor");

        publisher.publish_envelope(&completed).await?;

        warn!(
            "Execution {} cancelled due to policy violation: {}",
            execution_id, error_message
        );

        Ok(())
    }

    async fn revert_scheduled_execution(
        pool: &PgPool,
        execution_id: i64,
        policy_enforcer: &PolicyEnforcer,
        publisher: &Publisher,
    ) -> Result<()> {
        match ExecutionRepository::revert_scheduled_to_requested(pool, execution_id).await? {
            Some(_) => {
                Self::release_acquired_policy_slot(policy_enforcer, pool, publisher, execution_id)
                    .await?;
            }
            None => {
                let execution = ExecutionRepository::find_by_id(pool, execution_id).await?;
                let should_release_slot = match execution.as_ref().map(|execution| execution.status)
                {
                    Some(
                        ExecutionStatus::Running
                        | ExecutionStatus::Completed
                        | ExecutionStatus::Timeout
                        | ExecutionStatus::Abandoned,
                    ) => false,
                    Some(
                        ExecutionStatus::Requested
                        | ExecutionStatus::Scheduling
                        | ExecutionStatus::Scheduled
                        | ExecutionStatus::Failed
                        | ExecutionStatus::Canceling
                        | ExecutionStatus::Cancelled,
                    ) => true,
                    None => true,
                };

                if should_release_slot {
                    Self::release_acquired_policy_slot(
                        policy_enforcer,
                        pool,
                        publisher,
                        execution_id,
                    )
                    .await?;
                }

                warn!(
                    "Execution {} left Scheduled before scheduler could revert it after publish failure",
                    execution_id
                );
            }
        }

        Ok(())
    }

    async fn revert_scheduling_claim(pool: &PgPool, execution_id: i64) -> Result<()> {
        if ExecutionRepository::update_if_status(
            pool,
            execution_id,
            ExecutionStatus::Scheduling,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Requested),
                ..Default::default()
            },
        )
        .await?
        .is_none()
        {
            debug!(
                "Execution {} left Scheduling before claim revert after workflow-start error",
                execution_id
            );
        }

        Ok(())
    }

    async fn cleanup_unclaimable_execution(
        policy_enforcer: &PolicyEnforcer,
        pool: &PgPool,
        publisher: &Publisher,
        execution_id: i64,
    ) -> Result<()> {
        let execution = ExecutionRepository::find_by_id(pool, execution_id).await?;
        match execution.as_ref().map(|execution| execution.status) {
            Some(ExecutionStatus::Requested | ExecutionStatus::Scheduling) => {}
            _ => {
                Self::remove_queued_policy_execution(
                    policy_enforcer,
                    pool,
                    publisher,
                    execution_id,
                )
                .await;
            }
        }

        Ok(())
    }

    /// Check if a worker's heartbeat is fresh enough to schedule work
    ///
    /// A worker is considered fresh if its last heartbeat is within
    /// HEARTBEAT_STALENESS_MULTIPLIER * HEARTBEAT_INTERVAL seconds.
    fn is_worker_heartbeat_fresh(worker: &attune_common::models::Worker) -> bool {
        let Some(last_heartbeat) = worker.last_heartbeat else {
            warn!(
                "Worker {} has no heartbeat recorded, considering stale",
                worker.name
            );
            return false;
        };

        let now = Utc::now();
        let age = now.signed_duration_since(last_heartbeat);
        let max_age =
            Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL * HEARTBEAT_STALENESS_MULTIPLIER);

        let is_fresh = age.to_std().unwrap_or(Duration::MAX) <= max_age;

        if !is_fresh {
            warn!(
                "Worker {} heartbeat is stale: last seen {} seconds ago (max: {} seconds)",
                worker.name,
                age.num_seconds(),
                max_age.as_secs()
            );
        } else {
            debug!(
                "Worker {} heartbeat is fresh: last seen {} seconds ago",
                worker.name,
                age.num_seconds()
            );
        }

        is_fresh
    }

    /// Queue execution to a specific worker
    async fn queue_to_worker(
        publisher: &Publisher,
        execution_id: &i64,
        worker_id: &i64,
        action_ref: &str,
        config: &Option<JsonValue>,
        scheduled_attempt_updated_at: DateTime<Utc>,
        _action: &Action,
    ) -> Result<()> {
        debug!("Queuing execution {} to worker {}", execution_id, worker_id);

        // Create payload for worker
        let payload = ExecutionScheduledPayload {
            execution_id: *execution_id,
            worker_id: *worker_id,
            action_ref: action_ref.to_string(),
            config: config.clone(),
            scheduled_attempt_updated_at,
        };

        let envelope =
            MessageEnvelope::new(MessageType::ExecutionRequested, payload).with_source("executor");

        // Publish to worker-specific queue with routing key
        let routing_key = format!("execution.dispatch.worker.{}", worker_id);
        let exchange = "attune.executions";

        publisher
            .publish_envelope_with_routing(&envelope, exchange, &routing_key)
            .await?;

        info!(
            "Published execution.scheduled message to worker {} (routing key: {})",
            worker_id, routing_key
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::graph::TaskNode;
    use attune_common::models::{Worker, WorkerRole, WorkerStatus, WorkerType};
    use attune_common::workflow::TaskType;
    use chrono::{Duration as ChronoDuration, Utc};
    use std::collections::HashMap;

    fn create_test_worker(name: &str, heartbeat_offset_secs: i64) -> Worker {
        let last_heartbeat = if heartbeat_offset_secs == 0 {
            None
        } else {
            Some(Utc::now() - ChronoDuration::seconds(heartbeat_offset_secs))
        };

        Worker {
            id: 1,
            name: name.to_string(),
            worker_type: WorkerType::Local,
            worker_role: WorkerRole::Action,
            runtime: None,
            host: Some("localhost".to_string()),
            port: Some(8080),
            status: Some(WorkerStatus::Active),
            capabilities: Some(serde_json::json!({
                "runtimes": ["shell", "python"]
            })),
            meta: None,
            last_heartbeat,
            cordoned: false,
            cordon_reason: None,
            cordoned_by: None,
            cordoned_at: None,
            created: Utc::now(),
            updated: Utc::now(),
        }
    }

    #[test]
    fn reconcile_authoritative_task_statuses_preserves_unhandled_parallel_failure() {
        let mut completed_tasks = vec!["success_1".to_string(), "success_2".to_string()];
        let mut failed_tasks = Vec::new();

        reconcile_authoritative_task_statuses(
            &mut completed_tasks,
            &mut failed_tasks,
            vec![
                (
                    "success_1".to_string(),
                    None,
                    ExecutionStatus::Completed,
                    None,
                    0,
                ),
                (
                    "failure".to_string(),
                    None,
                    ExecutionStatus::Failed,
                    None,
                    0,
                ),
                (
                    "success_2".to_string(),
                    None,
                    ExecutionStatus::Completed,
                    None,
                    0,
                ),
            ],
        );

        assert!(completed_tasks.contains(&"success_1".to_string()));
        assert!(completed_tasks.contains(&"success_2".to_string()));
        assert!(failed_tasks.contains(&"failure".to_string()));
    }

    #[test]
    fn reconcile_authoritative_task_statuses_marks_handled_failure_completed() {
        let mut completed_tasks = Vec::new();
        let mut failed_tasks = vec!["validate".to_string()];

        reconcile_authoritative_task_statuses(
            &mut completed_tasks,
            &mut failed_tasks,
            vec![
                (
                    "validate".to_string(),
                    None,
                    ExecutionStatus::Failed,
                    None,
                    0,
                ),
                (
                    "repair".to_string(),
                    None,
                    ExecutionStatus::Completed,
                    Some("validate".to_string()),
                    0,
                ),
            ],
        );

        assert!(completed_tasks.contains(&"validate".to_string()));
        assert!(completed_tasks.contains(&"repair".to_string()));
        assert!(!failed_tasks.contains(&"validate".to_string()));
    }

    #[test]
    fn test_heartbeat_freshness_with_recent_heartbeat() {
        // Worker with heartbeat 30 seconds ago (within limit)
        let worker = create_test_worker("test-worker", 30);
        assert!(
            ExecutionScheduler::is_worker_heartbeat_fresh(&worker),
            "Worker with 30s old heartbeat should be considered fresh"
        );
    }

    #[test]
    fn test_heartbeat_freshness_with_stale_heartbeat() {
        // Worker with heartbeat 100 seconds ago (beyond 3x30s = 90s limit)
        let worker = create_test_worker("test-worker", 100);
        assert!(
            !ExecutionScheduler::is_worker_heartbeat_fresh(&worker),
            "Worker with 100s old heartbeat should be considered stale"
        );
    }

    #[test]
    fn test_heartbeat_freshness_at_boundary() {
        // Worker with heartbeat exactly at the 90 second boundary
        let worker = create_test_worker("test-worker", 90);
        assert!(
            !ExecutionScheduler::is_worker_heartbeat_fresh(&worker),
            "Worker with 90s old heartbeat should be considered stale (at boundary)"
        );
    }

    #[test]
    fn test_heartbeat_freshness_with_no_heartbeat() {
        // Worker with no heartbeat recorded
        let worker = create_test_worker("test-worker", 0);
        assert!(
            !ExecutionScheduler::is_worker_heartbeat_fresh(&worker),
            "Worker with no heartbeat should be considered stale"
        );
    }

    #[test]
    fn test_heartbeat_freshness_with_very_recent() {
        // Worker with heartbeat 5 seconds ago
        let worker = create_test_worker("test-worker", 5);
        assert!(
            ExecutionScheduler::is_worker_heartbeat_fresh(&worker),
            "Worker with 5s old heartbeat should be considered fresh"
        );
    }

    #[test]
    fn test_scheduler_creation() {
        // This is a placeholder test
        // Real tests will require database and message queue setup
    }

    #[test]
    fn test_worker_supports_runtime_with_alias_match() {
        let worker = create_test_worker("test-worker", 5);
        let runtime = Runtime {
            id: 1,
            r#ref: "core.shell".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: Some("Shell runtime".to_string()),
            name: "Shell".to_string(),
            aliases: vec!["shell".to_string(), "bash".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ExecutionScheduler::worker_supports_runtime(
            &worker, &runtime
        ));
    }

    #[test]
    fn test_worker_supports_runtime_falls_back_to_runtime_name_when_aliases_missing() {
        let worker = create_test_worker("test-worker", 5);
        let runtime = Runtime {
            id: 1,
            r#ref: "core.shell".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: Some("Shell runtime".to_string()),
            name: "Shell".to_string(),
            aliases: vec![],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ExecutionScheduler::worker_supports_runtime(
            &worker, &runtime
        ));
    }

    #[test]
    fn test_worker_supports_runtime_constraint_with_matching_version() {
        let mut worker = create_test_worker("test-worker", 5);
        worker.capabilities = Some(serde_json::json!({
            "runtimes": ["python"],
            "runtime_versions": {
                "python": ["3.12", "3.11"]
            }
        }));

        let runtime = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: Some("Python runtime".to_string()),
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ExecutionScheduler::worker_supports_runtime_constraint(
            &worker,
            &runtime,
            Some(">=3.12"),
        ));
        assert!(!ExecutionScheduler::worker_supports_runtime_constraint(
            &worker,
            &runtime,
            Some(">=3.13"),
        ));
    }

    #[test]
    fn test_worker_supports_runtime_constraint_uses_normalized_runtime_keys() {
        let mut worker = create_test_worker("test-worker", 5);
        worker.capabilities = Some(serde_json::json!({
            "runtimes": ["node"],
            "runtime_versions": {
                "node": ["20"]
            }
        }));

        let runtime = Runtime {
            id: 1,
            r#ref: "core.nodejs".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: Some("Node.js runtime".to_string()),
            name: "Node.js".to_string(),
            aliases: vec![
                "node".to_string(),
                "nodejs".to_string(),
                "node.js".to_string(),
            ],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ExecutionScheduler::worker_supports_runtime_constraint(
            &worker,
            &runtime,
            Some(">=18"),
        ));
    }

    #[test]
    fn test_worker_supports_required_runtimes_with_alias_normalization() {
        let mut worker = create_test_worker("test-worker", 5);
        worker.capabilities = Some(serde_json::json!({
            "runtimes": ["shell", "node"]
        }));

        assert!(ExecutionScheduler::worker_supports_required_runtimes(
            &worker,
            &serde_json::json!({ "nodejs": "*" })
        ));
        assert!(!ExecutionScheduler::worker_supports_required_runtimes(
            &worker,
            &serde_json::json!({ "ruby": "*" })
        ));
    }

    #[test]
    fn test_worker_supports_required_runtimes_with_version_constraints() {
        let mut worker = create_test_worker("test-worker", 6);
        worker.capabilities = Some(serde_json::json!({
            "runtimes": ["shell", "node"],
            "runtime_versions": {
                "node": ["20.11.1"]
            }
        }));

        assert!(ExecutionScheduler::worker_supports_required_runtimes(
            &worker,
            &serde_json::json!({ "node": ">=20" })
        ));
        assert!(!ExecutionScheduler::worker_supports_required_runtimes(
            &worker,
            &serde_json::json!({ "node": "<20" })
        ));
    }

    #[test]
    fn test_worker_supports_runtime_constraint_falls_back_to_detected_interpreters() {
        let mut worker = create_test_worker("test-worker", 5);
        worker.capabilities = Some(serde_json::json!({
            "runtimes": ["python"],
            "detected_interpreters": [
                {
                    "name": "python",
                    "path": "/usr/local/bin/python3",
                    "version": "3.12.13"
                }
            ]
        }));

        let runtime = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: None,
            pack_ref: Some("core".to_string()),
            description: Some("Python runtime".to_string()),
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: serde_json::json!({}),
            installation: None,
            installers: serde_json::json!({}),
            execution_config: serde_json::json!({}),
            auto_detected: false,
            detection_config: serde_json::json!({}),
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ExecutionScheduler::worker_supports_runtime_constraint(
            &worker,
            &runtime,
            Some(">=3.9"),
        ));
    }

    #[test]
    fn test_unschedulable_error_classification() {
        assert!(ExecutionScheduler::is_unschedulable_error(
            &anyhow::anyhow!(
                "No compatible workers found for action: core.sleep (requires runtime: Shell)"
            )
        ));
        assert!(!ExecutionScheduler::is_unschedulable_error(
            &anyhow::anyhow!("database temporarily unavailable")
        ));
    }

    #[test]
    fn test_policy_cancellation_error_classification() {
        assert!(ExecutionScheduler::is_policy_cancellation_error(
            &anyhow::anyhow!(
                "Policy violation: Concurrency limit exceeded: 1 running executions (limit: 1)"
            )
        ));
        assert!(ExecutionScheduler::is_policy_cancellation_error(
            &anyhow::anyhow!("Queue full for action 42: maximum 100 entries")
        ));
        assert!(ExecutionScheduler::is_policy_cancellation_error(
            &anyhow::anyhow!("Queue timeout for execution 99: waited 60 seconds")
        ));
        assert!(!ExecutionScheduler::is_policy_cancellation_error(
            &anyhow::anyhow!("rabbitmq publish failed")
        ));
    }

    #[test]
    fn test_concurrency_limit_dispatch_count() {
        // Verify the dispatch_count calculation used by dispatch_with_items_task
        let total = 20usize;
        let concurrency_limit = 3usize;
        let dispatch_count = total.min(concurrency_limit);
        assert_eq!(dispatch_count, 3);

        // No concurrency limit → default to serial (1 at a time)
        let concurrency_limit = 1usize;
        let dispatch_count = total.min(concurrency_limit);
        assert_eq!(dispatch_count, 1);

        // Concurrency exceeds total → dispatch all
        let concurrency_limit = 50usize;
        let dispatch_count = total.min(concurrency_limit);
        assert_eq!(dispatch_count, 20);
    }

    #[test]
    fn iteration_batch_size_changes_item_shape_independently() {
        let items = vec![
            serde_json::json!(1),
            serde_json::json!(2),
            serde_json::json!(3),
        ];
        assert_eq!(iteration_items(items.clone(), None), items);
        assert_eq!(
            iteration_items(items.clone(), Some(1)),
            vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3)
            ]
        );
        assert_eq!(
            iteration_items(items, Some(2)),
            vec![serde_json::json!([1, 2]), serde_json::json!([3])]
        );
    }

    #[test]
    fn cache_item_matches_cache_entry_response_shape() {
        let item = cache_entry_item(CacheEntry {
            id: 99,
            generation: 7,
            external_id: "customer-1".to_string(),
            value: serde_json::json!({"enabled": true}),
            source_updated_at: None,
            source_checksum: Some("sha256:test".to_string()),
            size_bytes: 42,
            created: Utc::now(),
        });
        assert_eq!(
            item,
            serde_json::json!({
                "external_id": "customer-1",
                "value": {"enabled": true},
                "source_updated_at": null,
                "source_checksum": "sha256:test",
                "size_bytes": 42,
            })
        );
        assert!(item.get("id").is_none());
        assert!(item.get("generation").is_none());
        assert!(item.get("created").is_none());
    }

    #[test]
    fn standard_cache_read_is_scoped_and_fail_closed() {
        assert!(standard_cache_read_allowed(
            true,
            "child.process",
            "workflow.run",
            OwnerType::Pack,
            Some("child"),
        ));
        assert!(standard_cache_read_allowed(
            true,
            "child.process",
            "workflow.run",
            OwnerType::Action,
            Some("workflow.run"),
        ));
        assert!(!standard_cache_read_allowed(
            true,
            "child.process",
            "workflow.run",
            OwnerType::Pack,
            Some("other"),
        ));
        assert!(!standard_cache_read_allowed(
            false,
            "child.process",
            "workflow.run",
            OwnerType::Action,
            Some("child.process"),
        ));
        assert!(!standard_cache_read_allowed(
            true,
            "child.process",
            "workflow.run",
            OwnerType::System,
            None,
        ));
    }

    #[test]
    fn cache_iteration_authorization_uses_identity_attributes() {
        let grant: Grant = serde_json::from_value(serde_json::json!({
            "resource": "caches",
            "actions": ["read"],
            "constraints": {
                "owner_types": ["pack"],
                "owner_refs": ["salesforce"],
                "refs": ["customers"],
                "attributes": {"department": "sales"}
            }
        }))
        .unwrap();
        let context = cache_iteration_authorization_context(
            42,
            serde_json::json!({"department": "sales"}),
            OwnerType::Pack,
            Some("salesforce"),
            "customers",
        );

        assert!(grant.allows(Resource::Caches, RbacAction::Read, &context));
    }

    #[test]
    fn named_cache_permissions_must_be_delegated_by_parent() {
        let standard = attune_common::auth::jwt::STANDARD_EXECUTION_ACCESS_REF.to_string();
        let delegated = vec!["cache.reader".to_string(), standard.clone()];
        assert!(named_cache_permission_refs_are_delegated(
            &["cache.reader".to_string()],
            &delegated,
        ));
        assert!(named_cache_permission_refs_are_delegated(&[standard], &[],));
        assert!(!named_cache_permission_refs_are_delegated(
            &["cache.admin".to_string()],
            &delegated,
        ));
        assert!(!named_cache_permission_refs_are_delegated(
            &["cache.reader".to_string(), "cache.admin".to_string()],
            &delegated,
        ));
    }

    #[test]
    fn cache_freshness_matches_api_age_and_state_semantics() {
        let now = Utc::now();
        assert!(!cache_generation_is_stale(
            CacheGenerationState::Active,
            Some(now - chrono::Duration::milliseconds(10_999)),
            10,
            now,
        ));
        assert!(cache_generation_is_stale(
            CacheGenerationState::Active,
            Some(now - chrono::Duration::seconds(11)),
            10,
            now,
        ));
        assert!(!cache_generation_is_stale(
            CacheGenerationState::Active,
            None,
            10,
            now,
        ));
        assert!(cache_generation_is_stale(
            CacheGenerationState::Retired,
            Some(now),
            0,
            now,
        ));
    }

    #[test]
    fn cache_generation_selector_has_typed_logical_failures() {
        assert_eq!(
            parse_cache_generation_selector("ACTIVE"),
            Ok(CacheGenerationSelector::Active)
        );
        assert_eq!(
            parse_cache_generation_selector("42"),
            Ok(CacheGenerationSelector::Explicit(42))
        );
        assert_eq!(
            parse_cache_generation_selector("latest"),
            Err(CacheIterationInitializationFailure::InvalidGeneration)
        );
    }

    #[test]
    fn cache_iteration_initialization_failure_codes_are_bounded() {
        let failures = [
            CacheIterationInitializationFailure::SelectorRendering,
            CacheIterationInitializationFailure::OwnerResolution,
            CacheIterationInitializationFailure::NamespaceResolution,
            CacheIterationInitializationFailure::PermissionResolution,
            CacheIterationInitializationFailure::NotAuthorized,
            CacheIterationInitializationFailure::NoActiveGeneration,
            CacheIterationInitializationFailure::InvalidGeneration,
            CacheIterationInitializationFailure::GenerationNotReadable,
            CacheIterationInitializationFailure::StaleGeneration,
        ];
        let codes = failures.map(CacheIterationInitializationFailure::code);
        assert!(codes.iter().all(|code| {
            code.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        }));
        assert_eq!(
            codes.into_iter().collect::<HashSet<_>>().len(),
            failures.len()
        );
    }

    #[test]
    fn synthetic_cache_iteration_failure_result_is_sanitized() {
        let result = cache_iteration_terminal_result(WorkflowCacheIterationState::Failed);
        assert_eq!(
            result,
            serde_json::json!({
                "cache_iteration": {"state": "failed"}
            })
        );
        assert!(!result.to_string().contains("error"));
        assert!(!result.to_string().contains("reason"));
    }

    #[test]
    fn failed_synthetic_cache_iteration_drives_failed_outcome_without_pin() {
        assert_eq!(
            cache_iteration_outcome_without_state(ExecutionStatus::Failed).unwrap(),
            TaskOutcome::Failed
        );
        assert!(cache_iteration_outcome_without_state(ExecutionStatus::Completed).is_err());
    }

    #[test]
    fn cache_iteration_materialization_counts_one_total_child_payload() {
        let first = serde_json::json!({"value": "one"});
        let second = serde_json::json!({"value": "two"});
        let first_increment = cache_iteration_item_increment(&first, 0, true).unwrap();
        let second_increment = cache_iteration_item_increment(&second, 1, true).unwrap();
        assert_eq!(
            first_increment + second_increment,
            serde_json::to_vec(&serde_json::json!([first, second]))
                .unwrap()
                .len()
        );
        assert_eq!(
            cache_iteration_item_increment(&serde_json::json!({"value": 1}), 0, false).unwrap(),
            serde_json::to_vec(&serde_json::json!({"value": 1}))
                .unwrap()
                .len()
        );
    }

    #[test]
    fn cache_iteration_budget_does_not_consume_rejected_cursor_entry() {
        let entry = |external_id: &str| CacheEntry {
            id: 1,
            generation: 7,
            external_id: external_id.to_string(),
            value: serde_json::json!({"payload": "large-enough"}),
            source_updated_at: None,
            source_checksum: None,
            size_bytes: 12,
            created: Utc::now(),
        };
        let first = entry("first");
        let first_item = cache_entry_item(first.clone());
        let budget = cache_iteration_item_increment(&first_item, 0, true).unwrap();
        let mut entries = Vec::new();
        let mut cursor = None;
        let mut materialized_bytes = 0;

        assert!(try_materialize_cache_iteration_entry(
            &mut entries,
            &mut cursor,
            &mut materialized_bytes,
            first,
            true,
            budget,
        )
        .unwrap());
        assert!(!try_materialize_cache_iteration_entry(
            &mut entries,
            &mut cursor,
            &mut materialized_bytes,
            entry("second"),
            true,
            budget,
        )
        .unwrap());
        assert_eq!(cursor.as_deref(), Some("first"));
        assert_eq!(entries.len(), 1);
        assert_eq!(materialized_bytes, budget);
    }

    #[test]
    fn test_free_slots_calculation() {
        // Simulates the free-slots logic in advance_workflow
        let concurrency_limit = 3usize;

        // 2 in-flight → 1 free slot
        let in_flight = 2usize;
        let free = concurrency_limit.saturating_sub(in_flight);
        assert_eq!(free, 1);

        // 0 in-flight → 3 free slots
        let in_flight = 0usize;
        let free = concurrency_limit.saturating_sub(in_flight);
        assert_eq!(free, 3);

        // 3 in-flight → 0 free slots
        let in_flight = 3usize;
        let free = concurrency_limit.saturating_sub(in_flight);
        assert_eq!(free, 0);
    }

    #[test]
    fn test_extract_workflow_params_flat_format() {
        let config = Some(serde_json::json!({"n": 5, "name": "test"}));
        let params = extract_workflow_params(&config);
        assert_eq!(params, serde_json::json!({"n": 5, "name": "test"}));
    }

    #[test]
    fn test_extract_workflow_params_none() {
        let params = extract_workflow_params(&None);
        assert_eq!(params, serde_json::json!({}));
    }

    #[test]
    fn test_extract_workflow_params_non_object() {
        let config = Some(serde_json::json!("not an object"));
        let params = extract_workflow_params(&config);
        assert_eq!(params, serde_json::json!({}));
    }

    #[test]
    fn test_extract_workflow_params_empty_object() {
        let config = Some(serde_json::json!({}));
        let params = extract_workflow_params(&config);
        assert_eq!(params, serde_json::json!({}));
    }

    #[test]
    fn test_normalize_workflow_permission_set_refs_accepts_string() {
        let refs = ExecutionScheduler::normalize_workflow_permission_set_refs(
            "agent",
            serde_json::json!(" core.agent "),
        )
        .unwrap();
        assert_eq!(refs, vec!["core.agent"]);
    }

    #[test]
    fn test_normalize_workflow_permission_set_refs_accepts_array_and_dedupes() {
        let refs = ExecutionScheduler::normalize_workflow_permission_set_refs(
            "agent",
            serde_json::json!(["core.agent", "core.agent", "core.reader", ""]),
        )
        .unwrap();
        assert_eq!(refs, vec!["core.agent", "core.reader"]);
    }

    #[test]
    fn test_normalize_workflow_permission_set_refs_rejects_non_string_items() {
        let err = ExecutionScheduler::normalize_workflow_permission_set_refs(
            "agent",
            serde_json::json!(["core.agent", 5]),
        )
        .unwrap_err();
        assert!(err.to_string().contains("array of strings"));
    }

    #[test]
    fn test_extract_workflow_params_with_parameters_key() {
        // A "parameters" key is just a regular parameter — not unwrapped
        let config = Some(serde_json::json!({
            "parameters": {"n": 5},
            "context": {"rule": "test"}
        }));
        let params = extract_workflow_params(&config);
        // Returns the whole object as-is — "parameters" is treated as a normal key
        assert_eq!(
            params,
            serde_json::json!({"parameters": {"n": 5}, "context": {"rule": "test"}})
        );
    }

    #[test]
    fn test_workflow_delay_context_formats_workflow_child() {
        let execution = attune_common::models::Execution {
            id: 42,
            action: Some(7),
            action_ref: "python_example.simulate_work".to_string(),
            config: None,
            env_vars: None,
            parent: Some(5),
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            retry_count: 0,
            max_retries: None,
            retry_reason: None,
            original_execution: None,
            started_at: None,
            timeout_seconds: None,
            workflow_task: Some(attune_common::models::execution::WorkflowTaskMetadata {
                workflow_execution: 9,
                task_name: "merge_results".to_string(),
                triggered_by: Some("run_linter".to_string()),
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
            created: Utc::now(),
            updated: Utc::now(),
        };

        let context = ExecutionScheduler::workflow_delay_context(&execution).unwrap();
        assert!(context.contains("merge_results"));
        assert!(context.contains("execution 42"));
        assert!(context.contains("workflow_execution 9"));
        assert!(context.contains("python_example.simulate_work"));
        assert!(context.contains("triggered by 'run_linter'"));
    }

    #[test]
    fn test_workflow_delay_context_ignores_non_workflow_execution() {
        let execution = attune_common::models::Execution {
            id: 42,
            action: Some(7),
            action_ref: "core.echo".to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            retry_count: 0,
            max_retries: None,
            retry_reason: None,
            original_execution: None,
            started_at: None,
            timeout_seconds: None,
            workflow_task: None,
            created: Utc::now(),
            updated: Utc::now(),
        };

        assert!(ExecutionScheduler::workflow_delay_context(&execution).is_none());
    }

    #[test]
    fn test_reconcile_authoritative_task_statuses_backfills_join_predecessors() {
        let mut completed_tasks = vec!["security_scan".to_string()];
        let mut failed_tasks = Vec::new();

        reconcile_authoritative_task_statuses(
            &mut completed_tasks,
            &mut failed_tasks,
            vec![
                (
                    "build_artifacts".to_string(),
                    None,
                    ExecutionStatus::Completed,
                    None,
                    0,
                ),
                (
                    "run_linter".to_string(),
                    None,
                    ExecutionStatus::Completed,
                    None,
                    0,
                ),
                (
                    "security_scan".to_string(),
                    None,
                    ExecutionStatus::Completed,
                    None,
                    0,
                ),
                // Incomplete with_items children must not make the parent task look done.
                (
                    "process_items".to_string(),
                    Some(0),
                    ExecutionStatus::Completed,
                    None,
                    0,
                ),
                (
                    "process_items".to_string(),
                    Some(1),
                    ExecutionStatus::Running,
                    None,
                    0,
                ),
            ],
        );

        assert!(completed_tasks.contains(&"build_artifacts".to_string()));
        assert!(completed_tasks.contains(&"run_linter".to_string()));
        assert!(completed_tasks.contains(&"security_scan".to_string()));
        assert!(!completed_tasks.contains(&"process_items".to_string()));
        assert!(failed_tasks.is_empty());
    }

    #[test]
    fn test_reconcile_authoritative_task_statuses_backfills_failures() {
        let mut completed_tasks = Vec::new();
        let mut failed_tasks = Vec::new();

        reconcile_authoritative_task_statuses(
            &mut completed_tasks,
            &mut failed_tasks,
            vec![
                (
                    "build_artifacts".to_string(),
                    None,
                    ExecutionStatus::Failed,
                    None,
                    0,
                ),
                (
                    "security_scan".to_string(),
                    None,
                    ExecutionStatus::Timeout,
                    None,
                    0,
                ),
                (
                    "process_items".to_string(),
                    Some(1),
                    ExecutionStatus::Failed,
                    None,
                    0,
                ),
            ],
        );

        assert!(failed_tasks.contains(&"build_artifacts".to_string()));
        assert!(failed_tasks.contains(&"security_scan".to_string()));
        assert!(failed_tasks.contains(&"process_items".to_string()));
        assert!(completed_tasks.is_empty());
    }

    #[test]
    fn test_reconcile_authoritative_task_statuses_uses_latest_retry_attempt() {
        let mut completed_tasks = Vec::new();
        let mut failed_tasks = vec!["flaky_task".to_string()];

        reconcile_authoritative_task_statuses(
            &mut completed_tasks,
            &mut failed_tasks,
            vec![
                (
                    "flaky_task".to_string(),
                    None,
                    ExecutionStatus::Failed,
                    None,
                    0,
                ),
                (
                    "flaky_task".to_string(),
                    None,
                    ExecutionStatus::Failed,
                    None,
                    1,
                ),
                (
                    "flaky_task".to_string(),
                    None,
                    ExecutionStatus::Completed,
                    None,
                    2,
                ),
            ],
        );

        assert!(completed_tasks.contains(&"flaky_task".to_string()));
        assert!(!failed_tasks.contains(&"flaky_task".to_string()));
    }

    #[test]
    fn test_scheduling_persists_selected_worker() {
        let mut execution = attune_common::models::Execution {
            id: 42,
            action: Some(7),
            action_ref: "core.sleep".to_string(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            retry_count: 0,
            max_retries: None,
            retry_reason: None,
            original_execution: None,
            started_at: None,
            timeout_seconds: None,
            workflow_task: None,
            created: Utc::now(),
            updated: Utc::now(),
        };

        execution.status = ExecutionStatus::Scheduled;
        execution.worker = Some(99);

        let update: UpdateExecutionInput = execution.into();
        assert_eq!(update.status, Some(ExecutionStatus::Scheduled));
        assert_eq!(update.worker, Some(99));
    }

    #[test]
    fn test_workflow_advancement_halts_for_any_cancellation_state() {
        assert!(ExecutionScheduler::should_halt_workflow_advancement(
            ExecutionStatus::Running,
            ExecutionStatus::Canceling,
            ExecutionStatus::Completed
        ));
        assert!(ExecutionScheduler::should_halt_workflow_advancement(
            ExecutionStatus::Cancelled,
            ExecutionStatus::Running,
            ExecutionStatus::Failed
        ));
        assert!(ExecutionScheduler::should_halt_workflow_advancement(
            ExecutionStatus::Running,
            ExecutionStatus::Running,
            ExecutionStatus::Cancelled
        ));
        assert!(!ExecutionScheduler::should_halt_workflow_advancement(
            ExecutionStatus::Running,
            ExecutionStatus::Running,
            ExecutionStatus::Failed
        ));
    }

    fn test_task_node(name: &str, timeout: Option<JsonValue>) -> TaskNode {
        TaskNode {
            name: name.to_string(),
            task_type: TaskType::Action,
            action: Some("core.echo".to_string()),
            input: serde_json::json!({}),
            permission_set_refs: None,
            trace_tag_template: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            when: None,
            with_items: None,
            iterate_cache: None,
            batch_size: None,
            concurrency: None,
            retry: None,
            timeout,
            transitions: Vec::new(),
            sub_tasks: None,
            inbound_tasks: HashSet::new(),
            join: None,
        }
    }

    #[test]
    fn test_resolve_workflow_task_timeout_no_timeout_is_none() {
        let task = test_task_node("no_timeout", None);
        let wf_ctx = WorkflowContext::new(serde_json::json!({}), HashMap::new());
        assert_eq!(
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).unwrap(),
            None
        );
    }

    #[test]
    fn test_resolve_workflow_task_timeout_literal_number() {
        let task = test_task_node("literal", Some(serde_json::json!(300)));
        let wf_ctx = WorkflowContext::new(serde_json::json!({}), HashMap::new());
        assert_eq!(
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).unwrap(),
            Some(300)
        );
    }

    #[test]
    fn test_resolve_workflow_task_timeout_literal_null_is_none() {
        let task = test_task_node("null_literal", Some(serde_json::json!(null)));
        let wf_ctx = WorkflowContext::new(serde_json::json!({}), HashMap::new());
        assert_eq!(
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).unwrap(),
            None
        );
    }

    #[test]
    fn test_resolve_workflow_task_timeout_template_to_number() {
        let task = test_task_node(
            "templated_number",
            Some(serde_json::json!("{{ parameters.start_task_timeout_seconds }}")),
        );
        let wf_ctx = WorkflowContext::new(
            serde_json::json!({ "start_task_timeout_seconds": 300 }),
            HashMap::new(),
        );
        assert_eq!(
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).unwrap(),
            Some(300)
        );
    }

    #[test]
    fn test_resolve_workflow_task_timeout_template_to_string_integer() {
        let task = test_task_node(
            "templated_string",
            Some(serde_json::json!("{{ parameters.monitor_task_timeout_seconds }}")),
        );
        let wf_ctx = WorkflowContext::new(
            serde_json::json!({ "monitor_task_timeout_seconds": "3900" }),
            HashMap::new(),
        );
        assert_eq!(
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).unwrap(),
            Some(3900)
        );
    }

    #[test]
    fn test_resolve_workflow_task_timeout_template_to_null_is_none() {
        let task = test_task_node(
            "templated_null",
            Some(serde_json::json!("{{ parameters.unset_timeout }}")),
        );
        let wf_ctx = WorkflowContext::new(
            serde_json::json!({ "unset_timeout": null }),
            HashMap::new(),
        );
        assert_eq!(
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).unwrap(),
            None
        );
    }

    #[test]
    fn test_resolve_workflow_task_timeout_rejects_non_integer() {
        let cases = [
            serde_json::json!(3.5),
            serde_json::json!(true),
            serde_json::json!({}),
            serde_json::json!("{{ parameters.not_a_number }}"),
        ];
        let wf_ctx = WorkflowContext::new(
            serde_json::json!({ "not_a_number": "abc" }),
            HashMap::new(),
        );
        for timeout in cases {
            let task = test_task_node("bad_timeout", Some(timeout));
            let result = ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx);
            assert!(
                result.is_err(),
                "expected non-integer timeout to fail; got {:?}",
                result
            );
        }
    }

    #[test]
    fn test_resolve_workflow_task_timeout_rejects_negative() {
        let task = test_task_node("negative", Some(serde_json::json!(-5)));
        let wf_ctx = WorkflowContext::new(serde_json::json!({}), HashMap::new());
        let error =
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).unwrap_err();
        assert!(
            error.to_string().contains("non-negative"),
            "unexpected error: {}",
            error
        );
    }

    #[test]
    fn test_resolve_workflow_task_timeout_rejects_template_to_negative() {
        let task = test_task_node(
            "templated_negative",
            Some(serde_json::json!("{{ parameters.negative_timeout }}")),
        );
        let wf_ctx = WorkflowContext::new(
            serde_json::json!({ "negative_timeout": -5 }),
            HashMap::new(),
        );
        assert!(
            ExecutionScheduler::resolve_workflow_task_timeout(&task, &wf_ctx).is_err()
        );
    }
}
